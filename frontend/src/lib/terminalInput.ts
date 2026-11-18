// Whether each terminal hides what's typed (a password prompt), as its
// `/ws/pane` socket reports while the key overlay watches.

/** A server report: whether any input since the last one, or the mode now, was hidden (null: can't tell). */
export interface InputReport {
  /** Input frames the server had received. */
  inputs: number;
  hidden: boolean | null;
}

/** A pane or popup socket taking input. */
export interface InputSocket {
  /** Input frames sent. */
  sent: number;
  /** Recent reports, oldest first. */
  reports: InputReport[];
  closed: boolean;
  /** Ask the server to report, or stop (`watch_input`). */
  watch: (on: boolean) => void;
}

const MAX_REPORTS = 256;

const sockets = new Map<string, InputSocket>();
const listeners = new Set<() => void>();
let watching = false;

const changed = () => {
  for (const listener of listeners) listener();
};

export const terminalInput = {
  /** A socket that finished its replay and takes input. */
  attach(id: string, watch: InputSocket['watch']): InputSocket {
    const previous = sockets.get(id);
    if (previous) previous.closed = true;
    const socket: InputSocket = { sent: 0, reports: [], closed: false, watch };
    sockets.set(id, socket);
    if (watching) watch(true);
    changed();
    return socket;
  },
  detach(id: string, socket: InputSocket) {
    socket.closed = true;
    if (sockets.get(id) === socket) sockets.delete(id);
    changed();
  },
  sent(socket: InputSocket) {
    socket.sent += 1;
  },
  report(socket: InputSocket, report: InputReport) {
    socket.reports.push(report);
    if (socket.reports.length > MAX_REPORTS) socket.reports.splice(0, socket.reports.length - MAX_REPORTS);
    changed();
  },
  socket(id: string): InputSocket | null {
    return sockets.get(id) ?? null;
  },
  setWatching(on: boolean) {
    if (watching === on) return;
    watching = on;
    for (const socket of sockets.values()) {
      // Reports from an earlier watch are stale.
      socket.reports = [];
      socket.watch(on);
    }
  },
  subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
};
