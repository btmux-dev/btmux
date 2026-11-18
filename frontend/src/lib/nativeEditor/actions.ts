// The native editor's named commands: palette actions, `[keys]` and vim mappings.
import type { EditorView } from '@codemirror/view';
import { findReferences, jumpToDefinition, LSPPlugin } from '@codemirror/lsp-client';
import { foldAll, unfoldAll } from '@codemirror/language';
import { forEachDiagnostic, openLintPanel } from '@codemirror/lint';
import { useStore } from '../../state/store';
import { canFormatRange, editorCommands as lspCommands, formatSelection } from './features';
import { formatCommand, signatureHelpCommand, startRename } from './lsp';
import { showDiagnostics, showHover } from './hover';
import { gitCommands } from './gitSigns';

type Which = 'next' | 'prev' | 'first' | 'last';

/**
 * Move the cursor to a diagnostic's start, wrapping, and show it. Unlike
 * lint's commands, it doesn't select it, which would enter visual mode.
 */
export function toDiagnostic(view: EditorView, which: Which): boolean {
  const head = view.state.selection.main.head;
  const starts: number[] = [];
  forEachDiagnostic(view.state, (_, from) => void starts.push(from));
  starts.sort((a, b) => a - b);
  if (!starts.length) {
    useStore.getState().showToast('No problems in this file', 'info');
    return true;
  }
  const last = starts[starts.length - 1];
  const target =
    which === 'first'
      ? starts[0]
      : which === 'last'
        ? last
        : which === 'next'
          ? (starts.find((from) => from > head) ?? starts[0])
          : ([...starts].reverse().find((from) => from < head) ?? last);
  view.dispatch({ selection: { anchor: target }, scrollIntoView: true, userEvent: 'select.jump' });
  showDiagnostics(view);
  return true;
}

const needsServer = (run: (view: EditorView) => boolean) => (view: EditorView) => !!LSPPlugin.get(view) && run(view);

/** Format the selection if there is one and the server can, else the file. */
function format(view: EditorView) {
  if (!view.state.selection.main.empty && canFormatRange(view)) {
    void formatSelection(view);
    return true;
  }
  return formatCommand(view);
}

/** Commands by action name, run on the shown file. */
export const editorCommands: Record<string, (view: EditorView) => boolean | Promise<boolean>> = {
  ...lspCommands,
  'editor-go-to-definition': needsServer(jumpToDefinition),
  'editor-find-references': needsServer(findReferences),
  'editor-rename': startRename,
  'editor-hover': showHover,
  'editor-signature-help': needsServer(signatureHelpCommand),
  'editor-format': format,
  'editor-next-diagnostic': (view) => toDiagnostic(view, 'next'),
  'editor-prev-diagnostic': (view) => toDiagnostic(view, 'prev'),
  'editor-first-diagnostic': (view) => toDiagnostic(view, 'first'),
  'editor-last-diagnostic': (view) => toDiagnostic(view, 'last'),
  'editor-show-diagnostics': (view) => {
    if (!showDiagnostics(view)) useStore.getState().showToast('No problems on this line', 'info');
    return true;
  },
  'editor-problems': openLintPanel,
  'editor-fold-all': foldAll,
  'editor-unfold-all': unfoldAll,
  ...gitCommands,
};
