// Soft wrap (`[native-editor] soft-wrap`).
import { EditorView } from '@codemirror/view';

/** Wraps a widget's text as CodeMirror's `lineWrapping` wraps lines. */
export const WRAP = { whiteSpace: 'break-spaces', wordBreak: 'break-word', overflowWrap: 'anywhere' };

/** Whether the main cursor is in the scroller's visible part. */
export function cursorVisible(view: EditorView): boolean {
  const at = view.coordsAtPos(view.state.selection.main.head);
  if (!at) return false;
  const box = view.scrollDOM.getBoundingClientRect();
  const gutters = view.dom.querySelector('.cm-gutters')?.getBoundingClientRect().right ?? box.left;
  return at.top >= box.top && at.bottom <= box.bottom && at.left >= gutters && at.right <= box.right;
}
