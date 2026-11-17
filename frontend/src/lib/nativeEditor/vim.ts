// Vim keybindings for the native editor (`[native-editor] vim`).
import { getCM, Vim, vim, type CodeMirror } from '@replit/codemirror-vim';
import { Prec, StateEffect, StateField, type Extension } from '@codemirror/state';
import { closeCompletion, completionStatus } from '@codemirror/autocomplete';
import { EditorView, ViewPlugin, showTooltip, type Tooltip } from '@codemirror/view';
import { jumpToDefinition, LSPPlugin } from '@codemirror/lsp-client';
import { forEachDiagnostic } from '@codemirror/lint';

/** What ex commands and the mode line reach in the editor showing a view. */
export interface VimHost {
  /** Save the shown file; resolves true once it is saved. */
  save(force: boolean): Promise<boolean>;
  hide(): void;
  /** The mode changed: NORMAL, INSERT, …, or null without vim. */
  mode(mode: string | null): void;
}

const hosts = new WeakMap<EditorView, VimHost>();

export const setVimHost = (view: EditorView, host: VimHost) => hosts.set(view, host);

const hostOf = (cm: CodeMirror) => hosts.get(cm.cm6);

// `:w!` parses as `w` with the argument `!`.
const bang = (params: { argString?: string }) => params.argString?.trim().startsWith('!') ?? false;

const saveAndHide = (cm: CodeMirror, params: { argString?: string }) => {
  const host = hostOf(cm);
  void host?.save(bang(params)).then((saved) => saved && host.hide());
};

// Global to vim, so defined once; each resolves the editor from its view.
Vim.defineEx('write', 'w', (cm, params) => void hostOf(cm)?.save(bang(params)));
Vim.defineEx('wq', 'wq', saveAndHide);
Vim.defineEx('xit', 'x', saveAndHide);
// Hiding keeps unsaved changes (the server holds them), so `:q` needs no `!`.
Vim.defineEx('quit', 'q', (cm) => hostOf(cm)?.hide());
Vim.defineEx('qall', 'qa', (cm) => hostOf(cm)?.hide());

Vim.defineAction('btmuxDefinition', (cm) => void jumpToDefinition(cm.cm6));
Vim.defineAction('btmuxHover', (cm) => void showHover(cm.cm6));
Vim.defineAction('btmuxNextDiagnostic', (cm) => toDiagnostic(cm.cm6, true));
Vim.defineAction('btmuxPrevDiagnostic', (cm) => toDiagnostic(cm.cm6, false));
Vim.mapCommand('gd', 'action', 'btmuxDefinition', {}, { context: 'normal' });
Vim.mapCommand('K', 'action', 'btmuxHover', {}, { context: 'normal' });
Vim.mapCommand(']d', 'action', 'btmuxNextDiagnostic', {}, { context: 'normal' });
Vim.mapCommand('[d', 'action', 'btmuxPrevDiagnostic', {}, { context: 'normal' });

/**
 * Move the cursor to the next or previous diagnostic's start, wrapping.
 * Unlike lint's commands, it doesn't select it, which would enter visual mode.
 */
function toDiagnostic(view: EditorView, forward: boolean) {
  const head = view.state.selection.main.head;
  const starts: number[] = [];
  forEachDiagnostic(view.state, (_, from) => void starts.push(from));
  starts.sort((a, b) => a - b);
  if (!starts.length) return;
  const target = forward
    ? (starts.find((from) => from > head) ?? starts[0])
    : ([...starts].reverse().find((from) => from < head) ?? starts[starts.length - 1]);
  view.dispatch({ selection: { anchor: target }, scrollIntoView: true });
}

type VimState = {
  insertMode?: boolean;
  visualMode?: boolean;
  insertModeReturn?: boolean;
  inputState?: {
    keyBuffer: string[];
    operator?: string | null;
    prefixRepeat: string[];
    motionRepeat: string[];
    registerName?: string;
  };
};

const vimState = (view: EditorView): VimState | null => (getCM(view)?.state.vim as VimState | undefined) ?? null;

/**
 * Whether vim is in normal mode with nothing pending, where Esc does
 * nothing in vim. `true` without vim.
 */
export function vimIdle(view: EditorView): boolean {
  const cm = getCM(view);
  const state = vimState(view);
  if (!cm || !state) return true;
  const input = state.inputState;
  const pending =
    !!input &&
    (input.keyBuffer.length > 0 ||
      !!input.operator ||
      input.prefixRepeat.length > 0 ||
      input.motionRepeat.length > 0 ||
      !!input.registerName);
  return !state.insertMode && !state.visualMode && !state.insertModeReturn && !pending && !cm.state.dialog;
}

const modeLabel = (mode: string, subMode?: string) =>
  mode === 'visual' && subMode ? (subMode === 'linewise' ? 'VISUAL LINE' : 'VISUAL BLOCK') : mode.toUpperCase();

/** Reports the mode to the view's host while vim is on. */
const modeReporter = ViewPlugin.define((view) => {
  const cm = getCM(view);
  const host = hosts.get(view);
  const changed = (e: { mode: string; subMode?: string }) => host?.mode(modeLabel(e.mode, e.subMode));
  cm?.on('vim-mode-change', changed);
  // Each state starts in normal mode.
  host?.mode('NORMAL');
  return {
    destroy() {
      cm?.off('vim-mode-change', changed);
      host?.mode(null);
    },
  };
});

// `K`: lsp-client's hover only follows the pointer.
const setHover = StateEffect.define<Tooltip | null>();
const hoverField = StateField.define<Tooltip | null>({
  create: () => null,
  update(value, tr) {
    for (const e of tr.effects) if (e.is(setHover)) return e.value;
    return tr.docChanged || tr.selection ? null : value;
  },
  provide: (f) => showTooltip.from(f),
});

type Markup = string | { language: string; value: string } | { kind: string; value: string };
type Hover = { contents: Markup | Markup[]; range?: { start: { line: number; character: number } } };

const markdown = (m: Markup) =>
  typeof m === 'string' ? m : 'language' in m ? '```' + m.language + '\n' + m.value + '\n```' : null;

async function showHover(view: EditorView) {
  const plugin = LSPPlugin.get(view);
  const capabilities = plugin?.client.serverCapabilities as Record<string, unknown> | null | undefined;
  if (!plugin || !capabilities?.hoverProvider) return;
  const { doc } = view.state;
  const pos = view.state.selection.main.head;
  plugin.client.sync();
  const result = await plugin.client
    .request<object, Hover | null>('textDocument/hover', {
      textDocument: { uri: plugin.uri },
      position: plugin.toPosition(pos),
    })
    .catch(() => null);
  if (!result || view.state.doc !== doc || view.state.selection.main.head !== pos) return;
  const parts = Array.isArray(result.contents) ? result.contents : [result.contents];
  const html = parts
    .map((part) => {
      const md = markdown(part);
      return md === null
        ? plugin.docToHTML(part as { kind: 'markdown'; value: string })
        : plugin.docToHTML(md, 'markdown');
    })
    .filter(Boolean)
    .join('<br>');
  if (!html) return;
  const at = result.range ? plugin.fromPosition(result.range.start) : pos;
  view.dispatch({
    effects: setHover.of({
      pos: Math.min(at, pos),
      above: true,
      create() {
        const dom = document.createElement('div');
        dom.className = 'cm-lsp-hover-tooltip cm-lsp-documentation';
        dom.innerHTML = html;
        return { dom };
      },
    }),
  });
}

/** Any key closes a `K` hover, and still does what it does. */
const hoverKeys = EditorView.domEventObservers({
  keydown(_, view) {
    if (view.state.field(hoverField, false)) view.dispatch({ effects: setHover.of(null) });
  },
});

/** Esc closes completions as it leaves insert mode, which vim takes it for. */
const escCloses = Prec.highest(
  EditorView.domEventHandlers({
    keydown(e, view) {
      if (e.key === 'Escape' && completionStatus(view.state) !== null) closeCompletion(view);
      return false;
    },
  }),
);

/** Vim, ahead of every keymap so its Ctrl chords win; Esc handling stays the editor's (`PaneEditor.keydown`). */
export const vimExtension: Extension = [escCloses, vim(), modeReporter, hoverField, hoverKeys];
