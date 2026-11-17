/** Language server commands the command palette runs in a pane's native editor. */
export const EDITOR_COMMANDS = [
  'editor-code-actions',
  'editor-document-symbols',
  'editor-workspace-symbols',
  'editor-incoming-calls',
  'editor-outgoing-calls',
  'editor-supertypes',
  'editor-subtypes',
  'editor-expand-selection',
];

const panes = new Map<string, (action: string) => boolean>();

/** Run commands for the editor over `paneId`; returns the unregister function. */
export function registerEditorCommands(paneId: string, run: (action: string) => boolean): () => void {
  panes.set(paneId, run);
  return () => {
    if (panes.get(paneId) === run) panes.delete(paneId);
  };
}

/** Run `action` in the editor over `paneId`; false if it has no file with a language server. */
export function runEditorCommand(paneId: string, action: string): boolean {
  return panes.get(paneId)?.(action) ?? false;
}
