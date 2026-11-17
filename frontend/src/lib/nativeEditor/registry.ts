/** Commands the command palette runs in a pane's native editor: quick open and language server commands. */
export const EDITOR_COMMANDS = [
  'editor-quick-open',
  'editor-code-actions',
  'editor-document-symbols',
  'editor-workspace-symbols',
  'editor-incoming-calls',
  'editor-outgoing-calls',
  'editor-supertypes',
  'editor-subtypes',
  'editor-expand-selection',
];

/** How long a command waits for the editor to be shown. */
const QUEUE_MS = 5000;

const panes = new Map<string, (action: string) => boolean>();
const queued = new Map<string, { action: string; failed: () => void; timer: ReturnType<typeof setTimeout> }>();

/** Run commands for the shown editor over `paneId`; returns the unregister function. */
export function registerEditorCommands(paneId: string, run: (action: string) => boolean): () => void {
  panes.set(paneId, run);
  const waiting = queued.get(paneId);
  if (waiting) {
    queued.delete(paneId);
    clearTimeout(waiting.timer);
    if (!run(waiting.action)) waiting.failed();
  }
  return () => {
    if (panes.get(paneId) === run) panes.delete(paneId);
  };
}

/** Run `action` in the editor over `paneId`; false if it has no file with a language server. */
export function runEditorCommand(paneId: string, action: string): boolean {
  return panes.get(paneId)?.(action) ?? false;
}

/** Run `action` in the editor over `paneId` once it is shown; `failed` if it isn't soon or the command fails. */
export function runEditorCommandWhenShown(paneId: string, action: string, failed: () => void) {
  const run = panes.get(paneId);
  if (run) {
    if (!run(action)) failed();
    return;
  }
  const previous = queued.get(paneId);
  if (previous) clearTimeout(previous.timer);
  const timer = setTimeout(() => {
    queued.delete(paneId);
    failed();
  }, QUEUE_MS);
  queued.set(paneId, { action, failed, timer });
}
