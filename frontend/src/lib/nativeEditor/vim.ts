// Vim keybindings for the native editor (`[native-editor] vim`).
import { getCM, Vim, vim, type CodeMirror } from '@replit/codemirror-vim';
import { Prec, StateEffect, StateField, type Extension } from '@codemirror/state';
import { closeCompletion, completionStatus } from '@codemirror/autocomplete';
import { EditorView, ViewPlugin, runScopeHandlers, showTooltip, type Tooltip } from '@codemirror/view';
import { jumpToDefinition, LSPPlugin } from '@codemirror/lsp-client';
import { forEachDiagnostic } from '@codemirror/lint';
import { ask, has } from './features';
import {
  blameCurrentLine,
  hunkLines,
  nextHunk,
  prevHunk,
  previewHunk,
  resetBuffer,
  resetHunk,
  stageBuffer,
  stageHunk,
  gitCommands,
} from './gitSigns';

/** What ex commands and the mode line reach in the editor showing a view. */
export interface VimHost {
  /** Save the shown file; resolves true once it holds every edit. */
  save(force: boolean): Promise<boolean>;
  /** Changes when the shown file or the editor's visibility does. */
  epoch(): number;
  hide(): void;
  /** Close the shown file; `force` discards unsaved changes. */
  close(force: boolean): void;
  /** The mode changed: NORMAL, INSERT, …, or null without vim. */
  mode(mode: string | null): void;
}

const hosts = new WeakMap<EditorView, VimHost>();

export const setVimHost = (view: EditorView, host: VimHost) => hosts.set(view, host);

const hostOf = (cm: CodeMirror) => hosts.get(cm.cm6);

// `:w!` parses as `w` with the argument `!`.
const bang = (params: { argString?: string }) => params.argString?.trim().startsWith('!') ?? false;

// Hides only if the save worked and the editor still shows that file, unhidden.
const saveAndHide = (cm: CodeMirror, params: { argString?: string }) => {
  const host = hostOf(cm);
  if (!host) return;
  const epoch = host.epoch();
  void host.save(bang(params)).then((saved) => saved && host.epoch() === epoch && host.hide());
};

// Global to vim, so defined once; each resolves the editor from its view.
Vim.defineEx('write', 'w', (cm, params) => void hostOf(cm)?.save(bang(params)));
Vim.defineEx('wq', 'wq', saveAndHide);
Vim.defineEx('xit', 'x', saveAndHide);
// Hiding keeps unsaved changes (the server holds them), so `:q` needs no `!`.
Vim.defineEx('quit', 'q', (cm) => hostOf(cm)?.hide());
Vim.defineEx('qall', 'qa', (cm) => hostOf(cm)?.hide());
Vim.defineEx('bdelete', 'bd', (cm, params) => hostOf(cm)?.close(bang(params)));

Vim.defineAction('btmuxDefinition', (cm) => void jumpToDefinition(cm.cm6));
Vim.defineAction('btmuxHover', (cm) => void showHover(cm.cm6));
Vim.defineAction('btmuxNextDiagnostic', (cm) => toDiagnostic(cm.cm6, true));
Vim.defineAction('btmuxPrevDiagnostic', (cm) => toDiagnostic(cm.cm6, false));
Vim.mapCommand('gd', 'action', 'btmuxDefinition', {}, { context: 'normal' });
Vim.mapCommand('K', 'action', 'btmuxHover', {}, { context: 'normal' });
Vim.mapCommand(']d', 'action', 'btmuxNextDiagnostic', {}, { context: 'normal' });
Vim.mapCommand('[d', 'action', 'btmuxPrevDiagnostic', {}, { context: 'normal' });

// Git signs, with the mappings gitsigns.nvim suggests (`\` is Vim's default leader).
const gitActions: [string, string, (view: EditorView) => boolean, string?][] = [
  [']c', 'btmuxNextHunk', nextHunk],
  ['[c', 'btmuxPrevHunk', prevHunk],
  ['\\hs', 'btmuxStageHunk', stageHunk, 'visual'],
  ['\\hr', 'btmuxResetHunk', resetHunk, 'visual'],
  ['\\hS', 'btmuxStageBuffer', stageBuffer],
  ['\\hR', 'btmuxResetBuffer', resetBuffer],
  ['\\hp', 'btmuxPreviewHunk', previewHunk],
  ['\\hb', 'btmuxBlameLine', blameCurrentLine],
  ['\\tb', 'btmuxToggleLineBlame', gitCommands['editor-toggle-line-blame']],
];
for (const [keys, name, run, visual] of gitActions) {
  Vim.defineAction(name, (cm) => {
    run(cm.cm6);
    // A range staged or reset from visual mode ends it.
    if (visual && cm.state.vim?.visualMode) Vim.exitVisualMode(cm);
  });
  Vim.mapCommand(keys, 'action', name, {}, { context: 'normal' });
  if (visual) Vim.mapCommand(keys, 'action', name, {}, { context: 'visual' });
}
// `ih`: the hunk's lines, as a text object.
Vim.defineMotion('btmuxHunk', (cm, head) => {
  const lines = hunkLines(cm.cm6, head.line);
  if (!lines) return null;
  const last = cm.cm6.state.doc.line(lines.last);
  return [
    { line: lines.first - 1, ch: 0 },
    { line: lines.last - 1, ch: last.length },
  ];
});
for (const context of ['operatorPending', 'visual'])
  Vim.mapCommand('ih', 'motion', 'btmuxHunk', { linewise: true }, { context });

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
 * nothing in vim: no search highlight to clear, no macro recording.
 */
export function vimIdle(view: EditorView): boolean {
  const cm = getCM(view);
  const state = vimState(view);
  if (!cm || !state) return true;
  const plugin = cm.state.vimPlugin as { query?: unknown } | undefined;
  const recording = (Vim.getVimGlobalState_() as { macroModeState?: { isRecording?: boolean } }).macroModeState
    ?.isRecording;
  if (plugin?.query || recording) return false;
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

/** Views whose vim state was just replaced in insert mode (a reopen, a file switch). */
const carried = new WeakSet<EditorView>();

/**
 * Reports the mode to the view's host while vim is on. Each state starts in
 * normal mode, so a state replaced in insert mode (Doc.opened, a file
 * switch) goes back into insert mode, rather than running what's typed next
 * as commands.
 */
const modeReporter = ViewPlugin.define((view) => {
  const cm = getCM(view);
  const host = hosts.get(view);
  let mode = 'NORMAL';
  const changed = (e: { mode: string; subMode?: string }) => {
    mode = modeLabel(e.mode, e.subMode);
    host?.mode(mode);
    // However insert mode ends (Esc, Ctrl-[, Ctrl-C), its completions close.
    if (e.mode !== 'insert' && completionStatus(view.state) !== null) queueMicrotask(() => closeCompletion(view));
  };
  cm?.on('vim-mode-change', changed);
  host?.mode(mode);
  if (cm && carried.has(view)) {
    carried.delete(view);
    // Not during the update that made this state.
    queueMicrotask(() => {
      if (getCM(view) === cm && !cm.state.vim?.insertMode) Vim.handleKey(cm, 'i', 'mapping');
    });
  }
  return {
    destroy() {
      cm?.off('vim-mode-change', changed);
      host?.mode(null);
      // Only a state replacing this one in the same update takes it.
      if (mode !== 'INSERT') return;
      carried.add(view);
      queueMicrotask(() => carried.delete(view));
    },
  };
});

/**
 * Alt chords go to the editor's keymaps first (format selection, incoming
 * calls, moving lines): on macOS vim takes every one in normal mode, which
 * keeps option characters out of the text.
 */
const altKeys = Prec.highest(
  EditorView.domEventHandlers({
    keydown(e, view) {
      if (!e.altKey || e.ctrlKey || e.metaKey) return false;
      return runScopeHandlers(view, e, 'editor');
    },
  }),
);

/** Keys that leave a hover up: modifiers alone, and Cmd chords such as copying from it. */
const keepsHover = (e: KeyboardEvent) =>
  e.key === 'Shift' || e.key === 'Control' || e.key === 'Alt' || e.key === 'Meta' || e.metaKey;

// `K`: lsp-client's hover only follows the pointer.
/** Counts keys, so a hover answered after one (Esc, `i`) isn't shown; a new state gets a new one. */
const hoverRequests = ViewPlugin.fromClass(
  class {
    generation = 0;
  },
  {
    eventObservers: {
      keydown(e, view) {
        if (keepsHover(e)) return;
        this.generation++;
        // Any key closes a `K` hover, and still does what it does.
        if (view.state.field(hoverField, false)) view.dispatch({ effects: setHover.of(null) });
      },
    },
  },
);

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
  const requests = view.plugin(hoverRequests);
  if (!plugin || !requests || !has(plugin, 'hoverProvider')) return;
  const generation = ++requests.generation;
  const pos = view.state.selection.main.head;
  const result = await ask<Hover | null>(view, 'textDocument/hover', (p) => ({ position: p.toPosition(pos) })).catch(
    () => null,
  );
  const current = view.plugin(hoverRequests) === requests && requests.generation === generation;
  if (!result || !current || view.state.selection.main.head !== pos) return;
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

/** Vim, ahead of every keymap so its Ctrl chords win; Esc handling stays the editor's (`PaneEditor.keydown`). */
export const vimExtension: Extension = [altKeys, vim(), modeReporter, hoverField, hoverRequests];
