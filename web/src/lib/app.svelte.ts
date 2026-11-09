// Client state and the WebSocket protocol (see src/protocol.rs on the server).

import { beep, notify } from './notify';
import { deviceHint, triggerDownload, uid } from './util';

export type ClientInfo = { id: string; name: string | null; device: string; ip: string };

export type Transfer = {
  reqId: string;
  bytes: number;
  total: number;
  state: 'running' | 'done' | 'failed';
  error?: string;
};

// `key` is generated locally and only used for rendering, so ids chosen by the peer can never
// collide in a keyed list. `id` is the protocol id (message id or file id).
type ItemBase = { key: string; id: string; ts: number };
export type TextItem = ItemBase & { kind: 'text'; mine: boolean; text: string; failed?: boolean };
export type FileItem = ItemBase & {
  kind: 'file';
  mine: boolean;
  name: string;
  size: number;
  mime: string;
  previewUrl?: string;
  transfer?: Transfer;
  /** Number of completed transfers to the peer (sender side). */
  delivered?: number;
  failed?: boolean;
};
export type SystemItem = ItemBase & { kind: 'system'; text: string };
export type ChatItem = TextItem | FileItem | SystemItem;

export type ChatSession = {
  id: string;
  peer: ClientInfo;
  peerOnline: boolean;
  /** Set once the session has ended; the tab stays open read-only until the user closes it. */
  closed: string | null;
  items: ChatItem[];
  unread: number;
  /** Unsent composer text, kept while another tab is open. */
  draft: string;
};

export type IncomingInvite = { inviteId: string; from: ClientInfo; expiresAt: number };
export type OutgoingInvite = { inviteId: string; to: string; expiresAt: number };
export type Toast = { id: string; text: string; kind: 'info' | 'error' };

/** Received images up to this size are fetched automatically and shown inline. */
const AUTO_PREVIEW_MAX = 32 * 1024 * 1024;
/** Same limit as the server (src/ws.rs MAX_TEXT_LEN). Longer texts are sent as a .txt file. */
const MAX_TEXT_BYTES = 1 << 20;
const PING_INTERVAL = 20_000;
/** Without a pong for this long the connection is treated as dead. */
const PONG_TIMEOUT = 50_000;
const TICKET_TIMEOUT = 5_000;

/**
 * Image types that are shown inline. SVG is excluded on purpose: it can carry script, and a preview
 * opened in a new tab would run it in this origin.
 */
const PREVIEW_TYPES = new Set(['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/avif', 'image/bmp']);

export function isPreviewable(mime: string): boolean {
  return PREVIEW_TYPES.has(mime.toLowerCase());
}

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
  private lastPong = 0;
  /** Local files offered to peers, by file id. Only references; nothing is read until requested. */
  private files = new Map<string, File>();
  /** Which session a file belongs to, by file id. */
  private fileSession = new Map<string, string>();
  /** Sessions that must be confirmed by the server after a resumed connection. */
  private resync: Set<string> | null = null;
  /** Invites already announced with sound, so a reconnect does not alert again. */
  private seenInvites = new Set<string>();
  private ticketWaiters = new Map<string, ((ticket: string | null) => void)[]>();
  private tabs: BroadcastChannel | null = typeof BroadcastChannel === 'function' ? new BroadcastChannel('nds-tabs') : null;

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

  // ---- connection -------------------------------------------------------------------------

  async start() {
    // Answer other tabs that ask whether an identity is in use.
    this.tabs?.addEventListener('message', (e) => {
      if (e.data?.type === 'who' && this.me && e.data.clientId === this.me.id) {
        this.tabs?.postMessage({ type: 'mine', clientId: this.me.id });
      }
    });
    // A duplicated tab inherits sessionStorage. If another tab still uses that identity, start fresh
    // instead of taking it over.
    const stored = sessionStorage.getItem('nds.clientId');
    if (stored && this.tabs && (await this.identityInUse(stored))) {
      sessionStorage.removeItem('nds.clientId');
      sessionStorage.removeItem('nds.token');
      this.token = null;
    }
    this.connect();
  }

  private identityInUse(clientId: string): Promise<boolean> {
    return new Promise((resolve) => {
      const onMessage = (e: MessageEvent) => {
        if (e.data?.type === 'mine' && e.data.clientId === clientId) finish(true);
      };
      const finish = (inUse: boolean) => {
        clearTimeout(timer);
        this.tabs!.removeEventListener('message', onMessage);
        resolve(inUse);
      };
      const timer = setTimeout(() => finish(false), 250);
      this.tabs!.addEventListener('message', onMessage);
      this.tabs!.postMessage({ type: 'who', clientId });
    });
  }

  private connect() {
    this.status = 'connecting';
    const proto = location.protocol === 'https:' ? 'wss' : 'ws';
    const ws = new WebSocket(`${proto}://${location.host}/ws`);
    this.ws = ws;

    ws.onopen = () => {
      this.retryDelay = 500;
      this.lastPong = Date.now();
      this.send({
        type: 'hello',
        clientId: sessionStorage.getItem('nds.clientId'),
        token: this.token,
        name: this.name || null,
        device: this.device,
      });
      clearInterval(this.pingTimer);
      this.pingTimer = window.setInterval(() => {
        if (Date.now() - this.lastPong > PONG_TIMEOUT) this.dropConnection(ws);
        else this.send({ type: 'ping' });
      }, PING_INTERVAL);
    };
    ws.onmessage = (e) => {
      try {
        this.handle(JSON.parse(e.data));
      } catch (err) {
        console.error('bad message', err);
      }
    };
    ws.onclose = () => this.dropConnection(ws);
  }

  /** Handles a closed or dead connection and schedules a reconnect. */
  private dropConnection(ws: WebSocket) {
    if (this.ws !== ws) return;
    ws.onclose = ws.onmessage = ws.onopen = null;
    // On a half-open link close() would wait a long time for the close event, so do not rely on it.
    ws.close();
    clearInterval(this.pingTimer);
    this.status = 'offline';
    this.ws = null;
    for (const s of this.sessions) s.peerOnline = false;
    for (const [, waiters] of this.ticketWaiters) for (const w of waiters) w(null);
    this.ticketWaiters.clear();
    setTimeout(() => this.connect(), this.retryDelay);
    this.retryDelay = Math.min(this.retryDelay * 2, 5000);
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
    const s = this.findSession(sessionId);
    if (!s) return;
    if (!s.closed) this.send({ type: 'close_session', sessionId });
    this.removeSession(sessionId);
  }

  sendText(sessionId: string, text: string) {
    const s = this.findSession(sessionId);
    if (!s || s.closed || !text.trim()) return;
    if (new Blob([text]).size > MAX_TEXT_BYTES) {
      const file = new File([text], `${timestampName('text')}.txt`, { type: 'text/plain' });
      this.sendFiles(sessionId, [file]);
      this.toast('The text is too long for a message and was sent as a file.');
      return;
    }
    const id = uid();
    const ok = this.send({ type: 'text', sessionId, msgId: id, text });
    s.items.push({ kind: 'text', key: uid(), id, mine: true, ts: Date.now(), text, failed: !ok });
  }

  sendFiles(sessionId: string, files: Iterable<File>) {
    const s = this.findSession(sessionId);
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
        key: uid(),
        id,
        mine: true,
        ts: Date.now(),
        name,
        size: file.size,
        mime,
        previewUrl: isPreviewable(mime) ? URL.createObjectURL(file) : undefined,
        delivered: 0,
        failed: !ok,
      });
    }
  }

  /** Received file: saves the inline preview if there is one, otherwise streams it from the sender. */
  async download(item: FileItem) {
    if (item.previewUrl) return triggerDownload(item.previewUrl, item.name);
    const ticket = await this.requestTicket(item.id);
    if (!ticket) return this.toast('The file is no longer available.', 'error');
    triggerDownload(this.fileUrl(item.id, ticket), item.name);
  }

  toast(text: string, kind: Toast['kind'] = 'info') {
    const id = uid();
    this.toasts.push({ id, text, kind });
    setTimeout(() => (this.toasts = this.toasts.filter((t) => t.id !== id)), 4500);
  }

  // ---- protocol ---------------------------------------------------------------------------

  private fileUrl(fileId: string, ticket: string): string {
    return `/api/files/${encodeURIComponent(fileId)}?ticket=${encodeURIComponent(ticket)}`;
  }

  /** Gets a single-use download ticket, so the long-lived token never shows up in a URL or history. */
  private requestTicket(fileId: string): Promise<string | null> {
    return new Promise((resolve) => {
      const waiters = this.ticketWaiters.get(fileId) ?? [];
      let done = false;
      const finish = (ticket: string | null) => {
        if (done) return;
        done = true;
        clearTimeout(timer);
        resolve(ticket);
      };
      const timer = setTimeout(() => {
        const list = this.ticketWaiters.get(fileId);
        if (list) this.ticketWaiters.set(fileId, list.filter((w) => w !== finish));
        finish(null);
      }, TICKET_TIMEOUT);
      this.ticketWaiters.set(fileId, [...waiters, finish]);
      if (!this.send({ type: 'request_download', fileId })) finish(null);
    });
  }

  private findSession(id: string) {
    return this.sessions.find((s) => s.id === id);
  }

  private findFile(fileId: string): FileItem | undefined {
    const s = this.findSession(this.fileSession.get(fileId) ?? '');
    return s?.items.find((i): i is FileItem => i.kind === 'file' && i.id === fileId);
  }

  private system(s: ChatSession, text: string) {
    s.items.push({ kind: 'system', key: uid(), id: '', ts: Date.now(), text });
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
    this.system(s, text);
    // Files can no longer be requested; the previews stay until the tab is closed.
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

      case 'pong':
        this.lastPong = Date.now();
        break;

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
        if (!this.seenInvites.has(m.inviteId)) {
          this.seenInvites.add(m.inviteId);
          beep();
          notify('Share invite', `${displayName(m.from)} (${m.from.ip}) wants to share with you`, m.inviteId);
        }
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
        this.sessions.push({ id: m.sessionId, peer: m.peer, peerOnline: true, closed: null, items: [], unread: 0, draft: '' });
        const s = this.findSession(m.sessionId)!;
        this.system(s, `Session with ${displayName(m.peer)} started.`);
        // The invitee just clicked Accept, so switch. The inviter may be busy in another chat.
        if (!m.initiator || this.activeId === null) this.activate(m.sessionId);
        else this.toast(`${displayName(m.peer)} accepted your invite.`);
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
        if (s) this.addItem(s, { kind: 'text', key: uid(), id: m.msgId, mine: false, ts: m.ts, text: m.text });
        break;
      }

      case 'file_offer': {
        const s = this.findSession(m.sessionId);
        if (!s) break;
        this.fileSession.set(m.fileId, s.id);
        this.addItem(s, {
          kind: 'file',
          key: uid(),
          id: m.fileId,
          mine: false,
          ts: m.ts,
          name: m.name,
          size: m.size,
          mime: m.mime,
        });
        if (isPreviewable(m.mime) && m.size <= AUTO_PREVIEW_MAX) void this.loadPreview(m.fileId, m.mime);
        break;
      }

      case 'delivery_failed': {
        const s = this.findSession(m.sessionId);
        const item = s?.items.find((i) => i.kind !== 'system' && i.mine && i.id === m.msgId);
        if (item && item.kind !== 'system') item.failed = true;
        break;
      }

      case 'download_ticket': {
        const waiters = this.ticketWaiters.get(m.fileId);
        const next = waiters?.shift();
        if (waiters && waiters.length === 0) this.ticketWaiters.delete(m.fileId);
        next?.(m.ticket);
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
      await fetch(`/api/upload/${encodeURIComponent(reqId)}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/octet-stream', 'X-NDS-Token': this.token ?? '' },
        body: file,
      });
    } catch {
      // The server reports the failure to both sides.
    }
  }

  private async loadPreview(fileId: string, mime: string) {
    try {
      const ticket = await this.requestTicket(fileId);
      if (!ticket) return;
      const res = await fetch(this.fileUrl(fileId, ticket));
      if (!res.ok) return;
      // Rebuild the blob with the checked type, never the one the response claims.
      const blob = new Blob([await res.blob()], { type: mime.toLowerCase() });
      const item = this.findFile(fileId);
      if (item) item.previewUrl = URL.createObjectURL(blob);
    } catch {
      // Shown through transfer_failed.
    }
  }
}

function timestampName(prefix: string): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  return `${prefix}-${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
}

function pastedName(file: File): string {
  const ext = (file.type.split('/')[1] || 'bin').replace('jpeg', 'jpg').replace(/\+.*/, '');
  return `${timestampName('pasted')}.${ext}`;
}

export const app = new App();
