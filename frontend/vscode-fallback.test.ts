import { expect, test } from 'bun:test';
import { vscodeRequestEditor } from './src/lib/vscodeFallback';

test('local tabs always show VS Code', () => {
  expect(vscodeRequestEditor({ relayed: false, editor: 'vscode' }, false, false)).toBe('vscode');
  expect(vscodeRequestEditor(null, false, true)).toBe('vscode');
});

test('relayed tabs open a request that followed the editor setting in their fallback', () => {
  // `relay_event` rewrote `editor = "vscode"` to the fallback.
  expect(vscodeRequestEditor({ relayed: true, editor: 'neovim' }, false, false)).toBe('neovim');
  expect(vscodeRequestEditor({ relayed: true, editor: 'pane' }, false, true)).toBe('pane');
  // Every tab gets the request; only the focused one types into the pane.
  expect(vscodeRequestEditor({ relayed: true, editor: 'pane' }, false, false)).toBeNull();
});

test('relayed tabs keep a request that named VS Code, which explains why it cannot open', () => {
  expect(vscodeRequestEditor({ relayed: true, editor: 'neovim' }, true, true)).toBe('vscode');
});
