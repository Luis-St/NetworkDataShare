//! JSON messages exchanged over the `/ws` WebSocket.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub id: String,
    pub name: Option<String>,
    pub device: String,
    pub ip: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InviteOutcome {
    Accepted,
    Denied,
    Expired,
    Cancelled,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEndReason {
    Closed,
    Disconnected,
}

/// Messages sent from a browser to the server.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ClientMsg {
    Hello {
        client_id: Option<String>,
        token: Option<String>,
        name: Option<String>,
        device: String,
    },
    SetName {
        name: Option<String>,
    },
    Invite {
        to: String,
    },
    CancelInvite {
        invite_id: String,
    },
    RespondInvite {
        invite_id: String,
        accept: bool,
    },
    CloseSession {
        session_id: String,
    },
    Text {
        session_id: String,
        msg_id: String,
        text: String,
    },
    FileOffer {
        session_id: String,
        file_id: String,
        name: String,
        size: u64,
        mime: String,
    },
    FileUnavailable {
        req_id: String,
    },
    Ping,
}

/// Messages sent from the server to a browser.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ServerMsg {
    Welcome {
        client_id: String,
        token: String,
        ip: String,
        resumed: bool,
        invite_timeout_secs: u64,
    },
    Clients {
        clients: Vec<ClientInfo>,
    },
    InviteSent {
        invite_id: String,
        to: String,
        expires_in_ms: u64,
    },
    InviteReceived {
        invite_id: String,
        from: ClientInfo,
        expires_in_ms: u64,
    },
    /// Sent to the inviter once the invite is resolved.
    InviteResult {
        invite_id: String,
        to: String,
        outcome: InviteOutcome,
        session_id: Option<String>,
    },
    /// Sent to the invitee once the invite is resolved.
    InviteClosed {
        invite_id: String,
        outcome: InviteOutcome,
    },
    SessionStarted {
        session_id: String,
        peer: ClientInfo,
        initiator: bool,
    },
    SessionClosed {
        session_id: String,
        reason: SessionEndReason,
        by_me: bool,
    },
    PeerStatus {
        session_id: String,
        online: bool,
    },
    Text {
        session_id: String,
        msg_id: String,
        text: String,
        ts: u64,
    },
    FileOffer {
        session_id: String,
        file_id: String,
        name: String,
        size: u64,
        mime: String,
        ts: u64,
    },
    DeliveryFailed {
        session_id: String,
        msg_id: String,
    },
    UploadRequest {
        file_id: String,
        req_id: String,
    },
    TransferStarted {
        file_id: String,
        req_id: String,
        total: u64,
    },
    TransferProgress {
        file_id: String,
        req_id: String,
        bytes: u64,
        total: u64,
    },
    TransferDone {
        file_id: String,
        req_id: String,
    },
    TransferFailed {
        file_id: String,
        req_id: String,
        error: String,
    },
    Error {
        message: String,
    },
    Pong,
}
