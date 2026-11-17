import type { ClientMessage } from '../protocol/messages';
import { useStore } from '../state/store';
import type { ClientConfig } from '../state/types';
import { findPane, vscodeFolderFor } from './vscodeFolder';

/** Open `path` (or just the editor) over the pane, in `editor`: the native editor, VS Code, the built-in Neovim, or the pane's own. */
export function openInEditor(
  send: (msg: ClientMessage) => void,
  paneId: string,
  path: string | null,
  line: number | null,
  editor: ClientConfig['editor'] | undefined = useStore.getState().config?.editor,
): void {
  const store = useStore.getState();
  if (editor === 'native') {
    send({
      type: 'editor_view',
      pane_id: paneId,
      op: path ? { kind: 'open', path, line } : { kind: 'show' },
    });
    // This tab's other surfaces over the pane would hide it.
    store.hideEditorSurfaces(paneId);
  } else if (editor === 'vscode') {
    // Only to the windows showing the folder the surface will show.
    const folder = vscodeFolderFor(findPane(store.allSessions, paneId), store.vscodeFolder);
    if (path) send({ type: 'vscode_open', path, line, folder });
    store.setVscodeOpen(true, paneId);
  } else if (editor === 'neovim') {
    if (path) send({ type: 'nvim_open', path, line });
    store.setNvimOpen(true, paneId);
  } else {
    store.hideEditorSurfaces(paneId);
    send({ type: 'open_file', pane_id: paneId, path, line });
  }
}
