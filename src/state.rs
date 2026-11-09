//! In-memory server state: connected clients, pending invites, share sessions and file offers.
//!
//! Nothing is persisted; everything disappears with the session or the process.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::Config;
use crate::protocol::{ClientInfo, InviteOutcome, ServerMsg, SessionEndReason};

pub type ClientTx = mpsc::UnboundedSender<ServerMsg>;
pub type ChunkTx = mpsc::Sender<Result<Bytes, io::Error>>;

pub struct Client {
    pub id: String,
    pub token: String,
    pub name: Option<String>,
    pub device: String,
    pub ip: String,
    /// `None` while the client is disconnected but still inside the reconnect grace period.
    pub tx: Option<ClientTx>,
    /// Incremented on every (re)connect so stale disconnect timers can be ignored.
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
    pub owner: String,
    pub receiver: String,
    pub size: u64,
    pub tx: ChunkTx,
}

#[derive(Default)]
pub struct Inner {
    pub clients: HashMap<String, Client>,
    pub invites: HashMap<String, Invite>,
    pub sessions: HashMap<String, Session>,
    pub files: HashMap<String, FileOffer>,
    pub uploads: HashMap<String, PendingUpload>,
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

impl Inner {
    /// Sends a message to a client if it is currently connected. Returns whether it was queued.
    pub fn send(&self, client_id: &str, msg: ServerMsg) -> bool {
        match self.clients.get(client_id).and_then(|c| c.tx.as_ref()) {
            Some(tx) => tx.send(msg).is_ok(),
            None => false,
        }
    }

    pub fn is_online(&self, client_id: &str) -> bool {
        self.clients.get(client_id).is_some_and(|c| c.tx.is_some())
    }

    pub fn client_by_token(&self, token: &str) -> Option<&Client> {
        self.clients.values().find(|c| c.token == token)
    }

    pub fn broadcast_clients(&self) {
        let mut clients: Vec<ClientInfo> =
            self.clients.values().filter(|c| c.tx.is_some()).map(Client::info).collect();
        clients.sort_by(|a, b| a.ip.cmp(&b.ip).then_with(|| a.id.cmp(&b.id)));
        for c in self.clients.values() {
            if let Some(tx) = &c.tx {
                let _ = tx.send(ServerMsg::Clients { clients: clients.clone() });
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

    pub fn end_session(&mut self, session_id: &str, reason: SessionEndReason, by: Option<&str>) {
        let Some(session) = self.sessions.remove(session_id) else { return };
        self.files.retain(|_, f| f.session_id != session_id);
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
        self.files.retain(|_, f| f.owner != client_id && f.receiver != client_id);
        self.uploads.retain(|_, u| u.owner != client_id && u.receiver != client_id);
        self.clients.remove(client_id);
        self.broadcast_clients();
    }
}
