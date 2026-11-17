// Quick open: a fuzzy file picker over the pane's folder, searched by
// `/ws/files`' `search_files`.
import type { EditorView } from '@codemirror/view';
import { nextId, type FileSearchResult } from '../../protocol/file-messages';
import { useStore } from '../../state/store';
import { openPicker, type PickItem } from './picker';

/** Score bonuses over the server's order (nucleo gives about 16 per matched character). */
const OPEN_BOOST = 16;
const RECENT_BOOST = 8;
const MAX_RECENT = 50;
const RECENT_KEY = 'btmux-editor-recent';
const IDLE_CLOSE_MS = 60_000;

let recent: string[] | null = null;

function recentFiles(): string[] {
  if (!recent) {
    try {
      const saved = JSON.parse(localStorage.getItem(RECENT_KEY) ?? '[]');
      recent = Array.isArray(saved) ? saved.filter((p): p is string => typeof p === 'string') : [];
    } catch {
      recent = [];
    }
  }
  return recent;
}

/** Note `path` as shown in an editor, for quick open's recent files. */
export function noteOpened(path: string) {
  const list = recentFiles();
  if (list[0] === path) return;
  recent = [path, ...list.filter((p) => p !== path)].slice(0, MAX_RECENT);
  try {
    localStorage.setItem(RECENT_KEY, JSON.stringify(recent));
  } catch {}
}

/** A `/ws/files` socket for quick open, closed once idle. */
class FilesClient {
  private ws: WebSocket | null = null;
  private pending = new Map<string, { resolve: (payload: unknown) => void; reject: (error: Error) => void }>();
  private idle: ReturnType<typeof setTimeout> | undefined;

  request<T>(type: string, payload: Record<string, unknown>): Promise<T> {
    clearTimeout(this.idle);
    const ws = this.socket();
    const id = nextId();
    const message = JSON.stringify({ id, type, payload });
    if (ws.readyState === WebSocket.OPEN) ws.send(message);
    else ws.addEventListener('open', () => ws.send(message), { once: true });
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (payload: unknown) => void, reject });
    });
  }

  private socket(): WebSocket {
    if (this.ws && this.ws.readyState <= WebSocket.OPEN) return this.ws;
    const ws = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws/files`);
    this.ws = ws;
    ws.onmessage = (event) => {
      let msg: { id?: string; type?: string; payload?: { message?: string } };
      try {
        msg = JSON.parse(event.data);
      } catch {
        return;
      }
      const pending = msg.id ? this.pending.get(msg.id) : undefined;
      if (!pending) return;
      this.pending.delete(msg.id!);
      if (msg.type === 'error') pending.reject(new Error(msg.payload?.message ?? 'Request failed'));
      else pending.resolve(msg.payload);
      if (this.pending.size === 0) this.idle = setTimeout(() => ws.close(), IDLE_CLOSE_MS);
    };
    ws.onclose = () => {
      if (this.ws === ws) this.ws = null;
      for (const pending of this.pending.values()) pending.reject(new Error('Connection closed'));
      this.pending.clear();
    };
    return ws;
  }
}

const files = new FilesClient();

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

export interface QuickOpenPane {
  /** The folder searched. */
  root: string;
  /** The pane's open files, and the one shown. */
  files: string[];
  active: string | null;
}

/** Show quick open in `view`; `open` opens the chosen file in the pane. */
export function quickOpen(view: EditorView, pane: QuickOpenPane, open: (path: string) => void) {
  const prefix = pane.root.endsWith('/') ? pane.root : `${pane.root}/`;
  const item = (path: string, matches?: number[]): PickItem => ({
    ...describe(path, prefix, matches),
    run: () => open(path),
  });
  openPicker(view, {
    title: 'Go to file',
    query: async (text) => {
      const recent = recentFiles();
      if (!text.trim()) {
        const opened = pane.files.filter((p) => p !== pane.active);
        const others = recent.filter((p) => p !== pane.active && !pane.files.includes(p) && p.startsWith(prefix));
        return [...opened, ...others].map((path) => item(path));
      }
      let results: FileSearchResult[];
      try {
        ({ results } = await files.request<{ results: FileSearchResult[] }>('search_files', {
          root: pane.root,
          path: '.',
          query: text,
        }));
      } catch (error) {
        useStore.getState().showToast('File search failed', 'error', {
          body: error instanceof Error ? error.message : String(error),
        });
        return [];
      }
      const boost = (path: string) =>
        pane.files.includes(path) ? OPEN_BOOST : recent.includes(path) ? RECENT_BOOST : 0;
      return results
        .map((result, order) => ({ result, order, score: result.score + boost(result.path) }))
        .sort((a, b) => b.score - a.score || a.order - b.order)
        .map(({ result }) => item(result.path, result.indices));
    },
  });
}
