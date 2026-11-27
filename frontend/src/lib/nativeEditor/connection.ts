import type {
  Blame,
  BufferInfo,
  BufferSummary,
  DiskStatus,
  EditorClientMessage,
  EditorServerMessage,
  LspServerStatus,
  Update,
} from '../../generated/protocol';
import { onWake, PING, watchSocket } from '../liveness';

const RECONNECT_MS = 2000;

/** What a document following a buffer hears from the server. */
export interface BufferListener {
  readonly path: string;
  /** The buffer and version to resume from after a reconnect, if any. */
  resume(): { buffer: string; version: number } | null;
  opened(info: BufferInfo): void;
  resumed(): void;
  updates(version: number, updates: Update[]): void;
  bufferState(dirty: boolean, disk: DiskStatus): void;
  /** The file's Git index text (`null` outside Git), at `generation`. */
  gitBase(generation: number, text: string | null, untracked: boolean): void;
  failed(error: string): void;
  /** The socket dropped; unconfirmed pushes are lost. */
  disconnected(): void;
}

export interface SaveResult {
  error: string | null;
  conflict: boolean;
}

export type BlameResult = { blame: Blame | null; stale: boolean; error: string | null };

type Pending = {
  listener?: BufferListener;
  done?: (result: SaveResult) => void;
  blame?: (result: BlameResult) => void;
};

/** The tab's `/ws/editor` socket, shared by every editor view. */
class EditorConnection {
  private ws: WebSocket | null = null;
  private nextReq = 1;
  private pending = new Map<number, Pending>();
  private listeners = new Set<BufferListener>();
  /** Listeners by buffer id, once opened. */
  private byBuffer = new Map<string, Set<BufferListener>>();
  private bufferOf = new Map<BufferListener, string>();
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private summaryListeners = new Set<(buffers: BufferSummary[]) => void>();
  buffers: BufferSummary[] = [];
  private lspListeners = new Map<number, Set<(message: string) => void>>();
  private statusListeners = new Set<(servers: LspServerStatus[]) => void>();
  private connectionListeners = new Set<(open: boolean) => void>();
  lspStatus: LspServerStatus[] = [];
  /** Server requests for the user: prompts and documents to show. */
  serverUi:
    | ((msg: Extract<EditorServerMessage, { type: 'lsp_prompt' | 'lsp_prompt_done' | 'lsp_show_document' }>) => void)
    | null = null;

  private url() {
    return `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws/editor`;
  }

  private wakeWatched = false;

  private connect() {
    if (this.reconnectTimer !== null) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    if (this.ws || this.listeners.size === 0) return;
    if (!this.wakeWatched) {
      this.wakeWatched = true;
      // The network is back: retry now rather than after the delay.
      onWake(() => {
        if (this.reconnectTimer !== null) this.connect();
      });
    }
    const ws = new WebSocket(this.url());
    this.ws = ws;
    ws.onopen = () => {
      for (const listener of this.listeners) this.sendOpen(listener);
      for (const fn of this.connectionListeners) fn(true);
    };
    ws.onmessage = (event) => {
      if (typeof event.data === 'string') this.receive(JSON.parse(event.data) as EditorServerMessage);
    };
    const lost = () => {
      if (this.ws !== ws) return;
      this.ws = null;
      for (const p of this.pending.values()) {
        p.done?.({ error: 'The editor connection closed', conflict: false });
        p.blame?.({ blame: null, stale: false, error: 'The editor connection closed' });
      }
      this.pending.clear();
      this.byBuffer.clear();
      this.bufferOf.clear();
      for (const listener of this.listeners) listener.disconnected();
      for (const fn of this.connectionListeners) fn(false);
      if (this.listeners.size > 0) this.reconnectTimer = setTimeout(() => this.connect(), RECONNECT_MS);
    };
    ws.onclose = lost;
    watchSocket(ws, PING, () => {
      lost();
      ws.close();
      this.connect();
    });
  }

  private send(msg: EditorClientMessage): boolean {
    if (this.ws?.readyState !== WebSocket.OPEN) return false;
    this.ws.send(JSON.stringify(msg));
    return true;
  }

  private sendOpen(listener: BufferListener) {
    const req = this.nextReq++;
    this.pending.set(req, { listener });
    this.send({ type: 'open', req, path: listener.path, resume: listener.resume() });
  }

  private follow(listener: BufferListener, buffer: string) {
    this.bufferOf.set(listener, buffer);
    let set = this.byBuffer.get(buffer);
    if (!set) this.byBuffer.set(buffer, (set = new Set()));
    set.add(listener);
  }

  private receive(msg: EditorServerMessage) {
    switch (msg.type) {
      case 'opened':
      case 'resumed': {
        const pending = this.pending.get(msg.req);
        this.pending.delete(msg.req);
        const listener = pending?.listener;
        if (!listener || !this.listeners.has(listener)) {
          // Closed while opening.
          const buffer = msg.type === 'opened' ? msg.buffer.id : msg.buffer;
          if (!this.byBuffer.has(buffer)) this.send({ type: 'release', buffer });
          return;
        }
        if (msg.type === 'opened') {
          this.follow(listener, msg.buffer.id);
          listener.opened(msg.buffer);
        } else {
          this.follow(listener, msg.buffer);
          listener.resumed();
        }
        return;
      }
      case 'updates':
        for (const l of this.byBuffer.get(msg.buffer) ?? []) l.updates(msg.version, msg.updates);
        return;
      case 'buffer_state':
        for (const l of this.byBuffer.get(msg.buffer) ?? []) l.bufferState(msg.dirty, msg.disk);
        return;
      case 'git_base':
        for (const l of this.byBuffer.get(msg.buffer) ?? []) l.gitBase(msg.generation, msg.text, msg.untracked);
        return;
      case 'git_blame': {
        const pending = this.pending.get(msg.req);
        this.pending.delete(msg.req);
        pending?.blame?.({ blame: msg.blame, stale: msg.stale, error: msg.error });
        return;
      }
      case 'pong':
        return;
      case 'buffers':
        this.buffers = msg.buffers;
        for (const fn of this.summaryListeners) fn(msg.buffers);
        return;
      case 'lsp': {
        const text = JSON.stringify(msg.message);
        for (const fn of this.lspListeners.get(msg.server) ?? []) fn(text);
        return;
      }
      case 'lsp_prompt':
      case 'lsp_prompt_done':
      case 'lsp_show_document':
        this.serverUi?.(msg);
        return;
      case 'lsp_status':
        this.lspStatus = msg.servers;
        for (const fn of this.statusListeners) fn(msg.servers);
        return;
      case 'result': {
        const pending = this.pending.get(msg.req);
        this.pending.delete(msg.req);
        if (pending?.listener) {
          if (msg.error) pending.listener.failed(msg.error);
        } else pending?.done?.({ error: msg.error, conflict: msg.conflict });
        return;
      }
    }
  }

  /** Follow `listener.path`; `opened` or `resumed` follows. */
  open(listener: BufferListener) {
    this.listeners.add(listener);
    if (this.reconnectTimer === null && !this.ws) this.connect();
    else if (this.ws?.readyState === WebSocket.OPEN) this.sendOpen(listener);
  }

  /** Reopen after the server lost track (a gap in updates). */
  reopen(listener: BufferListener) {
    const buffer = this.bufferOf.get(listener);
    if (buffer) this.unfollow(listener, buffer);
    if (this.ws?.readyState === WebSocket.OPEN) this.sendOpen(listener);
  }

  private unfollow(listener: BufferListener, buffer: string) {
    this.bufferOf.delete(listener);
    const set = this.byBuffer.get(buffer);
    set?.delete(listener);
    if (set?.size === 0) {
      this.byBuffer.delete(buffer);
      return true;
    }
    return false;
  }

  close(listener: BufferListener) {
    this.listeners.delete(listener);
    const buffer = this.bufferOf.get(listener);
    if (buffer && this.unfollow(listener, buffer)) this.send({ type: 'release', buffer });
    if (this.listeners.size === 0 && this.summaryListeners.size === 0) this.shutdown();
  }

  private shutdown() {
    if (this.reconnectTimer !== null) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    const ws = this.ws;
    this.ws = null;
    ws?.close();
  }

  push(buffer: string, version: number, updates: Update[]): boolean {
    return this.send({ type: 'push', buffer, version, updates });
  }

  private request(build: (req: number) => EditorClientMessage): Promise<SaveResult> {
    return new Promise((resolve) => {
      const req = this.nextReq++;
      if (!this.send(build(req))) {
        resolve({ error: 'The editor is not connected', conflict: false });
        return;
      }
      this.pending.set(req, { done: resolve });
    });
  }

  get connected() {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  /** A JSON-RPC message for language server `server`; requests carry the document version their positions use. */
  sendLsp(server: number, message: unknown, buffer: string | null, version: number | null): boolean {
    return this.send({ type: 'lsp', server, message, buffer, version });
  }

  onLsp(server: number, fn: (message: string) => void): () => void {
    let set = this.lspListeners.get(server);
    if (!set) this.lspListeners.set(server, (set = new Set()));
    set.add(fn);
    return () => set.delete(fn);
  }

  /** Called with true each time the socket opens, false when it closes. */
  onConnection(fn: (open: boolean) => void): () => void {
    this.connectionListeners.add(fn);
    return () => this.connectionListeners.delete(fn);
  }

  onLspStatus(fn: (servers: LspServerStatus[]) => void): () => void {
    this.statusListeners.add(fn);
    fn(this.lspStatus);
    return () => this.statusListeners.delete(fn);
  }

  answerPrompt(server: number, token: number, action: number | null) {
    this.send({ type: 'lsp_prompt_answer', server, token, action });
  }

  applyEdit(edit: unknown, server: number | null, buffer: string | null, version: number | null) {
    return this.request((req) => ({ type: 'apply_edit', req, edit, server, buffer, version }));
  }

  save(buffer: string, force: boolean) {
    return this.request((req) => ({ type: 'save', req, buffer, force }));
  }

  revert(buffer: string) {
    return this.request((req) => ({ type: 'revert', req, buffer }));
  }

  /** Make `text` the file's index content, if the index is still at `generation`. */
  gitStage(buffer: string, generation: number, text: string) {
    return this.request((req) => ({ type: 'git_stage', req, buffer, generation, text }));
  }

  /** Blame 1-based `line` of the buffer at `version`. */
  gitBlame(buffer: string, version: number, line: number, full: boolean): Promise<BlameResult> {
    return new Promise((resolve) => {
      const req = this.nextReq++;
      if (!this.send({ type: 'git_blame', req, buffer, version, line, full })) {
        resolve({ blame: null, stale: false, error: 'The editor is not connected' });
        return;
      }
      this.pending.set(req, { blame: resolve });
    });
  }

  /** Every buffer on the server, while subscribed. */
  subscribeBuffers(fn: (buffers: BufferSummary[]) => void): () => void {
    this.summaryListeners.add(fn);
    fn(this.buffers);
    return () => {
      this.summaryListeners.delete(fn);
      if (this.listeners.size === 0 && this.summaryListeners.size === 0) this.shutdown();
    };
  }
}

export const editorConnection = new EditorConnection();
