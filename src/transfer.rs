//! File relay: the receiver's download is piped straight from the sender's upload.
//!
//! 1. Receiver opens `GET /api/files/{file_id}?token=...`.
//! 2. Server asks the sender (via WebSocket) to upload, with a fresh request id.
//! 3. Sender sends the file as `POST /api/upload/{req_id}?token=...`.
//! 4. Server forwards every body chunk to the download response; nothing touches the disk.

use std::io;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::protocol::ServerMsg;
use crate::state::{AppState, PendingUpload, new_id};

/// How long the sender has to start the upload after a download was requested.
const UPLOAD_START_TIMEOUT: Duration = Duration::from_secs(30);
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Deserialize)]
pub struct TokenQuery {
    token: String,
}

fn content_disposition(name: &str) -> HeaderValue {
    let ascii: String =
        name.chars().map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' }).collect();
    let encoded = utf8_percent_encode(name, NON_ALPHANUMERIC);
    HeaderValue::from_str(&format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}"))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

pub async fn download(
    State(state): State<AppState>,
    Path(file_id): Path<String>,
    Query(q): Query<TokenQuery>,
) -> Response {
    let (rx, req_id, name, size, mime) = {
        let mut inner = state.lock();
        let Some(me) = inner.client_by_token(&q.token).map(|c| c.id.clone()) else {
            return (StatusCode::UNAUTHORIZED, "unknown client").into_response();
        };
        let Some(file) = inner.files.get(&file_id) else {
            return (StatusCode::NOT_FOUND, "file is no longer available").into_response();
        };
        if file.receiver != me {
            return (StatusCode::FORBIDDEN, "not your file").into_response();
        }
        if !inner.is_online(&file.owner) {
            return (StatusCode::SERVICE_UNAVAILABLE, "sender is offline").into_response();
        }
        let (owner, name, size, mime) = (file.owner.clone(), file.name.clone(), file.size, file.mime.clone());

        let (tx, rx) = mpsc::channel(16);
        let req_id = new_id();
        inner.uploads.insert(
            req_id.clone(),
            PendingUpload { file_id: file_id.clone(), owner: owner.clone(), receiver: me, size, tx },
        );
        inner.send(&owner, ServerMsg::UploadRequest { file_id: file_id.clone(), req_id: req_id.clone() });
        (rx, req_id, name, size, mime)
    };

    // Give up if the sender never starts uploading.
    {
        let state = state.clone();
        let req_id = req_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(UPLOAD_START_TIMEOUT).await;
            let mut inner = state.lock();
            if let Some(upload) = inner.uploads.remove(&req_id) {
                let msg = ServerMsg::TransferFailed {
                    file_id: upload.file_id.clone(),
                    req_id,
                    error: "the sender did not start the upload".into(),
                };
                inner.send(&upload.owner, msg.clone());
                inner.send(&upload.receiver, msg);
                let _ = upload.tx.try_send(Err(io::Error::other("upload did not start")));
            }
        });
    }

    let mut res = Response::new(Body::from_stream(ReceiverStream::new(rx)));
    let headers = res.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    headers.insert(header::CONTENT_DISPOSITION, content_disposition(&name));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    res
}

pub async fn upload(
    State(state): State<AppState>,
    Path(req_id): Path<String>,
    Query(q): Query<TokenQuery>,
    body: Body,
) -> Response {
    let upload = {
        let mut inner = state.lock();
        let Some(me) = inner.client_by_token(&q.token).map(|c| c.id.clone()) else {
            return (StatusCode::UNAUTHORIZED, "unknown client").into_response();
        };
        if !inner.uploads.get(&req_id).is_some_and(|u| u.owner == me) {
            return (StatusCode::NOT_FOUND, "no such upload request").into_response();
        }
        let upload = inner.uploads.remove(&req_id).unwrap();
        let msg = ServerMsg::TransferStarted { file_id: upload.file_id.clone(), req_id: req_id.clone(), total: upload.size };
        inner.send(&upload.owner, msg.clone());
        inner.send(&upload.receiver, msg);
        upload
    };

    let notify = |msg: ServerMsg| {
        let inner = state.lock();
        inner.send(&upload.owner, msg.clone());
        inner.send(&upload.receiver, msg);
    };
    let fail = |error: &str| {
        notify(ServerMsg::TransferFailed { file_id: upload.file_id.clone(), req_id: req_id.clone(), error: error.into() });
    };

    let mut stream = body.into_data_stream();
    let mut bytes: u64 = 0;
    let mut last_progress = Instant::now();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = upload.tx.send(Err(io::Error::other("upload aborted"))).await;
                fail("the upload was interrupted");
                return (StatusCode::BAD_REQUEST, format!("upload failed: {e}")).into_response();
            }
        };
        bytes += chunk.len() as u64;
        if bytes > upload.size {
            let _ = upload.tx.send(Err(io::Error::other("too much data"))).await;
            fail("the sender sent more data than announced");
            return (StatusCode::BAD_REQUEST, "more data than announced").into_response();
        }
        if upload.tx.send(Ok(chunk)).await.is_err() {
            fail("the download was cancelled");
            return (StatusCode::GONE, "receiver went away").into_response();
        }
        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            last_progress = Instant::now();
            notify(ServerMsg::TransferProgress {
                file_id: upload.file_id.clone(),
                req_id: req_id.clone(),
                bytes,
                total: upload.size,
            });
        }
    }

    if bytes != upload.size {
        let _ = upload.tx.send(Err(io::Error::other("incomplete"))).await;
        fail("the upload ended early");
        return (StatusCode::BAD_REQUEST, "incomplete upload").into_response();
    }
    // Wait until the download response has consumed everything before reporting success.
    upload.tx.closed_or_drained().await;
    notify(ServerMsg::TransferDone { file_id: upload.file_id.clone(), req_id: req_id.clone() });
    StatusCode::NO_CONTENT.into_response()
}

trait Drain {
    async fn closed_or_drained(&self);
}

impl<T> Drain for mpsc::Sender<T> {
    async fn closed_or_drained(&self) {
        // The channel is drained when its full capacity is available again.
        while !self.is_closed() && self.capacity() < self.max_capacity() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
