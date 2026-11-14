import type { ClientConfig } from '../state/types';

/**
 * Which editor a tab opens for an `open_vscode` request. A relayed tab
 * (btmux connect) has no VS Code, so a request that only followed the `editor`
 * setting opens in the editor its relayed config falls back to; one that named
 * VS Code still goes to VS Code, which shows why it can't open. Every tab gets
 * the request, so only the focused one, where the command was typed, runs the
 * pane's editor; `null` means this tab does nothing.
 */
export function vscodeRequestEditor(
  config: Pick<ClientConfig, 'relayed' | 'editor'> | null,
  explicit: boolean,
  focused: boolean,
): 'vscode' | 'neovim' | 'pane' | null {
  if (!config?.relayed || explicit) return 'vscode';
  if (config.editor === 'neovim') return 'neovim';
  return focused ? 'pane' : null;
}
