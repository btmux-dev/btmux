/** `editor_reveal` from the server, held until the pane's editor is ready for it. */
type Reveal = { path: string; line: number };

const pending = new Map<string, Reveal>();
const listeners = new Map<string, (reveal: Reveal) => void>();

export function revealInEditor(paneId: string, path: string, line: number) {
  const listener = listeners.get(paneId);
  if (listener) listener({ path, line });
  else pending.set(paneId, { path, line });
}

/** Receive reveals for `paneId`, starting with one that came before. */
export function onReveal(paneId: string, listener: (reveal: Reveal) => void): () => void {
  listeners.set(paneId, listener);
  const reveal = pending.get(paneId);
  pending.delete(paneId);
  if (reveal) listener(reveal);
  return () => {
    if (listeners.get(paneId) === listener) listeners.delete(paneId);
  };
}
