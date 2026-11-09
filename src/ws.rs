//! WebSocket endpoint: presence, invites, sessions and text relay.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{Notify, mpsc};

use crate::protocol::{ClientMsg, InviteOutcome, ServerMsg, SessionEndReason};
use crate::state::{AppState, Client, Conn, FileOffer, Inner, Invite, Session, encode, new_id, now_ms};

const MAX_NAME_LEN: usize = 64;
const MAX_DEVICE_LEN: usize = 100;
const MAX_ID_LEN: usize = 64;
/// Maximum text message size in bytes. Longer texts are sent as a file by the browser.
pub const MAX_TEXT_LEN: usize = 1 << 20;
/// JSON escaping can grow text up to 6x (`\u0001`), plus room for the other fields.
const MAX_FRAME_LEN: usize = 6 * MAX_TEXT_LEN + (64 << 10);
const MAX_FILES_PER_SESSION: usize = 10_000;
/// Browsers send an application-level ping every 20 s; a silent connection is considered dead.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

pub async fn handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    if !same_origin(&headers) {
        return (StatusCode::FORBIDDEN, "cross-origin WebSocket connections are not allowed").into_response();
    }
    ws.max_message_size(MAX_FRAME_LEN).on_upgrade(move |socket| handle_socket(socket, state, addr))
}

/// Rejects WebSockets opened by other websites. Non-browser clients send no Origin and are allowed.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) else { return true };
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default();
    let origin_host = origin.split_once("://").map_or(origin, |(_, rest)| rest);
    !host.is_empty() && origin_host.eq_ignore_ascii_case(host)
}

fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

fn clean_name(name: Option<String>) -> Option<String> {
    name.map(|n| clean(&n, MAX_NAME_LEN)).filter(|n| !n.is_empty())
}

async fn handle_socket(socket: WebSocket, state: AppState, addr: SocketAddr) {
    let (mut sink, mut stream) = socket.split();

    // The first message must be a hello.
    let hello = loop {
        match tokio::time::timeout(Duration::from_secs(15), stream.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => match serde_json::from_str::<ClientMsg>(&t) {
                Ok(msg @ ClientMsg::Hello { .. }) => break msg,
                _ => return,
            },
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => continue,
            _ => return,
        }
    };
    let ClientMsg::Hello { client_id, token, name, device } = hello else { return };

    let (tx, mut rx) = mpsc::unbounded_channel::<Utf8Bytes>();
    let queued = Arc::new(AtomicUsize::new(0));
    let kick = Arc::new(Notify::new());
    let conn = Conn { tx, queued: queued.clone(), kick: kick.clone() };
    let ip = addr.ip().to_canonical().to_string();
    let Some((my_id, conn_gen)) = register(&state, client_id, token, name, device, ip, conn) else {
        let msg = encode(&ServerMsg::Error { message: "too many connections from your address".into() });
        let _ = sink.send(Message::Text(msg)).await;
        let _ = sink.close().await;
        return;
    };

    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let len = msg.len();
            if sink.send(Message::Text(msg)).await.is_err() {
                break;
            }
            queued.fetch_sub(len, Ordering::Relaxed);
        }
        let _ = sink.close().await;
    });

    let reader = async {
        while let Ok(Some(Ok(msg))) = tokio::time::timeout(READ_TIMEOUT, stream.next()).await {
            match msg {
                Message::Text(t) => match serde_json::from_str::<ClientMsg>(&t) {
                    Ok(msg) => dispatch(&state, &my_id, msg),
                    Err(e) => {
                        state.lock().send(&my_id, ServerMsg::Error { message: format!("invalid message: {e}") });
                    }
                },
                Message::Close(_) => break,
                _ => {}
            }
        }
    };
    tokio::select! {
        _ = reader => {}
        _ = kick.notified() => {}
    }

    disconnect(&state, &my_id, conn_gen);
    writer.abort();
}

/// Registers a new client or resumes an existing one (same id + token within the grace period).
/// Returns `None` when the address already has too many clients.
fn register(
    state: &AppState,
    client_id: Option<String>,
    token: Option<String>,
    name: Option<String>,
    device: String,
    ip: String,
    conn: Conn,
) -> Option<(String, u64)> {
    let mut inner = state.lock();
    let name = clean_name(name);
    let device = clean(&device, MAX_DEVICE_LEN);

    let resumable = match (&client_id, &token) {
        (Some(id), Some(token)) => inner.clients.get(id).is_some_and(|c| &c.token == token),
        _ => false,
    };

    let (id, conn_gen) = if resumable {
        let id = client_id.unwrap();
        let c = inner.clients.get_mut(&id).unwrap();
        // A reconnect takes over: the old connection may still look alive (dead link not yet noticed).
        // Duplicated browser tabs are detected in the browser and never send stored credentials.
        if let Some(old) = c.conn.replace(conn) {
            old.kick.notify_one();
        }
        c.conn_gen += 1;
        c.name = name;
        c.device = device;
        c.ip = ip.clone();
        (id, c.conn_gen)
    } else {
        if inner.clients.values().filter(|c| c.ip == ip).count() >= state.config.max_clients_per_ip {
            return None;
        }
        let id = new_id();
        let token = new_id();
        inner.tokens.insert(token.clone(), id.clone());
        inner.clients.insert(
            id.clone(),
            Client { id: id.clone(), token, name, device, ip: ip.clone(), conn: Some(conn), conn_gen: 0 },
        );
        (id, 0)
    };

    let token = inner.clients[&id].token.clone();
    inner.send(
        &id,
        ServerMsg::Welcome {
            client_id: id.clone(),
            token,
            ip,
            resumed: resumable,
            invite_timeout_secs: state.config.invite_timeout,
        },
    );

    if resumable {
        restore(&inner, &id);
    }

    inner.broadcast_clients();
    Some((id, conn_gen))
}

/// Re-sends sessions and pending invites to a resumed client.
fn restore(inner: &Inner, id: &str) {
    for sid in inner.sessions_of(id) {
        let session = &inner.sessions[&sid];
        let peer_id = session.peer_of(id).unwrap();
        let initiator = session.a == id;
        let Some(peer) = inner.clients.get(peer_id).map(Client::info) else { continue };
        inner.send(id, ServerMsg::SessionStarted { session_id: sid.clone(), peer, initiator });
        inner.send(id, ServerMsg::PeerStatus { session_id: sid.clone(), online: inner.is_online(peer_id) });
        inner.send(peer_id, ServerMsg::PeerStatus { session_id: sid, online: true });
    }
    let now = Instant::now();
    for (iid, inv) in &inner.invites {
        let expires_in_ms = inv.expires_at.saturating_duration_since(now).as_millis() as u64;
        if inv.to == id {
            if let Some(from) = inner.clients.get(&inv.from).map(Client::info) {
                inner.send(id, ServerMsg::InviteReceived { invite_id: iid.clone(), from, expires_in_ms });
            }
        } else if inv.from == id {
            inner.send(id, ServerMsg::InviteSent { invite_id: iid.clone(), to: inv.to.clone(), expires_in_ms });
        }
    }
}

fn disconnect(state: &AppState, id: &str, conn_gen: u64) {
    {
        let mut inner = state.lock();
        let Some(c) = inner.clients.get_mut(id) else { return };
        if c.conn_gen != conn_gen {
            return; // already replaced by a newer connection
        }
        c.conn = None;
        for sid in inner.sessions_of(id) {
            let peer = inner.sessions[&sid].peer_of(id).unwrap().to_string();
            inner.send(&peer, ServerMsg::PeerStatus { session_id: sid, online: false });
        }
        inner.broadcast_clients();
    }

    let state = state.clone();
    let id = id.to_string();
    let grace = Duration::from_secs(state.config.reconnect_grace);
    tokio::spawn(async move {
        tokio::time::sleep(grace).await;
        let mut inner = state.lock();
        let expired = inner.clients.get(&id).is_some_and(|c| c.conn_gen == conn_gen && c.conn.is_none());
        if expired {
            inner.remove_client(&id);
        }
    });
}

fn dispatch(state: &AppState, me: &str, msg: ClientMsg) {
    let mut inner = state.lock();
    let error = |inner: &Inner, message: &str| {
        inner.send(me, ServerMsg::Error { message: message.to_string() });
    };
    let peer_in = |inner: &Inner, session_id: &str| {
        inner.sessions.get(session_id).and_then(|s| s.peer_of(me)).map(str::to_string)
    };

    match msg {
        ClientMsg::Hello { .. } => error(&inner, "already greeted"),
        ClientMsg::Ping => {
            inner.send(me, ServerMsg::Pong);
        }

        ClientMsg::SetName { name } => {
            if let Some(c) = inner.clients.get_mut(me) {
                c.name = clean_name(name);
            }
            inner.broadcast_clients();
        }

        ClientMsg::Invite { to } => {
            if to == me {
                return error(&inner, "you cannot invite yourself");
            }
            if !inner.is_online(&to) {
                return error(&inner, "that client is not online");
            }
            if inner.session_between(me, &to).is_some() {
                return error(&inner, "a session with that client is already open");
            }
            if inner.invites.values().any(|i| (i.from == me && i.to == to) || (i.from == to && i.to == me)) {
                return error(&inner, "an invite between you and that client is already pending");
            }
            let timeout = Duration::from_secs(state.config.invite_timeout);
            let invite_id = new_id();
            let from = inner.clients[me].info();
            inner.invites.insert(
                invite_id.clone(),
                Invite { from: me.to_string(), to: to.clone(), expires_at: Instant::now() + timeout },
            );
            let expires_in_ms = timeout.as_millis() as u64;
            inner.send(me, ServerMsg::InviteSent { invite_id: invite_id.clone(), to: to.clone(), expires_in_ms });
            inner.send(&to, ServerMsg::InviteReceived { invite_id: invite_id.clone(), from, expires_in_ms });

            let state = state.clone();
            tokio::spawn(async move {
                tokio::time::sleep(timeout).await;
                state.lock().resolve_invite(&invite_id, InviteOutcome::Expired, None);
            });
        }

        ClientMsg::CancelInvite { invite_id } => {
            if inner.invites.get(&invite_id).is_some_and(|i| i.from == me) {
                inner.resolve_invite(&invite_id, InviteOutcome::Cancelled, None);
            }
        }

        ClientMsg::RespondInvite { invite_id, accept } => {
            let Some(invite) = inner.invites.get(&invite_id) else {
                return error(&inner, "that invite is no longer valid");
            };
            if invite.to != me {
                return;
            }
            if !accept {
                inner.resolve_invite(&invite_id, InviteOutcome::Denied, None);
                return;
            }
            let from = invite.from.clone();
            if !inner.is_online(&from) {
                inner.resolve_invite(&invite_id, InviteOutcome::Unavailable, None);
                return;
            }
            let session_id = new_id();
            inner.resolve_invite(&invite_id, InviteOutcome::Accepted, Some(session_id.clone()));
            inner.sessions.insert(session_id.clone(), Session { a: from.clone(), b: me.to_string() });
            let from_info = inner.clients[&from].info();
            let me_info = inner.clients[me].info();
            inner.send(&from, ServerMsg::SessionStarted { session_id: session_id.clone(), peer: me_info, initiator: true });
            inner.send(me, ServerMsg::SessionStarted { session_id, peer: from_info, initiator: false });
        }

        ClientMsg::CloseSession { session_id } => {
            if peer_in(&inner, &session_id).is_some() {
                inner.end_session(&session_id, SessionEndReason::Closed, Some(me));
            }
        }

        ClientMsg::Text { session_id, msg_id, text } => {
            let Some(peer) = peer_in(&inner, &session_id) else {
                return error(&inner, "unknown session");
            };
            let msg_id = clean(&msg_id, MAX_ID_LEN);
            let delivered = text.len() <= MAX_TEXT_LEN
                && inner.relay(
                    &peer,
                    ServerMsg::Text { session_id: session_id.clone(), msg_id: msg_id.clone(), text, ts: now_ms() },
                );
            if !delivered {
                inner.send(me, ServerMsg::DeliveryFailed { session_id, msg_id });
            }
        }

        ClientMsg::FileOffer { session_id, file_id, name, size, mime } => {
            let Some(peer) = peer_in(&inner, &session_id) else {
                return error(&inner, "unknown session");
            };
            let file_id = clean(&file_id, MAX_ID_LEN);
            if file_id.is_empty() || inner.files.contains_key(&file_id) {
                return error(&inner, "invalid file id");
            }
            if inner.files.values().filter(|f| f.session_id == session_id).count() >= MAX_FILES_PER_SESSION {
                inner.send(me, ServerMsg::DeliveryFailed { session_id, msg_id: file_id });
                return error(&inner, "too many files in this session, start a new one");
            }
            let name = {
                let n = clean(&name.replace(['/', '\\'], "_"), 255);
                if n.is_empty() { "file".to_string() } else { n }
            };
            let mime = {
                let m = clean(&mime, 127);
                if m.is_empty() || !m.contains('/') { "application/octet-stream".to_string() } else { m }
            };
            let offer = ServerMsg::FileOffer {
                session_id: session_id.clone(),
                file_id: file_id.clone(),
                name: name.clone(),
                size,
                mime: mime.clone(),
                ts: now_ms(),
            };
            if !inner.relay(&peer, offer) {
                inner.send(me, ServerMsg::DeliveryFailed { session_id, msg_id: file_id });
                return;
            }
            inner.files.insert(
                file_id,
                FileOffer { session_id, owner: me.to_string(), receiver: peer, name, size, mime },
            );
        }

        ClientMsg::RequestDownload { file_id } => {
            if !inner.files.get(&file_id).is_some_and(|f| f.receiver == me) {
                return error(&inner, "file is no longer available");
            }
            let ticket = inner.issue_ticket(me, &file_id);
            inner.send(me, ServerMsg::DownloadTicket { file_id, ticket });
        }

        ClientMsg::FileUnavailable { req_id } => {
            if inner.uploads.get(&req_id).is_some_and(|u| u.owner == me) {
                let upload = inner.uploads.remove(&req_id).unwrap();
                let error = "the sender no longer has this file (page reloaded?)";
                inner.transfer_failed(&upload.owner, &upload.receiver, &upload.file_id, &req_id, error);
                let _ = upload.tx.try_send(Err(std::io::Error::other("file unavailable")));
            }
        }
    }
}
