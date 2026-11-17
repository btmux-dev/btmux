/** The native editor's Git commands, after gitsigns.nvim. */
export const GIT_COMMANDS = [
  'editor-next-hunk',
  'editor-prev-hunk',
  'editor-preview-hunk',
  'editor-stage-hunk',
  'editor-reset-hunk',
  'editor-stage-buffer',
  'editor-reset-buffer',
  'editor-blame-line',
  'editor-toggle-line-blame',
];

/** Commands the command palette runs in a pane's native editor: quick open, language server, and Git commands. */
export const EDITOR_COMMANDS = [
  'editor-quick-open',
  'editor-code-actions',
  'editor-document-symbols',
  'editor-workspace-symbols',
  'editor-incoming-calls',
  'editor-go-to-declaration',
  'editor-go-to-implementation',
  'editor-go-to-type-definition',
  'editor-outgoing-calls',
  'editor-supertypes',
  'editor-subtypes',
  'editor-expand-selection',
  ...GIT_COMMANDS,
];

export const QUICK_OPEN_NEEDS_FOLDER = 'Quick open needs a folder on this machine';

/** How long a command waits for the editor to be shown. */
const QUEUE_MS = 5000;
/** How long a command waits for modal dialogs to close. */
const MODAL_WAIT_MS = 2000;

const panes = new Map<string, (action: string) => boolean>();
const queued = new Map<string, { action: string; failed: () => void; timer: ReturnType<typeof setTimeout> }>();

/**
 * Run `fn` once no modal dialog is open: the command palette is still open as
 * it runs a command, keeping focus from the editor, and restores focus as it
 * closes.
 */
function afterModals(fn: () => void, deadline = performance.now() + MODAL_WAIT_MS) {
  requestAnimationFrame(() => {
    if (document.querySelector('dialog:modal') && performance.now() < deadline) afterModals(fn, deadline);
    else fn();
  });
}

function runNow(paneId: string, action: string, failed: () => void) {
  const run = panes.get(paneId);
  if (!run || !run(action)) failed();
}

/** Run commands for the shown editor over `paneId`; returns the unregister function. */
export function registerEditorCommands(paneId: string, run: (action: string) => boolean): () => void {
  panes.set(paneId, run);
  const waiting = queued.get(paneId);
  if (waiting) {
    queued.delete(paneId);
    clearTimeout(waiting.timer);
    afterModals(() => runNow(paneId, waiting.action, waiting.failed));
  }
  return () => {
    if (panes.get(paneId) === run) panes.delete(paneId);
  };
}

/** Run `action` in the shown editor over `paneId` once modals close; `failed` if there is none or it fails. */
export function runEditorCommand(paneId: string, action: string, failed: () => void) {
  afterModals(() => runNow(paneId, action, failed));
}

/** Like `runEditorCommand`, but waits up to 5 s for the editor over `paneId` to be shown. */
export function runEditorCommandWhenShown(paneId: string, action: string, failed: () => void) {
  afterModals(() => {
    if (panes.has(paneId)) return runNow(paneId, action, failed);
    const previous = queued.get(paneId);
    if (previous) clearTimeout(previous.timer);
    const timer = setTimeout(() => {
      queued.delete(paneId);
      failed();
    }, QUEUE_MS);
    queued.set(paneId, { action, failed, timer });
  });
}
