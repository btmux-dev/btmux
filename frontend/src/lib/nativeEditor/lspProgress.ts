import type { LspServerStatus } from '../../generated/protocol';
import { PROGRESS_DONE_MS, type Progress } from '../progress';

export interface LspProgress extends Progress {
  server: number;
}

/**
 * Cards for the servers' work in `lsp_status`, in the order it began; work that
 * ended stays a moment as finished, or failed if its server stopped.
 */
export function lspProgress(prev: LspProgress[], servers: LspServerStatus[], now: number): LspProgress[] {
  const live = new Map<string, LspProgress>();
  for (const server of servers)
    for (const p of server.progress) {
      const id = `${server.id}:${p.token}`;
      live.set(id, {
        id,
        server: server.id,
        source: server.name,
        title: p.title,
        text: p.message ?? '',
        percent: p.percent,
        status: 'running',
        expires: Infinity,
      });
    }
  const next: LspProgress[] = [];
  for (const p of prev) {
    const current = live.get(p.id);
    live.delete(p.id);
    if (current) next.push(current);
    else if (p.status === 'running') {
      const ready = servers.find((s) => s.id === p.server)?.state === 'ready';
      next.push({ ...p, status: ready ? 'success' : 'failed', text: '', expires: now + PROGRESS_DONE_MS });
    } else if (p.expires > now) next.push(p);
  }
  return [...next, ...live.values()];
}
