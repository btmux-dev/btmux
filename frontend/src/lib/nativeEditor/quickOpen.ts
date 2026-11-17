// Quick open: a fuzzy file picker over the pane's folder, searched by
// `/ws/files`' `search_files`.
import type { EditorView } from '@codemirror/view';
import { nextId, searchFiles, type FileSearchResponse } from '../../protocol/file-messages';
import { useStore } from '../../state/store';
import { openPicker, type PickItem } from './picker';

/** Score bonuses on the server (nucleo gives about 16 per matched character). */
const OPEN_BOOST = 16;
const RECENT_BOOST = 8;
const MAX_RECENT = 50;
const RECENT_KEY = 'btmux-editor-recent';
const IDLE_CLOSE_MS = 60_000;
/** A search answered later, with nothing else heard meanwhile, means a dead socket. */
const SEARCH_TIMEOUT_MS = 15_000;
/** A fresh search waits for a whole listing. */
const FRESH_TIMEOUT_MS = 120_000;

/** Recent files when storage is unavailable. */
let memory: string[] = [];

/** Read on every use, as other tabs write it too. */
function recentFiles(): string[] {
  try {
    const saved = JSON.parse(localStorage.getItem(RECENT_KEY) ?? '[]');
    return Array.isArray(saved) ? saved.filter((p): p is string => typeof p === 'string') : [];
  } catch {
    return memory;
  }
}

/** Note `path` as viewed in a focused editor, for quick open's recent files. */
export function noteOpened(path: string) {
  const list = recentFiles();
  if (list[0] === path) return;
  memory = [path, ...list.filter((p) => p !== path)].slice(0, MAX_RECENT);
  try {
    localStorage.setItem(RECENT_KEY, JSON.stringify(memory));
  } catch {}
}

type Pending = {
  resolve: (payload: unknown) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
};

/** A `/ws/files` socket for quick open, closed once idle or silent. */
class FilesClient {
  private ws: WebSocket | null = null;
  private pending = new Map<string, Pending>();
  private idle: ReturnType<typeof setTimeout> | undefined;
  /** When the socket last heard from the server. */
  private heard = 0;

  request<T>(type: string, payload: Record<string, unknown>, timeoutMs = SEARCH_TIMEOUT_MS): Promise<T> {
    clearTimeout(this.idle);
    const ws = this.socket();
    const id = nextId();
    const message = JSON.stringify({ id, type, payload });
    if (ws.readyState === WebSocket.OPEN) ws.send(message);
    else ws.addEventListener('open', () => ws.send(message), { once: true });
    const sent = performance.now();
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        if (!this.pending.delete(id)) return;
        reject(new Error('File search timed out'));
        if (this.ws === ws && this.heard < sent) this.abandon(ws);
        else this.idleSoon(ws);
      }, timeoutMs);
      this.pending.set(id, { resolve: resolve as (payload: unknown) => void, reject, timer });
    });
  }

  private idleSoon(ws: WebSocket) {
    if (this.pending.size === 0) this.idle = setTimeout(() => ws.close(), IDLE_CLOSE_MS);
  }

  /** Fail what's pending and stop using `ws`, which close() may never report on. */
  private abandon(ws: WebSocket) {
    ws.onmessage = ws.onclose = null;
    ws.close();
    if (this.ws === ws) this.ws = null;
    clearTimeout(this.idle);
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Error('Connection closed'));
    }
    this.pending.clear();
  }

  private socket(): WebSocket {
    if (this.ws && this.ws.readyState <= WebSocket.OPEN) return this.ws;
    const ws = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws/files`);
    this.ws = ws;
    ws.onmessage = (event) => {
      this.heard = performance.now();
      let msg: { id?: string; type?: string; payload?: { message?: string } };
      try {
        msg = JSON.parse(event.data);
      } catch {
        return;
      }
      const pending = msg.id ? this.pending.get(msg.id) : undefined;
      if (!pending) return;
      this.pending.delete(msg.id!);
      clearTimeout(pending.timer);
      if (msg.type === 'error') pending.reject(new Error(msg.payload?.message ?? 'Request failed'));
      else pending.resolve(msg.payload);
      this.idleSoon(ws);
    };
    ws.onclose = () => this.abandon(ws);
    return ws;
  }
}

const files = new FilesClient();

const prefixOf = (root: string) => (root.endsWith('/') ? root : `${root}/`);

/** Label (file name) and detail (directory) for `path`, with `matches` (code points of `path`) split between them. */
function describe(path: string, prefix: string, matches: number[] = []) {
  const under = path.startsWith(prefix);
  const relative = Array.from(under ? path.slice(prefix.length) : path);
  const shift = under ? Array.from(prefix).length : 0;
  const slash = relative.lastIndexOf('/');
  const at = matches.map((i) => i - shift);
  return {
    label: relative.slice(slash + 1).join(''),
    labelMatches: at.filter((i) => i > slash).map((i) => i - slash - 1),
    detail: slash > 0 ? relative.slice(0, slash).join('') : undefined,
    detailMatches: at.filter((i) => i >= 0 && i < slash),
  };
}

/** Split `file:line` (or `file:line:column`, whose column is ignored) into its query and line. */
export function parseQuery(text: string): { query: string; line: number | null } {
  const match = /:(\d*)(?::\d*)?$/.exec(text);
  if (!match) return { query: text, line: null };
  const line = Number.parseInt(match[1], 10);
  return { query: text.slice(0, match.index), line: line > 0 ? line : null };
}

export interface QuickOpenPane {
  /** The folder searched. */
  root: string;
  /** The pane's open files, and the one shown. */
  files: string[];
  active: string | null;
}

/** Show quick open in `view`; `open` opens the chosen file in the pane, on `line` if given. */
export function quickOpen(view: EditorView, pane: QuickOpenPane, open: (path: string, line: number | null) => void) {
  const search = (payload: Record<string, unknown>) =>
    files.request<FileSearchResponse>('search_files', payload, payload.fresh ? FRESH_TIMEOUT_MS : SEARCH_TIMEOUT_MS);
  openPicker(view, {
    title: 'Go to file',
    query: async (typed, refresh) => {
      const { query: text, line } = parseQuery(typed);
      const item = (path: string, prefix: string, matches?: number[]): PickItem => {
        const described = describe(path, prefix, matches);
        return {
          ...described,
          label: line ? `${described.label}:${line}` : described.label,
          run: () => open(path, line),
        };
      };
      const recent = recentFiles();
      // `:line` alone goes to a line of the shown file.
      if (!text.trim() && line && pane.active) {
        const active = pane.active;
        const name = describe(active, '').label;
        return [{ label: `Go to line ${line}`, detail: name, run: () => open(active, line) }];
      }
      if (!text.trim()) {
        // Names the folder as editor paths do (symlinks resolved), and lists
        // it ahead of the first search.
        const root = await search({ root: pane.root, path: '.', query: '' }).then(
          (response) => response.root,
          () => pane.root,
        );
        const prefix = prefixOf(root);
        const opened = pane.files.filter((p) => p !== pane.active);
        const others = recent.filter((p) => p !== pane.active && !pane.files.includes(p) && p.startsWith(prefix));
        return [...opened, ...others].map((path) => item(path, prefix));
      }
      // Open files over recent ones.
      const boost = Object.fromEntries([
        ...recent.map((path) => [path, RECENT_BOOST]),
        ...pane.files.map((path) => [path, OPEN_BOOST]),
      ]);
      const items = (response: FileSearchResponse) =>
        response.results.map((result) => item(result.path, prefixOf(response.root), result.indices));
      try {
        const response = await searchFiles(search, { root: pane.root, path: '.', query: text, boost }, (fresh) =>
          refresh(fresh.then(items)),
        );
        return items(response);
      } catch (error) {
        useStore.getState().showToast('File search failed', 'error', {
          body: error instanceof Error ? error.message : String(error),
        });
        return [];
      }
    },
  });
}
