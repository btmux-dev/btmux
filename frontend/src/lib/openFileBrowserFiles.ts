import { findPaneState, nativeEditorOver, useStore } from '../state/store';

/**
 * Open file mode; if an editor is shown over the pane (native or the built-in
 * Neovim) or the pane runs Neovim, start in the current file's directory focused on it.
 */
export async function openFileBrowserFiles(paneId: string | null, cwd: string | null) {
  let dir = cwd;
  let focus: string | null = null;
  const state = useStore.getState();
  const { nvimOpen, nvimPaneId } = state;
  const builtin = nvimOpen && paneId !== null && nvimPaneId === paneId;
  const native = nativeEditorOver(state, paneId) ? findPaneState(state, paneId)?.editor.active : null;
  if (native) {
    const slash = native.lastIndexOf('/');
    dir = native.slice(0, slash) || '/';
    focus = native.slice(slash + 1);
  } else if (paneId) {
    try {
      const url = builtin ? '/api/nvim/file' : `/api/panes/${encodeURIComponent(paneId)}/editor-file`;
      const res = await fetch(url, { cache: 'no-store' });
      const path: string | null = res.ok ? ((await res.json()) as { path: string | null }).path : null;
      const slash = path ? path.lastIndexOf('/') : -1;
      if (path && slash >= 0) {
        dir = path.slice(0, slash) || '/';
        focus = path.slice(slash + 1);
      }
    } catch {
      // fall back to the pane cwd
    }
  }
  useStore.getState().setFileBrowserOpen(true, dir, paneId, 'files', focus);
}
