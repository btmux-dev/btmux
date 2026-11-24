// Vim keybindings for the native editor (`[native-editor] vim`).
import { getCM, Vim, vim, type CodeMirror } from '@replit/codemirror-vim';
import { Prec, type Extension } from '@codemirror/state';
import { closeCompletion, completionStatus } from '@codemirror/autocomplete';
import { EditorView, ViewPlugin, runScopeHandlers } from '@codemirror/view';
import { canFormatRange, formatRange } from './features';
import { hunkLines } from './gitSigns';
import { VIM_COMMANDS, type VimCommandName } from './vimCommands';

/** What ex commands and the mode line reach in the editor showing a view. */
export interface VimHost {
  /** Save the shown file; resolves true once it holds every edit. */
  save(force: boolean): Promise<boolean>;
  /** Changes when the shown file or the editor's visibility does. */
  epoch(): number;
  hide(): void;
  /** Close the shown file; `force` discards unsaved changes. */
  close(force: boolean): void;
  /** Close every file but the shown one, keeping those with unsaved changes. */
  closeOthers(): void;
  /** Show the open file `step` tabs over. */
  cycle(step: number): void;
  /** The mode changed: NORMAL, INSERT, …, or null without vim. */
  mode(mode: string | null): void;
  /**
   * Show or hide the problems list, listing the workspace's (the quickfix
   * list) or the shown file's (the location list); `auto` shows it only if
   * it has problems.
   */
  problems(open: boolean | 'auto', scope: 'workspace' | 'file'): void;
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

/** Ex command names, for completing them on the command line. */
const exNames = new Set([
  'colorscheme',
  'map',
  'imap',
  'nmap',
  'vmap',
  'omap',
  'noremap',
  'nnoremap',
  'vnoremap',
  'inoremap',
  'onoremap',
  'unmap',
  'mapclear',
  'nmapclear',
  'vmapclear',
  'imapclear',
  'omapclear',
  'undo',
  'redo',
  'set',
  'setlocal',
  'setglobal',
  'sort',
  'substitute',
  'startinsert',
  'nohlsearch',
  'yank',
  'put',
  'delmarks',
  'marks',
  'registers',
  'vglobal',
  'delete',
  'join',
  'normal',
  'global',
]);

// After the command line closes, which focuses the text again.
const problems = (cm: CodeMirror, open: boolean | 'auto', scope: 'workspace' | 'file') => {
  const host = hostOf(cm);
  if (host) setTimeout(() => host.problems(open, scope));
};

// Global to vim, so defined once; each resolves the editor from its view.
const commands: Record<VimCommandName, Parameters<typeof Vim.defineEx>[2]> = {
  write: (cm, params) => void hostOf(cm)?.save(bang(params)),
  // Hiding keeps unsaved changes (the server holds them), so `:q` needs no `!`.
  quit: (cm) => hostOf(cm)?.hide(),
  qall: (cm) => hostOf(cm)?.hide(),
  wq: saveAndHide,
  xit: saveAndHide,
  bdelete: (cm, params) => hostOf(cm)?.close(bang(params)),
  BufOnly: (cm) => hostOf(cm)?.closeOthers(),
  bnext: (cm) => hostOf(cm)?.cycle(1),
  bprevious: (cm) => hostOf(cm)?.cycle(-1),
  copen: (cm) => problems(cm, true, 'workspace'),
  cclose: (cm) => problems(cm, false, 'workspace'),
  cwindow: (cm) => problems(cm, 'auto', 'workspace'),
  lopen: (cm) => problems(cm, true, 'file'),
  lclose: (cm) => problems(cm, false, 'file'),
  lwindow: (cm) => problems(cm, 'auto', 'file'),
};
for (const [name, fn] of Object.entries(commands)) {
  Vim.defineEx(name, VIM_COMMANDS[name as VimCommandName].prefix, fn);
  exNames.add(name);
}

// `gq`: format with the language server when it formats ranges, else hard
// wrap, as codemirror-vim's own `gq` does (Neovim sets 'formatexpr' to the
// server's). `gw` keeps wrapping.
Vim.defineOperator('btmuxFormat', (cm, args, ranges, oldAnchor) => {
  const view = cm.cm6;
  const { anchor, head } = ranges[0];
  if (canFormatRange(view)) {
    const [from, to] = [cm.indexFromPos(anchor), cm.indexFromPos(head)].sort((a, b) => a - b);
    void formatRange(view, from, to);
    return anchor.line <= head.line ? anchor : head;
  }
  const wrap = (cm as unknown as { hardWrap?: (r: { from: number; to: number }) => number }).hardWrap;
  if (!wrap) return;
  const from = Math.min(anchor.line, head.line);
  let to = Math.max(anchor.line, head.line);
  if (args.linewise) to--;
  let end = wrap.call(cm, { from, to });
  if (end > from && args.linewise) end--;
  return (args as { keepCursor?: boolean }).keepCursor ? oldAnchor : ({ line: end, ch: 0 } as typeof oldAnchor);
});
Vim.mapCommand('gq', 'operator', 'btmuxFormat', {}, {});

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

const inputPending = (input: VimState['inputState']) =>
  !!input &&
  (input.keyBuffer.length > 0 ||
    !!input.operator ||
    input.prefixRepeat.length > 0 ||
    input.motionRepeat.length > 0 ||
    !!input.registerName);

/** The mode the editor's mappings apply in: null while vim is midway through a command, or in a prompt. */
export function mappingMode(view: EditorView): 'normal' | 'visual' | 'insert' | null {
  const cm = getCM(view);
  const state = vimState(view);
  if (!cm || !state || cm.state.dialog) return null;
  if (state.insertMode) return (cm.state as { overwrite?: boolean }).overwrite ? null : 'insert';
  if (inputPending(state.inputState)) return null;
  return state.visualMode ? 'visual' : 'normal';
}

/** What vim holds of a command: its keys so far, and whether an operator waits for a motion. */
export function pendingInput(view: EditorView): { keys: string[]; operator: boolean } | null {
  const state = vimState(view);
  if (!state || state.insertMode || !inputPending(state.inputState)) return null;
  const input = state.inputState!;
  return { keys: [...input.keyBuffer], operator: !!input.operator };
}

/** Give vim keys as if typed. Vim resets itself if a command throws; the key that fed it still counts as handled. */
export function feedKeys(view: EditorView, keys: readonly string[]) {
  const cm = getCM(view);
  if (!cm) return;
  for (const key of keys) {
    try {
      Vim.handleKey(cm, key, 'user');
    } catch (error) {
      console.error(error);
    }
  }
}

/**
 * Vim's jump list is global, but its marks are positions in the state they
 * were made in, and the view shows another file's state after a switch:
 * there they'd be wrong or out of range (`gg` would throw). A mark from a
 * replaced state reads as removed, which vim skips. CTRL-O and CTRL-I use
 * the editor's own list (`jumps.ts`), across files.
 */
let marksGuarded = false;
function guardMarks(cm: CodeMirror) {
  if (marksGuarded) return;
  marksGuarded = true;
  const mark = (cm as unknown as { setBookmark(pos: { line: number; ch: number }): { clear(): void } }).setBookmark({
    line: 0,
    ch: 0,
  });
  mark.clear();
  type Mark = { cm: CodeMirror };
  const proto = Object.getPrototypeOf(mark) as { find(this: Mark): unknown };
  const find = proto.find;
  proto.find = function (this: Mark) {
    return getCM(this.cm.cm6) === this.cm ? find.call(this) : null;
  };
}

export function exitVisual(view: EditorView) {
  const cm = getCM(view);
  if (cm?.state.vim?.visualMode) Vim.exitVisualMode(cm as Parameters<typeof Vim.exitVisualMode>[0]);
}

/** Registers and their text, for the which-key popup after `"`. */
export function registers(): { name: string; text: string }[] {
  const controller = Vim.getRegisterController() as unknown as {
    registers: Record<string, { toString(): string } | undefined>;
  };
  return Object.entries(controller.registers)
    .map(([name, register]) => ({ name, text: register?.toString() ?? '' }))
    .filter((r) => r.text);
}

/** Insert-mode `keys` mappings (`jk` to Esc), global to vim like its own. */
let insertMaps: string[] = [];
export function setInsertMappings(maps: { lhs: string; rhs: string }[]) {
  for (const lhs of insertMaps) Vim.unmap(lhs, 'insert');
  for (const { lhs, rhs } of maps) Vim.map(lhs, rhs, 'insert');
  insertMaps = maps.map((m) => m.lhs);
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
  if (cm) guardMarks(cm);
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

/** Tab on the `:` line: the command names starting with the typed one, cycled through and listed, as Vim's wildmenu. */
type Completing = { head: string; matches: string[]; index: number; value: string; list: HTMLElement | null };
const completing = new WeakMap<HTMLInputElement, Completing>();

/** The input of vim's `:` line, if `target` is it (not `/` or `?`). */
function exInput(target: EventTarget | null): HTMLInputElement | null {
  if (!(target instanceof HTMLInputElement) || !target.closest('.cm-vim-panel')) return null;
  return target.parentElement?.firstChild?.textContent === ':' ? target : null;
}

function showMatches(input: HTMLInputElement, state: Completing) {
  if (state.matches.length < 2) return;
  if (!state.list) {
    state.list = document.createElement('span');
    state.list.className = 'cm-btmux-ex-matches';
    input.parentElement?.after(state.list);
  }
  state.list.replaceChildren(
    ...state.matches.map((name, i) => {
      const item = document.createElement('span');
      item.textContent = name;
      if (i === state.index) item.className = 'cm-btmux-ex-match-selected';
      return item;
    }),
  );
  state.list.children[state.index]?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
}

const exCompletion = ViewPlugin.define((view) => {
  const keydown = (e: KeyboardEvent) => {
    const input = exInput(e.target);
    if (!input) return;
    if (e.key !== 'Tab' || e.ctrlKey || e.altKey || e.metaKey) {
      // Vim changes the line without input events (Ctrl-U, history): any other key starts over.
      if (!['Shift', 'Control', 'Alt', 'Meta'].includes(e.key)) forget(input);
      return;
    }
    // Tab would move focus, which closes the line.
    e.preventDefault();
    e.stopPropagation();
    let state = completing.get(input);
    if (state && state.value === input.value) {
      const n = state.matches.length;
      state.index = (state.index + (e.shiftKey ? -1 : 1) + n) % n;
    } else {
      // A range (`%`, `'<,'>`, `1,5`), then the command's name so far.
      const typed = /^([\d\s.,$%'<>+;-]*)([A-Za-z]*)$/.exec(input.value);
      const matches = typed ? [...exNames].filter((name) => name.startsWith(typed[2])).sort() : [];
      if (!typed || !matches.length) return;
      state?.list?.remove();
      state = { head: typed[1], matches, index: e.shiftKey ? matches.length - 1 : 0, value: '', list: null };
      completing.set(input, state);
    }
    input.value = state.value = state.head + state.matches[state.index];
    showMatches(input, state);
  };
  const forget = (input: HTMLInputElement) => {
    completing.get(input)?.list?.remove();
    completing.delete(input);
  };
  // Typing starts over.
  const typed = (e: Event) => {
    const input = exInput(e.target);
    if (input) forget(input);
  };
  view.dom.addEventListener('keydown', keydown, true);
  view.dom.addEventListener('input', typed, true);
  return {
    destroy() {
      view.dom.removeEventListener('keydown', keydown, true);
      view.dom.removeEventListener('input', typed, true);
    },
  };
});

const exCompletionTheme = EditorView.theme({
  '.cm-btmux-ex-matches': {
    display: 'flex',
    gap: '2ch',
    flex: '0 1 auto',
    minWidth: 0,
    maxWidth: '70%',
    marginLeft: '2ch',
    overflowX: 'auto',
    scrollbarWidth: 'none',
    whiteSpace: 'pre',
  },
  '.cm-btmux-ex-matches > span': { opacity: 0.6 },
  '.cm-btmux-ex-matches > .cm-btmux-ex-match-selected': { opacity: 1, color: 'var(--btm-editor-blue)' },
});

/** Vim, ahead of every keymap so its Ctrl chords win; Esc handling stays the editor's (`PaneEditor.keydown`). */
export const vimExtension: Extension = [altKeys, vim(), modeReporter, exCompletion, exCompletionTheme];
