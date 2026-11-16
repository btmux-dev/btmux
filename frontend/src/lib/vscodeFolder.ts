import type { PaneState, SessionState } from '../state/types';

type FolderPane = Pick<PaneState, 'cwd' | 'cwd_remote' | 'git'>;

/**
 * The folder an editor opens for a pane: its worktree's root, else its cwd,
 * unless that's on another host. Mirrors `editor_folder` in manager.rs.
 */
export function workspaceFolder(pane: FolderPane | null | undefined): string | null {
  if (!pane) return null;
  return pane.git?.worktree_root ?? (pane.cwd_remote ? null : pane.cwd);
}

/** The folder VS Code shows once opened over `pane`: the pane's, else the one it shows. */
export function vscodeFolderFor(pane: FolderPane | null | undefined, shown: string | null): string | null {
  return workspaceFolder(pane) ?? shown;
}

/** The VS Code page for `folder`. */
export function vscodeUrl(folder: string | null): string {
  return `/vscode/${folder ? `?folder=${encodeURIComponent(folder)}` : ''}`;
}

/** The folder in a VS Code page's address, which VS Code updates when it opens another. */
export function urlFolder(href: string): string | null {
  try {
    return new URL(href).searchParams.get('folder');
  } catch {
    return null;
  }
}

export function findPane(sessions: SessionState[], paneId: string | null): PaneState | undefined {
  return sessions.flatMap((s) => s.windows.flatMap((w) => w.panes)).find((p) => p.id === paneId);
}

export function findPaneSession(sessions: SessionState[], paneId: string | null): SessionState | undefined {
  return sessions.find((s) => s.windows.some((w) => w.panes.some((p) => p.id === paneId)));
}
