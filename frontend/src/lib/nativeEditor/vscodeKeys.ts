// VS Code's keys that CodeMirror's keymaps don't have, or bind otherwise.
// Its `Mod-k` chords are in `sequences.ts`.
import { EditorSelection, type StateCommand } from '@codemirror/state';
import type { EditorView, KeyBinding } from '@codemirror/view';
import { toggleBlockComment } from '@codemirror/commands';
import { gotoLine, selectNextOccurrence, selectSelectionMatches } from '@codemirror/search';

/** A line above each cursor, indented like its line. */
export const insertLineAbove: StateCommand = ({ state, dispatch }) => {
  if (state.readOnly) return false;
  const changes = state.changeByRange((range) => {
    const line = state.doc.lineAt(range.head);
    const indent = /^\s*/.exec(line.text)![0];
    return {
      changes: { from: line.from, insert: indent + state.lineBreak },
      range: EditorSelection.cursor(line.from + indent.length),
    };
  });
  dispatch(state.update(changes, { scrollIntoView: true, userEvent: 'input' }));
  return true;
};

/** Select every occurrence of the selection, or of the word at the cursor. */
function selectAllOccurrences(view: EditorView) {
  if (view.state.selection.main.empty) selectNextOccurrence(view);
  return selectSelectionMatches(view);
}

/**
 * Ahead of the search keymap, whose `Mod-g` is find next off macOS (F3 and
 * Enter still are). `Mod-t`, which the browser keeps, only in the desktop
 * app (`Mod-w` is the editor's, `PaneEditor.common`).
 */
export function vscodeKeys(desktop: { workspaceSymbols: (view: EditorView) => boolean } | null) {
  const keys: KeyBinding[] = [
    { key: 'Ctrl-g', run: gotoLine, preventDefault: true },
    { key: 'Mod-Shift-Enter', run: insertLineAbove, preventDefault: true },
    { key: 'Mod-F2', run: selectAllOccurrences, preventDefault: true },
    // CodeMirror's is Ctrl-Shift-a on macOS.
    { mac: 'Shift-Alt-a', run: toggleBlockComment, preventDefault: true },
  ];
  if (desktop) keys.push({ key: 'Mod-t', run: desktop.workspaceSymbols, preventDefault: true });
  return keys;
}
