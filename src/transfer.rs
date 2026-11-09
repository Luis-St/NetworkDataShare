//! File relay: the receiver's download is piped straight from the sender's upload.
//!
//! 1. Receiver gets a single-use ticket over the WebSocket and opens `GET /api/files/{file_id}?ticket=...`.
//! 2. Server asks the sender (via WebSocket) to upload, with a fresh request id.
//! 3. Sender sends the file as `POST /api/upload/{req_id}` with its token in the `x-nds-token` header.
//! 4. Server forwards every body chunk to the download response; nothing touches the disk.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use tokio::sync::{Notify, mpsc};
use tokio_stream::wrappers::ReceiverStream;

use crate::protocol::ServerMsg;
use crate::state::{ActiveTransfer, AppState, PendingUpload, new_id};

/// How long the sender has to start the upload after a download was requested.
const UPLOAD_START_TIMEOUT: Duration = Duration::from_secs(30);
/// A transfer that moves no data for this long is aborted.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Deserialize)]
pub struct TicketQuery {
    ticket: String,
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
    Query(q): Query<TicketQuery>,
) -> Response {
    let (rx, consumed, req_id, name, size, mime) = {
        let mut inner = state.lock();
        let Some(me) = inner.redeem_ticket(&q.ticket, &file_id) else {
            return (StatusCode::UNAUTHORIZED, "invalid or expired download ticket").into_response();
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
        let (owner, session_id, name, size, mime) =
            (file.owner.clone(), file.session_id.clone(), file.name.clone(), file.size, file.mime.clone());

        let (tx, rx) = mpsc::channel(16);
        let consumed = Arc::new(AtomicU64::new(0));
        let req_id = new_id();
        inner.uploads.insert(
            req_id.clone(),
            PendingUpload {
                file_id: file_id.clone(),
                session_id,
                owner: owner.clone(),
                receiver: me,
                size,
                tx,
                consumed: consumed.clone(),
            },
        );
        inner.send(&owner, ServerMsg::UploadRequest { file_id: file_id.clone(), req_id: req_id.clone() });
        (rx, consumed, req_id, name, size, mime)
    };

    // Give up if the sender never starts uploading.
    {
        let state = state.clone();
        let req_id = req_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(UPLOAD_START_TIMEOUT).await;
            let mut inner = state.lock();
            if let Some(upload) = inner.uploads.remove(&req_id) {
                let error = "the sender did not start the upload";
                inner.transfer_failed(&upload.owner, &upload.receiver, &upload.file_id, &req_id, error);
                let _ = upload.tx.try_send(Err(io::Error::other("upload did not start")));
            }
        });
    }

    // Count what the response actually takes, so the upload only reports success once all bytes left.
    let body = ReceiverStream::new(rx).map(move |chunk| {
        if let Ok(bytes) = &chunk {
            consumed.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        }
        chunk
    });
    let mut res = Response::new(Body::from_stream(body));
    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&mime).unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    headers.insert(header::CONTENT_DISPOSITION, content_disposition(&name));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    // Even if a browser decided to render a peer's file, it must not run in this origin.
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; sandbox"));
    res
}

/// Removes the transfer from the registry however the upload ends.
struct TransferGuard {
    state: AppState,
    req_id: String,
}

impl Drop for TransferGuard {
    fn drop(&mut self) {
        self.state.lock().transfers.remove(&self.req_id);
    }
}

pub async fn upload(
    State(state): State<AppState>,
    Path(req_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let token = headers.get("x-nds-token").and_then(|t| t.to_str().ok()).unwrap_or_default();
    let abort = Arc::new(Notify::new());
    let upload = {
        let mut inner = state.lock();
        let Some(me) = inner.client_by_token(token).map(|c| c.id.clone()) else {
            return (StatusCode::UNAUTHORIZED, "unknown client").into_response();
        };
        if !inner.uploads.get(&req_id).is_some_and(|u| u.owner == me) {
            return (StatusCode::NOT_FOUND, "no such upload request").into_response();
        }
        let upload = inner.uploads.remove(&req_id).unwrap();
        inner.transfers.insert(
            req_id.clone(),
            ActiveTransfer { session_id: upload.session_id.clone(), abort: abort.clone() },
        );
        let msg = ServerMsg::TransferStarted { file_id: upload.file_id.clone(), req_id: req_id.clone(), total: upload.size };
        inner.send(&upload.owner, msg.clone());
        inner.send(&upload.receiver, msg);
        upload
    };
    let _guard = TransferGuard { state: state.clone(), req_id: req_id.clone() };

    let fail = |error: &str, status: StatusCode| {
        let _ = upload.tx.try_send(Err(io::Error::other(error.to_string())));
        state.lock().transfer_failed(&upload.owner, &upload.receiver, &upload.file_id, &req_id, error);
        (status, error.to_string()).into_response()
    };

    let mut stream = body.into_data_stream();
    let mut bytes: u64 = 0;
    let mut last_progress = Instant::now();
    loop {
        let next = tokio::select! {
            _ = abort.notified() => return fail("the session was ended", StatusCode::GONE),
            next = tokio::time::timeout(STALL_TIMEOUT, stream.next()) => next,
        };
        let chunk = match next {
            Err(_) => return fail("the upload stalled", StatusCode::REQUEST_TIMEOUT),
            Ok(None) => break,
            Ok(Some(Err(_))) => return fail("the upload was interrupted", StatusCode::BAD_REQUEST),
            Ok(Some(Ok(c))) => c,
        };
        bytes += chunk.len() as u64;
        if bytes > upload.size {
            return fail("the sender sent more data than announced", StatusCode::BAD_REQUEST);
        }
        let sent = tokio::select! {
            _ = abort.notified() => return fail("the session was ended", StatusCode::GONE),
            sent = tokio::time::timeout(STALL_TIMEOUT, upload.tx.send(Ok(chunk))) => sent,
        };
        match sent {
            Err(_) => return fail("the download stalled", StatusCode::REQUEST_TIMEOUT),
            Ok(Err(_)) => return fail("the download was cancelled", StatusCode::GONE),
            Ok(Ok(())) => {}
        }
        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            last_progress = Instant::now();
            let msg = ServerMsg::TransferProgress {
                file_id: upload.file_id.clone(),
                req_id: req_id.clone(),
                bytes,
                total: upload.size,
            };
            let inner = state.lock();
            inner.send(&upload.owner, msg.clone());
            inner.send(&upload.receiver, msg);
        }
    }

    if bytes != upload.size {
        return fail("the upload ended early", StatusCode::BAD_REQUEST);
    }

    // Success only once the download response has taken every byte out of the channel.
    let deadline = Instant::now() + STALL_TIMEOUT;
    while upload.consumed.load(Ordering::Relaxed) < upload.size {
        if upload.tx.is_closed() {
            return fail("the download was cancelled", StatusCode::GONE);
        }
        if Instant::now() > deadline {
            return fail("the download stalled", StatusCode::REQUEST_TIMEOUT);
        }
        tokio::select! {
            _ = abort.notified() => return fail("the session was ended", StatusCode::GONE),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }

    let msg = ServerMsg::TransferDone { file_id: upload.file_id.clone(), req_id: req_id.clone() };
    let inner = state.lock();
    inner.send(&upload.owner, msg.clone());
    inner.send(&upload.receiver, msg);
    StatusCode::NO_CONTENT.into_response()
}
