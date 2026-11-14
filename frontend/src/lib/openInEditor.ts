import type { ClientMessage } from '../protocol/messages';
import { useStore } from '../state/store';
import type { ClientConfig } from '../state/types';

/** Open `path` (or just the editor) over the pane, in `editor`: VS Code, the built-in Neovim, or the pane's own. */
export function openInEditor(
  send: (msg: ClientMessage) => void,
  paneId: string,
  path: string | null,
  line: number | null,
  editor: ClientConfig['editor'] | undefined = useStore.getState().config?.editor,
): void {
  const store = useStore.getState();
  if (editor === 'vscode') {
    if (path) send({ type: 'vscode_open', path, line });
    store.setVscodeOpen(true, paneId);
  } else if (editor !== 'pane') {
    if (path) send({ type: 'nvim_open', path, line });
    store.setNvimOpen(true, paneId);
  } else {
    store.hideEditorSurfaces(paneId);
    send({ type: 'open_file', pane_id: paneId, path, line });
  }
}
