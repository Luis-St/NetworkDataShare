// Client state and the WebSocket protocol (see src/protocol.rs on the server).

import { beep, notify } from './notify';
import { deviceHint, triggerDownload, uid } from './util';

export type ClientInfo = { id: string; name: string | null; device: string; ip: string };

export type Transfer = {
  reqId: string;
  bytes: number;
  total: number;
  state: 'waiting' | 'running' | 'done' | 'failed';
  error?: string;
};

export type TextItem = { kind: 'text'; id: string; mine: boolean; ts: number; text: string; failed?: boolean };
export type FileItem = {
  kind: 'file';
  id: string;
  mine: boolean;
  ts: number;
  name: string;
  size: number;
  mime: string;
  previewUrl?: string;
  transfer?: Transfer;
  /** Number of completed transfers to the peer (sender side). */
  delivered?: number;
  failed?: boolean;
};
export type SystemItem = { kind: 'system'; id: string; ts: number; text: string };
export type ChatItem = TextItem | FileItem | SystemItem;

export type ChatSession = {
  id: string;
  peer: ClientInfo;
  peerOnline: boolean;
  /** Set once the session has ended; the tab stays open read-only until the user closes it. */
  closed: string | null;
  items: ChatItem[];
  unread: number;
};

export type IncomingInvite = { inviteId: string; from: ClientInfo; expiresAt: number };
export type OutgoingInvite = { inviteId: string; to: string; expiresAt: number };
export type Toast = { id: string; text: string; kind: 'info' | 'error' };

/** Received images up to this size are fetched automatically and shown inline. */
const AUTO_PREVIEW_MAX = 32 * 1024 * 1024;
const PING_INTERVAL = 20_000;

type ServerMsg = { type: string; [key: string]: any };

export function displayName(c: ClientInfo): string {
  return c.name || c.device;
}

class App {
  status = $state<'connecting' | 'online' | 'offline'>('connecting');
  me = $state<{ id: string; ip: string } | null>(null);
  name = $state(localStorage.getItem('nds.name') ?? '');
  readonly device = deviceHint();
  clients = $state<ClientInfo[]>([]);
  incoming = $state<IncomingInvite[]>([]);
  outgoing = $state<Record<string, OutgoingInvite>>({});
  sessions = $state<ChatSession[]>([]);
  activeId = $state<string | null>(null);
  toasts = $state<Toast[]>([]);
  inviteTimeoutSecs = $state(60);

  private token: string | null = sessionStorage.getItem('nds.token');
  private ws: WebSocket | null = null;
  private retryDelay = 500;
  private pingTimer: number | undefined;
  /** Local files offered to peers, by file id. Only references; nothing is read until requested. */
  private files = new Map<string, File>();
  /** Which session a file belongs to, by file id. */
  private fileSession = new Map<string, string>();
  /** Sessions that must be confirmed by the server after a resumed connection. */
  private resync: Set<string> | null = null;

  get others(): ClientInfo[] {
    return this.clients.filter((c) => c.id !== this.me?.id);
  }

  get active(): ChatSession | undefined {
    return this.sessions.find((s) => s.id === this.activeId);
  }

  get unreadTotal(): number {
    return this.sessions.reduce((n, s) => n + s.unread, 0);
  }

  sessionWith(clientId: string): ChatSession | undefined {
    return this.sessions.find((s) => s.peer.id === clientId && !s.closed);
  }

  fileUrl(fileId: string): string {
    return `/api/files/${encodeURIComponent(fileId)}?token=${encodeURIComponent(this.token ?? '')}`;
  }

  // ---- connection -------------------------------------------------------------------------

  connect() {
    this.status = 'connecting';
    const proto = location.protocol === 'https:' ? 'wss' : 'ws';
    const ws = new WebSocket(`${proto}://${location.host}/ws`);
    this.ws = ws;

    ws.onopen = () => {
      this.retryDelay = 500;
      this.send({
        type: 'hello',
        clientId: sessionStorage.getItem('nds.clientId'),
        token: this.token,
        name: this.name || null,
        device: this.device,
      });
      clearInterval(this.pingTimer);
      this.pingTimer = window.setInterval(() => this.send({ type: 'ping' }), PING_INTERVAL);
    };
    ws.onmessage = (e) => {
      try {
        this.handle(JSON.parse(e.data));
      } catch (err) {
        console.error('bad message', err);
      }
    };
    ws.onclose = () => {
      if (this.ws !== ws) return;
      clearInterval(this.pingTimer);
      this.status = 'offline';
      this.ws = null;
      for (const s of this.sessions) s.peerOnline = false;
      setTimeout(() => this.connect(), this.retryDelay);
      this.retryDelay = Math.min(this.retryDelay * 2, 5000);
    };
  }

  private send(msg: Record<string, unknown>): boolean {
    if (this.ws?.readyState !== WebSocket.OPEN) return false;
    this.ws.send(JSON.stringify(msg));
    return true;
  }

  // ---- user actions -----------------------------------------------------------------------

  setName(name: string) {
    this.name = name.trim().slice(0, 64);
    localStorage.setItem('nds.name', this.name);
    this.send({ type: 'set_name', name: this.name || null });
  }

  invite(clientId: string) {
    const existing = this.sessionWith(clientId);
    if (existing) return this.activate(existing.id);
    this.send({ type: 'invite', to: clientId });
  }

  cancelInvite(clientId: string) {
    const inv = this.outgoing[clientId];
    if (inv) this.send({ type: 'cancel_invite', inviteId: inv.inviteId });
  }

  respond(inviteId: string, accept: boolean) {
    this.send({ type: 'respond_invite', inviteId, accept });
    this.incoming = this.incoming.filter((i) => i.inviteId !== inviteId);
  }

  activate(sessionId: string | null) {
    this.activeId = sessionId;
    const s = this.active;
    if (s) s.unread = 0;
  }

  /** Ends a running session, or just removes the tab of one that has already ended. */
  closeSession(sessionId: string) {
    const s = this.sessions.find((x) => x.id === sessionId);
    if (!s) return;
    if (!s.closed) this.send({ type: 'close_session', sessionId });
    this.removeSession(sessionId);
  }

  sendText(sessionId: string, text: string) {
    const s = this.sessions.find((x) => x.id === sessionId);
    if (!s || s.closed || !text.trim()) return;
    const id = uid();
    const ok = this.send({ type: 'text', sessionId, msgId: id, text });
    s.items.push({ kind: 'text', id, mine: true, ts: Date.now(), text, failed: !ok });
  }

  sendFiles(sessionId: string, files: Iterable<File>) {
    const s = this.sessions.find((x) => x.id === sessionId);
    if (!s || s.closed) return;
    for (const file of files) {
      const id = uid();
      const name = file.name && file.name !== 'image.png' ? file.name : pastedName(file);
      const mime = file.type || 'application/octet-stream';
      this.files.set(id, file);
      this.fileSession.set(id, sessionId);
      const ok = this.send({ type: 'file_offer', sessionId, fileId: id, name, size: file.size, mime });
      s.items.push({
        kind: 'file',
        id,
        mine: true,
        ts: Date.now(),
        name,
        size: file.size,
        mime,
        previewUrl: mime.startsWith('image/') ? URL.createObjectURL(file) : undefined,
        delivered: 0,
        failed: !ok,
      });
    }
  }

  /** Received file: saves the inline preview if there is one, otherwise streams it from the sender. */
  download(item: FileItem) {
    triggerDownload(item.previewUrl ?? this.fileUrl(item.id), item.name);
  }

  toast(text: string, kind: Toast['kind'] = 'info') {
    const id = uid();
    this.toasts.push({ id, text, kind });
    setTimeout(() => (this.toasts = this.toasts.filter((t) => t.id !== id)), 4500);
  }

  // ---- protocol ---------------------------------------------------------------------------

  private findSession(id: string) {
    return this.sessions.find((s) => s.id === id);
  }

  private findFile(fileId: string): FileItem | undefined {
    const s = this.findSession(this.fileSession.get(fileId) ?? '');
    return s?.items.find((i): i is FileItem => i.kind === 'file' && i.id === fileId);
  }

  private addItem(s: ChatSession, item: ChatItem) {
    s.items.push(item);
    if (item.kind !== 'system' && (this.activeId !== s.id || document.hidden)) s.unread++;
    if (item.kind !== 'system' && document.hidden) {
      notify(displayName(s.peer), item.kind === 'text' ? item.text.slice(0, 120) : `sent ${item.name}`, s.id);
    }
  }

  private removeSession(id: string) {
    const s = this.findSession(id);
    if (!s) return;
    for (const item of s.items) {
      if (item.kind === 'file') {
        if (item.previewUrl) URL.revokeObjectURL(item.previewUrl);
        this.files.delete(item.id);
        this.fileSession.delete(item.id);
      }
    }
    this.sessions = this.sessions.filter((x) => x.id !== id);
    if (this.activeId === id) this.activeId = this.sessions.at(-1)?.id ?? null;
  }

  private endSession(s: ChatSession, text: string) {
    if (s.closed) return;
    s.closed = text;
    s.peerOnline = false;
    s.items.push({ kind: 'system', id: uid(), ts: Date.now(), text });
    // Files can no longer be requested; keep the references only for what is already shown.
    for (const item of s.items) if (item.kind === 'file' && item.mine) this.files.delete(item.id);
  }

  private handle(m: ServerMsg) {
    switch (m.type) {
      case 'welcome': {
        this.me = { id: m.clientId, ip: m.ip };
        this.inviteTimeoutSecs = m.inviteTimeoutSecs;
        this.token = m.token;
        sessionStorage.setItem('nds.clientId', m.clientId);
        sessionStorage.setItem('nds.token', m.token);
        this.status = 'online';
        this.incoming = [];
        this.outgoing = {};
        if (m.resumed) {
          this.resync = new Set(this.sessions.filter((s) => !s.closed).map((s) => s.id));
        } else {
          for (const s of this.sessions) this.endSession(s, 'Connection to the server was lost; session ended.');
        }
        break;
      }

      case 'clients': {
        this.clients = m.clients;
        for (const s of this.sessions) {
          const fresh = m.clients.find((c: ClientInfo) => c.id === s.peer.id);
          if (fresh) s.peer = fresh;
        }
        // The server sends restored sessions before the first client list.
        if (this.resync) {
          for (const id of this.resync) {
            const s = this.findSession(id);
            if (s) this.endSession(s, 'Session ended while you were disconnected.');
          }
          this.resync = null;
        }
        break;
      }

      case 'invite_sent':
        this.outgoing[m.to] = { inviteId: m.inviteId, to: m.to, expiresAt: Date.now() + m.expiresInMs };
        break;

      case 'invite_received': {
        if (this.incoming.some((i) => i.inviteId === m.inviteId)) break;
        this.incoming.push({ inviteId: m.inviteId, from: m.from, expiresAt: Date.now() + m.expiresInMs });
        beep();
        notify('Share invite', `${displayName(m.from)} (${m.from.ip}) wants to share with you`, m.inviteId);
        break;
      }

      case 'invite_result': {
        delete this.outgoing[m.to];
        const peer = this.clients.find((c) => c.id === m.to);
        const who = peer ? displayName(peer) : 'The client';
        const texts: Record<string, string> = {
          denied: `${who} declined your invite.`,
          expired: `${who} did not answer your invite.`,
          unavailable: `${who} is no longer available.`,
        };
        if (texts[m.outcome]) this.toast(texts[m.outcome], 'error');
        break;
      }

      case 'invite_closed':
        this.incoming = this.incoming.filter((i) => i.inviteId !== m.inviteId);
        break;

      case 'session_started': {
        this.resync?.delete(m.sessionId);
        const existing = this.findSession(m.sessionId);
        if (existing) {
          existing.peer = m.peer;
          break;
        }
        this.sessions.push({ id: m.sessionId, peer: m.peer, peerOnline: true, closed: null, items: [], unread: 0 });
        const s = this.findSession(m.sessionId)!;
        s.items.push({ kind: 'system', id: uid(), ts: Date.now(), text: `Session with ${displayName(m.peer)} started.` });
        this.activate(m.sessionId);
        break;
      }

      case 'session_closed': {
        const s = this.findSession(m.sessionId);
        if (!s) break;
        if (m.byMe) {
          this.removeSession(m.sessionId);
        } else {
          const who = displayName(s.peer);
          this.endSession(s, m.reason === 'disconnected' ? `${who} disconnected.` : `${who} closed the session.`);
        }
        break;
      }

      case 'peer_status': {
        const s = this.findSession(m.sessionId);
        if (s && !s.closed) s.peerOnline = m.online;
        break;
      }

      case 'text': {
        const s = this.findSession(m.sessionId);
        if (s) this.addItem(s, { kind: 'text', id: m.msgId, mine: false, ts: m.ts, text: m.text });
        break;
      }

      case 'file_offer': {
        const s = this.findSession(m.sessionId);
        if (!s) break;
        this.fileSession.set(m.fileId, s.id);
        this.addItem(s, { kind: 'file', id: m.fileId, mine: false, ts: m.ts, name: m.name, size: m.size, mime: m.mime });
        if (m.mime.startsWith('image/') && m.size <= AUTO_PREVIEW_MAX) void this.loadPreview(m.fileId);
        break;
      }

      case 'delivery_failed': {
        const s = this.findSession(m.sessionId);
        const item = s?.items.find((i) => i.id === m.msgId);
        if (item && item.kind !== 'system') item.failed = true;
        break;
      }

      case 'upload_request':
        void this.upload(m.fileId, m.reqId);
        break;

      case 'transfer_started':
      case 'transfer_progress': {
        const item = this.findFile(m.fileId);
        if (item) item.transfer = { reqId: m.reqId, bytes: m.bytes ?? 0, total: m.total, state: 'running' };
        break;
      }

      case 'transfer_done': {
        const item = this.findFile(m.fileId);
        if (!item) break;
        if (item.transfer?.reqId === m.reqId) item.transfer = { reqId: m.reqId, bytes: item.size, total: item.size, state: 'done' };
        if (item.mine) item.delivered = (item.delivered ?? 0) + 1;
        break;
      }

      case 'transfer_failed': {
        const item = this.findFile(m.fileId);
        if (item) item.transfer = { reqId: m.reqId, bytes: item.transfer?.bytes ?? 0, total: item.size, state: 'failed', error: m.error };
        break;
      }

      case 'error':
        this.toast(m.message, 'error');
        break;
    }
  }

  private async upload(fileId: string, reqId: string) {
    const file = this.files.get(fileId);
    if (!file) {
      this.send({ type: 'file_unavailable', reqId });
      return;
    }
    try {
      await fetch(`/api/upload/${encodeURIComponent(reqId)}?token=${encodeURIComponent(this.token ?? '')}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/octet-stream' },
        body: file,
      });
    } catch {
      // The server reports the failure to both sides.
    }
  }

  private async loadPreview(fileId: string) {
    try {
      const res = await fetch(this.fileUrl(fileId));
      if (!res.ok) return;
      const blob = await res.blob();
      const item = this.findFile(fileId);
      if (item) item.previewUrl = URL.createObjectURL(blob);
    } catch {
      // Shown through transfer_failed.
    }
  }
}

function pastedName(file: File): string {
  const ext = (file.type.split('/')[1] || 'bin').replace('jpeg', 'jpg').replace(/\+.*/, '');
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  return `pasted-${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}.${ext}`;
}

export const app = new App();
