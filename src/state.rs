//! In-memory server state: connected clients, pending invites, share sessions and file offers.
//!
//! Nothing is persisted; everything disappears with the session or the process.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::ws::Utf8Bytes;
use bytes::Bytes;
use tokio::sync::{Notify, mpsc};

use crate::Config;
use crate::protocol::{ClientInfo, InviteOutcome, ServerMsg, SessionEndReason};

pub type ChunkTx = mpsc::Sender<Result<Bytes, io::Error>>;

/// Queued but unsent bytes above which relayed user content is refused for a client.
const RELAY_QUEUE_LIMIT: usize = 4 << 20;
/// Queued but unsent bytes above which a client is considered stuck and disconnected.
const HARD_QUEUE_LIMIT: usize = 16 << 20;
/// How long a download ticket stays valid.
const TICKET_TTL: Duration = Duration::from_secs(30);

/// The sending half of one WebSocket connection.
pub struct Conn {
    pub tx: mpsc::UnboundedSender<Utf8Bytes>,
    /// Bytes handed to the writer task that are not yet written to the socket.
    pub queued: Arc<AtomicUsize>,
    /// Signalled to terminate the connection (stuck client or taken over by a reconnect).
    pub kick: Arc<Notify>,
}

impl Conn {
    fn push(&self, msg: Utf8Bytes, limit: usize) -> bool {
        let len = msg.len();
        if self.queued.load(Ordering::Relaxed) + len > limit {
            if limit == HARD_QUEUE_LIMIT {
                self.kick.notify_one();
            }
            return false;
        }
        self.queued.fetch_add(len, Ordering::Relaxed);
        if self.tx.send(msg).is_err() {
            self.queued.fetch_sub(len, Ordering::Relaxed);
            return false;
        }
        true
    }
}

pub struct Client {
    pub id: String,
    pub token: String,
    pub name: Option<String>,
    pub device: String,
    pub ip: String,
    /// `None` while the client is disconnected but still inside the reconnect grace period.
    pub conn: Option<Conn>,
    /// Incremented on every (re)connect so stale connections and timers can be ignored.
    pub conn_gen: u64,
}

impl Client {
    pub fn info(&self) -> ClientInfo {
        ClientInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            device: self.device.clone(),
            ip: self.ip.clone(),
        }
    }
}

pub struct Invite {
    pub from: String,
    pub to: String,
    pub expires_at: Instant,
}

pub struct Session {
    pub a: String,
    pub b: String,
}

impl Session {
    pub fn peer_of(&self, id: &str) -> Option<&str> {
        if self.a == id {
            Some(&self.b)
        } else if self.b == id {
            Some(&self.a)
        } else {
            None
        }
    }
}

pub struct FileOffer {
    pub session_id: String,
    pub owner: String,
    pub receiver: String,
    pub name: String,
    pub size: u64,
    pub mime: String,
}

/// A download that is waiting for the sender to start the matching upload.
pub struct PendingUpload {
    pub file_id: String,
    pub session_id: String,
    pub owner: String,
    pub receiver: String,
    pub size: u64,
    pub tx: ChunkTx,
    /// Bytes the download response has taken from the channel.
    pub consumed: Arc<AtomicU64>,
}

/// A running upload, so that ending its session can abort it.
pub struct ActiveTransfer {
    pub session_id: String,
    pub abort: Arc<Notify>,
}

/// A single-use permission to download one file, so the long-lived token never appears in a URL.
pub struct Ticket {
    pub client: String,
    pub file_id: String,
    pub expires_at: Instant,
}

#[derive(Default)]
pub struct Inner {
    pub clients: HashMap<String, Client>,
    /// Token -> client id.
    pub tokens: HashMap<String, String>,
    pub invites: HashMap<String, Invite>,
    pub sessions: HashMap<String, Session>,
    pub files: HashMap<String, FileOffer>,
    pub uploads: HashMap<String, PendingUpload>,
    pub transfers: HashMap<String, ActiveTransfer>,
    pub tickets: HashMap<String, Ticket>,
}

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Mutex<Inner>>,
    pub config: Arc<Config>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self { inner: Arc::default(), config: Arc::new(config) }
    }

    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub fn encode(msg: &ServerMsg) -> Utf8Bytes {
    serde_json::to_string(msg).unwrap_or_default().into()
}

impl Inner {
    fn conn(&self, client_id: &str) -> Option<&Conn> {
        self.clients.get(client_id).and_then(|c| c.conn.as_ref())
    }

    /// Sends a control message. A client whose queue overflows is disconnected.
    pub fn send(&self, client_id: &str, msg: ServerMsg) -> bool {
        self.conn(client_id).is_some_and(|c| c.push(encode(&msg), HARD_QUEUE_LIMIT))
    }

    /// Sends user content from a peer. Refused (returns false) while the receiver is not keeping up.
    pub fn relay(&self, client_id: &str, msg: ServerMsg) -> bool {
        self.conn(client_id).is_some_and(|c| c.push(encode(&msg), RELAY_QUEUE_LIMIT))
    }

    pub fn is_online(&self, client_id: &str) -> bool {
        self.conn(client_id).is_some()
    }

    pub fn client_by_token(&self, token: &str) -> Option<&Client> {
        self.tokens.get(token).and_then(|id| self.clients.get(id))
    }

    pub fn broadcast_clients(&self) {
        let mut clients: Vec<ClientInfo> =
            self.clients.values().filter(|c| c.conn.is_some()).map(Client::info).collect();
        clients.sort_by(|a, b| a.ip.cmp(&b.ip).then_with(|| a.id.cmp(&b.id)));
        let msg = encode(&ServerMsg::Clients { clients });
        for c in self.clients.values() {
            if let Some(conn) = &c.conn {
                conn.push(msg.clone(), HARD_QUEUE_LIMIT);
            }
        }
    }

    pub fn session_between(&self, x: &str, y: &str) -> Option<&String> {
        self.sessions
            .iter()
            .find(|(_, s)| (s.a == x && s.b == y) || (s.a == y && s.b == x))
            .map(|(id, _)| id)
    }

    pub fn sessions_of(&self, client_id: &str) -> Vec<String> {
        self.sessions.iter().filter(|(_, s)| s.peer_of(client_id).is_some()).map(|(id, _)| id.clone()).collect()
    }

    /// Reports a failed transfer to both sides.
    pub fn transfer_failed(&self, owner: &str, receiver: &str, file_id: &str, req_id: &str, error: &str) {
        let msg = ServerMsg::TransferFailed { file_id: file_id.into(), req_id: req_id.into(), error: error.into() };
        self.send(owner, msg.clone());
        self.send(receiver, msg);
    }

    pub fn end_session(&mut self, session_id: &str, reason: SessionEndReason, by: Option<&str>) {
        let Some(session) = self.sessions.remove(session_id) else { return };
        self.files.retain(|_, f| f.session_id != session_id);
        self.tickets.retain(|_, t| self.files.contains_key(&t.file_id));
        // Downloads that wait for their upload end with an error, running uploads are aborted.
        self.uploads.retain(|_, u| {
            if u.session_id == session_id {
                let _ = u.tx.try_send(Err(io::Error::other("session ended")));
                false
            } else {
                true
            }
        });
        for t in self.transfers.values().filter(|t| t.session_id == session_id) {
            t.abort.notify_one();
        }
        for member in [&session.a, &session.b] {
            self.send(
                member,
                ServerMsg::SessionClosed {
                    session_id: session_id.to_string(),
                    reason,
                    by_me: by == Some(member.as_str()),
                },
            );
        }
    }

    /// Resolves an invite and notifies both sides. Returns the invite if it still existed.
    pub fn resolve_invite(&mut self, invite_id: &str, outcome: InviteOutcome, session_id: Option<String>) -> Option<Invite> {
        let invite = self.invites.remove(invite_id)?;
        self.send(
            &invite.from,
            ServerMsg::InviteResult {
                invite_id: invite_id.to_string(),
                to: invite.to.clone(),
                outcome,
                session_id,
            },
        );
        self.send(&invite.to, ServerMsg::InviteClosed { invite_id: invite_id.to_string(), outcome });
        Some(invite)
    }

    pub fn issue_ticket(&mut self, client: &str, file_id: &str) -> String {
        let now = Instant::now();
        self.tickets.retain(|_, t| t.expires_at > now);
        let ticket = new_id();
        self.tickets.insert(
            ticket.clone(),
            Ticket { client: client.to_string(), file_id: file_id.to_string(), expires_at: now + TICKET_TTL },
        );
        ticket
    }

    /// Consumes a ticket and returns the client it was issued to.
    pub fn redeem_ticket(&mut self, ticket: &str, file_id: &str) -> Option<String> {
        let t = self.tickets.remove(ticket)?;
        (t.file_id == file_id && t.expires_at > Instant::now()).then_some(t.client)
    }

    /// Removes a client for good (after the reconnect grace period ran out).
    pub fn remove_client(&mut self, client_id: &str) {
        for sid in self.sessions_of(client_id) {
            self.end_session(&sid, SessionEndReason::Disconnected, None);
        }
        let invites: Vec<(String, bool)> = self
            .invites
            .iter()
            .filter(|(_, i)| i.from == client_id || i.to == client_id)
            .map(|(id, i)| (id.clone(), i.from == client_id))
            .collect();
        for (id, was_sender) in invites {
            let outcome = if was_sender { InviteOutcome::Cancelled } else { InviteOutcome::Unavailable };
            self.resolve_invite(&id, outcome, None);
        }
        self.tickets.retain(|_, t| t.client != client_id);
        if let Some(c) = self.clients.remove(client_id) {
            self.tokens.remove(&c.token);
        }
        self.broadcast_clients();
    }
}
