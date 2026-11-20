import {
  crosshairCursor,
  drawSelection,
  dropCursor,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  type ViewUpdate,
  rectangularSelection,
  runScopeHandlers,
} from '@codemirror/view';
import { Compartment, EditorSelection, EditorState, Prec, type Extension } from '@codemirror/state';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { bracketMatching, foldGutter, foldKeymap, indentOnInput, indentUnit } from '@codemirror/language';
import {
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  completionKeymap,
  currentCompletions,
  selectedCompletionIndex,
  setSelectedCompletion,
} from '@codemirror/autocomplete';
import { highlightSelectionMatches, searchKeymap } from '@codemirror/search';
import { virtualText } from './virtualText';
import { forEachDiagnostic, lintKeymap } from '@codemirror/lint';
import { Doc, type DocStatus } from './doc';
import { editorConnection } from './connection';
import { editorTheme, type EditorAppearance } from './theme';
import { detectIndent, languageFor } from './languages';
import { codeDetail } from './completion';
import { panelSpace } from './panelSpace';
import { format, lspExtension, setDisplay, workingServers } from './lsp';
import { serverFoldsChanged } from './features';
import { gitSigns, gitSummary } from './gitSigns';
import { editorCommands, toDiagnostic } from './actions';
import { keyboardTooltips } from './hover';
import { noteOpened, quickOpen } from './quickOpen';
import { EDITOR_COMMANDS, LSP_COMMANDS, QUICK_OPEN_NEEDS_FOLDER } from './registry';
import { Sequences } from './sequences';
import { NAVIGATION_KEYS } from './keyHelp';
import { JumpList, type Jump } from './jumps';
import { vscodeKeys } from './vscodeKeys';
import { isDesktopApp } from '../desktopApp';
import type { KeyEntry, KeyMode } from './keyLayer';
import type { VimBind, VimGroup } from '../../generated/protocol';
import { findPaneState, useStore } from '../../state/store';
import { workspaceFolder } from '../vscodeFolder';
import type { VimHost } from './vim';
import { IS_MAC } from '../nvimKeys';
import { optionChordKey } from '../macOption';

type Vim = typeof import('./vim');
// Loaded once vim is turned on.
let vimLoader: Promise<Vim> | null = null;

/** The keymap scope of chords run ahead of vim (`keydown`). */
const EARLY_SCOPE = 'btmux-early';

/**
 * Ctrl-N/Ctrl-P as in vim and Ctrl-J/Ctrl-K as in fzf (browsers off macOS keep
 * Ctrl-N) through shown completions, ahead of Mod-k chords, vim, and Mod-p
 * (Ctrl-P off macOS). Unlike arrows, they move during CodeMirror's interaction
 * delay, and a list waiting for refreshed results keeps them, doing nothing.
 */
function cycleCompletion(e: KeyboardEvent, view: EditorView): boolean {
  if (!e.ctrlKey || e.metaKey || e.altKey || e.shiftKey) return false;
  // Caps Lock gives uppercase without Shift.
  const key = e.key.toLowerCase();
  const step = key === 'n' || key === 'j' ? 1 : key === 'p' || key === 'k' ? -1 : 0;
  if (!step) return false;
  const n = currentCompletions(view.state).length;
  if (!n) return !!view.dom.querySelector('.cm-tooltip-autocomplete-disabled');
  const i = selectedCompletionIndex(view.state);
  view.dispatch({ effects: setSelectedCompletion(i === null ? (step > 0 ? 0 : n - 1) : (i + step + n) % n) });
  return true;
}

/** Longest wait for formatting before a save goes ahead without it. */
const FORMAT_TIMEOUT_MS = 3000;

/** Longest wait for the active file to load before quick open shows anyway. */
const QUICK_OPEN_WAIT_MS = 2000;

/** Max gap between two Esc presses that hide the editor. */
const DOUBLE_ESC_MS = 300;

/**
 * An Option chord on macOS as CodeMirror's keymaps should see it: by its
 * physical key, since they take it as the text Option types. Null for other
 * keys, and where Option types ASCII text on this layout.
 */
function optionChord(e: KeyboardEvent): KeyboardEvent | null {
  const key = IS_MAC ? optionChordKey(e) : null;
  if (key === null) return null;
  return {
    type: e.type,
    key,
    code: e.code,
    keyCode: e.keyCode,
    altKey: true,
    shiftKey: e.shiftKey,
    ctrlKey: false,
    metaKey: false,
    repeat: e.repeat,
    preventDefault: () => e.preventDefault(),
    stopPropagation: () => e.stopPropagation(),
  } as KeyboardEvent;
}

export interface PaneEditorEvents {
  /** Status, cursor, or the active file changed. */
  changed(): void;
  /** A quick double Esc. */
  hide(): void;
  /** A save or revert failed. */
  error(message: string): void;
  /** Open `path` in this pane (a jump to another file), on `line` if given. */
  open(path: string, line?: number | null): void;
  /** Close `path`, asking to save unsaved changes unless `force` discards them. */
  close(path: string, force: boolean): void;
  /** Run a btmux action (a mapping to one that isn't the editor's). */
  action(name: string): void;
}

/** Actions run from visual mode that keep it: they change the selection. */
const KEEPS_VISUAL = new Set(['editor-expand-selection', 'editor-shrink-selection']);

export interface FileStatus extends DocStatus {
  language: string | null;
  /** The last save found the file changed on disk. */
  conflict: boolean;
  /** Language server ids. */
  servers: number[];
}

/** The native editor over one pane: a CodeMirror view showing one of its open files. */
export class PaneEditor {
  readonly view: EditorView;
  private docs = new Map<string, Doc>();
  private languages = new Map<string, { name: string; support: Extension }>();
  private conflicts = new Set<string>();
  private reveals = new Map<string, number>();
  private active: string | null = null;
  private appearance: EditorAppearance;
  private readonly themeC = new Compartment();
  private readonly languageC = new Compartment();
  private readonly lspC = new Compartment();
  private readonly vimC = new Compartment();
  private readonly virtualTextC = new Compartment();
  private virtualText = false;
  /** Docs' working servers when last configured. */
  private working = new WeakMap<Doc, string>();
  private stopStatus: () => void;
  /** The last Esc of a quick run, and whether the run can hide the editor. */
  private esc = { last: -Infinity, armed: false };
  private vim = false;
  private vimLib: Vim | null = null;
  private readonly vimHost: VimHost = {
    save: (force) => this.save(force),
    epoch: () => this.epoch,
    hide: () => this.events.hide(),
    close: (force) => this.close(force),
    mode: (mode) => {
      if (mode === this.vimMode) return;
      this.vimMode = mode;
      this.events.changed();
    },
  };
  private waiting: { path: string; resolve: (view: EditorView | null) => void }[] = [];
  private quickOpenWaiting = false;
  private destroyed = false;
  cursor = { line: 1, column: 1, selected: 0 };
  problems = { errors: 0, warnings: 0 };
  /** Lines changed from the Git index; `null` outside Git. */
  git: { added: number; changed: number; removed: number } | null = null;
  formatOnSave = false;
  /** Changes when the shown file or the editor's visibility does. */
  private epoch = 0;
  private visible = true;
  /** Vim's mode while vim is on, for the status line. */
  vimMode: string | null = null;
  /** Key sequences: mappings ahead of vim, and what's pending of them. */
  readonly keys: Sequences;
  private insertMaps: { lhs: string; rhs: string }[] = [];
  private readonly jumps = new JumpList();
  /** Going back or forward: its moves aren't jumps. */
  private navigating = false;
  /** Just switched files: the first move there (to a jump's target, a revealed line) isn't one. */
  private landed = false;

  constructor(
    parent: HTMLElement,
    private readonly paneId: string,
    appearance: EditorAppearance,
    private readonly events: PaneEditorEvents,
  ) {
    this.appearance = appearance;
    this.keys = new Sequences({
      mode: () => this.mappingMode(),
      vimPending: () => (this.vim && this.vimLib ? this.vimLib.pendingInput(this.view) : null),
      registers: () => this.vimLib?.registers() ?? [],
      run: (entry, mode) => this.runMapping(entry, mode),
      replay: (keys) => {
        if (this.vim) this.vimLib?.feedKeys(this.view, keys);
      },
      changed: () => this.events.changed(),
    });
    this.view = new EditorView({ parent, state: this.placeholder() });
    // Drop a server's features once it fails, and add them back if it recovers.
    this.stopStatus = editorConnection.onLspStatus(() => {
      for (const doc of this.docs.values()) {
        const working = workingServers(doc).join();
        if (!doc.state || working === this.working.get(doc)) continue;
        this.working.set(doc, working);
        doc.update({ effects: this.lspC.reconfigure(lspExtension(doc)) });
      }
    });
  }

  private placeholder() {
    // Focusable, so focus waits here while a file loads.
    return EditorState.create({ extensions: [this.common(), EditorState.readOnly.of(true)] });
  }

  /** What every state has, loaded or not. */
  private common(): Extension {
    return [
      this.themeC.of(editorTheme(this.appearance)),
      EditorView.contentAttributes.of({
        'data-native-editor': '',
        'data-pane-id': this.paneId,
        'aria-label': 'Editor',
      }),
      // Ahead of vim, which takes Esc and Ctrl-P.
      Prec.highest(EditorView.domEventHandlers({ keydown: (e, view) => this.keydown(e, view) })),
      EditorView.domEventHandlers({
        // Jumps to other files open where the user is.
        focus: () => {
          setDisplay((path) => this.display(path));
          if (this.active) noteOpened(this.active);
        },
        // Keys held for a mapping don't wait for a return.
        blur: () => this.keys.reset(),
      }),
      keymap.of([
        { key: 'Mod-p', run: () => this.quickOpen(), preventDefault: true, scope: EARLY_SCOPE },
        // Mod-W closes the browser tab.
        { key: 'Alt-w', run: () => this.close(false), preventDefault: true, scope: EARLY_SCOPE },
        // In the desktop app, Mod-W closes the file; with none, the window
        // closes as elsewhere (`close.js`), as the key goes unhandled.
        ...(isDesktopApp() ? [{ key: 'Mod-w', run: () => this.close(false) }] : []),
      ]),
    ];
  }

  /** Show `path` here; resolves with the view once it is loaded. */
  private display(path: string): Promise<EditorView | null> {
    const doc = this.docs.get(path);
    if (path === this.active && doc?.view && doc.status.ready) return Promise.resolve(this.view);
    this.events.open(path);
    return this.whenShown(path);
  }

  /** Resolves with the view once `path` is shown and loaded, or null after `timeout` ms. */
  private whenShown(path: string, timeout = 10_000): Promise<EditorView | null> {
    return new Promise((resolve) => {
      const waiter = { path, resolve };
      this.waiting.push(waiter);
      setTimeout(() => {
        if (!this.waiting.includes(waiter)) return;
        this.waiting = this.waiting.filter((w) => w !== waiter);
        resolve(null);
      }, timeout);
    });
  }

  private resolveWaiting() {
    const doc = this.active ? this.docs.get(this.active) : undefined;
    if (!doc?.view || !doc.status.ready) return;
    const ready = this.waiting.filter((w) => w.path === doc.path);
    this.waiting = this.waiting.filter((w) => w.path !== doc.path);
    for (const w of ready) w.resolve(this.view);
  }

  private extensions(doc: Doc, text: string): Extension {
    return [
      this.common(),
      // Ahead of the keymaps, so vim's chords win over theirs.
      this.vimC.of(this.vimExtension()),
      this.virtualTextC.of(this.virtualText ? virtualText : []),
      lineNumbers(),
      gitSigns(() => doc.git),
      highlightActiveLineGutter(),
      highlightSpecialChars(),
      history(),
      foldGutter({ foldingChanged: serverFoldsChanged }),
      drawSelection(),
      dropCursor(),
      EditorState.allowMultipleSelections.of(true),
      indentOnInput(),
      bracketMatching(),
      closeBrackets(),
      autocompletion({ addToOptions: [codeDetail] }),
      panelSpace,
      rectangularSelection(),
      crosshairCursor(),
      highlightActiveLine(),
      highlightSelectionMatches(),
      indentUnit.of(detectIndent(text)),
      this.languageC.of(this.languages.get(doc.path)?.support ?? []),
      this.lspC.of(this.lspFor(doc)),
      keyboardTooltips,
      keymap.of([
        { key: 'Mod-s', run: () => (void this.save(false), true), preventDefault: true },
        // VS Code's next and previous problem, ahead of lint's F8.
        { key: 'F8', run: (view) => toDiagnostic(view, 'next'), preventDefault: true },
        { key: 'Shift-F8', run: (view) => toDiagnostic(view, 'prev'), preventDefault: true },
        { key: NAVIGATION_KEYS.back, run: () => this.navigate(true), preventDefault: true },
        { key: NAVIGATION_KEYS.forward, run: () => this.navigate(false), preventDefault: true },
        ...vscodeKeys(
          isDesktopApp() ? { workspaceSymbols: (view) => !!editorCommands['editor-workspace-symbols'](view) } : null,
        ),
        ...closeBracketsKeymap,
        ...defaultKeymap,
        ...searchKeymap,
        ...historyKeymap,
        ...foldKeymap,
        ...completionKeymap,
        ...lintKeymap,
        indentWithTab,
      ]),
      EditorView.updateListener.of((update) => {
        if (update.docChanged) doc.push();
        if (update.docChanged) this.jumps.map(doc.path, update.changes);
        else if (update.selectionSet && doc.view) this.noteMove(doc, update);
        if (!doc.view) return;
        if (update.docChanged || update.selectionSet) this.updateCursor();
        if (update.docChanged || update.transactions.some((tr) => tr.effects.length > 0)) {
          this.countProblems();
          this.countGit();
        }
      }),
    ];
  }

  private lspFor(doc: Doc) {
    this.working.set(doc, workingServers(doc).join());
    return lspExtension(doc);
  }

  private keydown(e: KeyboardEvent, view: EditorView): boolean {
    // Moves after a key are the user's, even if nothing moved since a switch.
    this.landed = false;
    const plainEsc = e.key === 'Escape' && !e.ctrlKey && !e.altKey && !e.metaKey && !e.shiftKey;
    // Esc drops keys held for a mapping, and nothing else.
    if (plainEsc && this.keys.cancel()) {
      this.esc = { last: -Infinity, armed: false };
      e.preventDefault();
      return true;
    }
    // Otherwise it goes to vim, which may end a command the popup shows.
    if (plainEsc) this.keys.passed('<Esc>');
    // A quick second Esc hides the editor; the first still closes panels and
    // completions. With vim, only a run of Escs that starts in normal mode
    // with nothing pending hides it: one that leaves insert or visual mode,
    // or cancels a command, never does.
    if (plainEsc && !e.repeat) {
      const idle = this.vimLib?.vimIdle(this.view) ?? true;
      if (e.timeStamp - this.esc.last >= DOUBLE_ESC_MS) {
        this.esc = { last: e.timeStamp, armed: idle };
        return false;
      }
      this.esc.last = e.timeStamp;
      if (!this.esc.armed || !idle) return false;
      this.esc = { last: -Infinity, armed: false };
      e.preventDefault();
      this.events.hide();
      return true;
    }
    this.esc = { last: -Infinity, armed: false };
    if (cycleCompletion(e, view)) {
      e.preventDefault();
      return true;
    }
    if (this.docs.get(this.active ?? '')?.view && this.keys.keydown(e)) {
      e.preventDefault();
      return true;
    }
    const option = optionChord(e);
    // CodeMirror tries a shifted chord unshifted first: Shift-Option-W isn't Alt-w.
    if (option)
      return (
        (!option.shiftKey && runScopeHandlers(view, option, EARLY_SCOPE)) || runScopeHandlers(view, option, 'editor')
      );
    return runScopeHandlers(view, e, EARLY_SCOPE);
  }

  private here(): Jump | null {
    const doc = this.active ? this.docs.get(this.active) : undefined;
    if (!doc?.view) return null;
    const pos = this.view.state.selection.main.head;
    return { path: doc.path, pos, line: this.view.state.doc.lineAt(pos).number };
  }

  /**
   * Record a jump: a move tagged as one (to a definition, a symbol, a
   * diagnostic), or any other that leaves the screen (`G`, a search).
   */
  private noteMove(doc: Doc, update: ViewUpdate) {
    const landed = this.landed;
    this.landed = false;
    if (this.navigating || landed) return;
    const tagged = update.transactions.some(
      (tr) => tr.isUserEvent('select.definition') || tr.isUserEvent('select.jump'),
    );
    const before = update.startState.selection.main.head;
    const fromLine = update.startState.doc.lineAt(before).number;
    const toLine = update.state.doc.lineAt(update.state.selection.main.head).number;
    const screen = Math.max(1, Math.floor(this.view.dom.clientHeight / this.view.defaultLineHeight));
    const pointer = update.transactions.some((tr) => tr.isUserEvent('select.pointer'));
    if (tagged || (!pointer && Math.abs(toLine - fromLine) >= screen))
      this.jumps.record({ path: doc.path, pos: before, line: fromLine });
  }

  /** Go back (or forward) to where a jump left. */
  private navigate(back: boolean): boolean {
    const here = this.here();
    if (!here) return false;
    const target = back ? this.jumps.back(here) : this.jumps.forward(here);
    if (!target) return true;
    this.navigating = true;
    void this.display(target.path).then((view) => {
      // The move there isn't a jump of its own.
      if (view) {
        const pos = Math.min(target.pos, view.state.doc.length);
        view.dispatch({ selection: { anchor: pos }, effects: EditorView.scrollIntoView(pos, { y: 'center' }) });
      }
      this.navigating = false;
    });
    return true;
  }

  /** Where mappings apply: without vim, VS Code's chords; with it, its mode once nothing is pending. */
  private mappingMode(): KeyMode | null {
    if (!this.vim) return 'plain';
    return this.vimLib?.mappingMode(this.view) ?? null;
  }

  private runMapping(entry: KeyEntry, mode: KeyMode) {
    if (entry.rhs) {
      this.vimLib?.feedKeys(this.view, entry.rhs);
      return;
    }
    const action = entry.action;
    if (!action) return;
    if (!EDITOR_COMMANDS.includes(action)) this.events.action(action);
    else if (!this.command(action))
      useStore
        .getState()
        .showToast(
          LSP_COMMANDS.includes(action) ? 'Needs a language server for this file' : `Can't run ${action} here`,
          'info',
        );
    if (mode === 'visual' && !KEEPS_VISUAL.has(action)) this.vimLib?.exitVisual(this.view);
  }

  /** The vim mappings and which-key settings from the config. */
  setKeys(binds: readonly VimBind[], groups: readonly VimGroup[], timeout: number, whichKey: boolean) {
    // Vim maps typed keys in insert mode itself, inserting them as it waits.
    const insertKeys = (b: VimBind) => b.mode === 'insert' && !!b.rhs;
    this.keys.configure(
      binds.filter((b) => !insertKeys(b)),
      groups,
      timeout,
      whichKey,
    );
    this.insertMaps = binds.filter(insertKeys).map((b) => ({ lhs: b.keys.join(''), rhs: b.rhs!.join('') }));
    this.vimLib?.setInsertMappings(this.insertMaps);
  }

  private countProblems() {
    const problems = { errors: 0, warnings: 0 };
    forEachDiagnostic(this.view.state, (d) => {
      if (d.severity === 'error') problems.errors++;
      else if (d.severity === 'warning') problems.warnings++;
    });
    if (problems.errors === this.problems.errors && problems.warnings === this.problems.warnings) return;
    this.problems = problems;
    this.events.changed();
  }

  private countGit() {
    const git = gitSummary(this.view.state);
    const same =
      git === this.git ||
      (git &&
        this.git &&
        git.added === this.git.added &&
        git.changed === this.git.changed &&
        git.removed === this.git.removed);
    if (same) return;
    this.git = git;
    this.events.changed();
  }

  private updateCursor() {
    const state = this.view.state;
    const head = state.selection.main.head;
    const line = state.doc.lineAt(head);
    const selected = state.selection.ranges.reduce((n, r) => n + r.to - r.from, 0);
    const next = { line: line.number, column: head - line.from + 1, selected };
    if (next.line === this.cursor.line && next.column === this.cursor.column && next.selected === this.cursor.selected)
      return;
    this.cursor = next;
    this.events.changed();
  }

  /** Follow the pane's open files, showing `active`. */
  setFiles(files: string[], active: string | null) {
    for (const [path, doc] of this.docs) {
      if (files.includes(path)) continue;
      if (doc.view) this.view.setState(this.placeholder());
      doc.attach(null);
      doc.close();
      this.docs.delete(path);
      this.conflicts.delete(path);
    }
    for (const path of files) {
      if (this.docs.has(path)) continue;
      const doc = new Doc(
        path,
        (d, text) => this.extensions(d, text),
        () => this.docChanged(doc),
      );
      this.docs.set(path, doc);
      doc.open();
      void this.loadLanguage(doc);
    }
    if (active !== this.active || (active && !this.docs.get(active)?.view)) {
      const focused = this.view.hasFocus;
      const from = active !== this.active ? this.here() : null;
      if (from && !this.navigating) this.jumps.record(from);
      if (from) this.landed = true;
      if (this.active) this.docs.get(this.active)?.attach(null);
      this.active = active && this.docs.has(active) ? active : null;
      this.keys.reset();
      // Recent files are those viewed here, not other tabs' or restored ones.
      if (this.active && focused) noteOpened(this.active);
      this.epoch++;
      const doc = this.active ? this.docs.get(this.active) : undefined;
      // Not loaded yet: shown once it is (docChanged).
      if (!doc || !doc.attach(this.view)) this.view.setState(this.placeholder());
      if (doc?.view) this.afterShow(doc);
      if (focused) this.view.focus();
      this.events.changed();
    }
  }

  private afterShow(doc: Doc) {
    // Settings may have changed while hidden.
    doc.update({ effects: this.themeC.reconfigure(editorTheme(this.appearance)) });
    this.applyReveal(doc);
    this.updateCursor();
    this.countProblems();
    this.countGit();
    this.resolveWaiting();
  }

  private docChanged(doc: Doc) {
    if (doc.path === this.active && !doc.view && doc.state) {
      const focused = this.view.hasFocus;
      doc.attach(this.view);
      this.afterShow(doc);
      if (focused) this.view.focus();
    } else if (doc.view) {
      this.applyReveal(doc);
      this.resolveWaiting();
    }
    this.events.changed();
  }

  private async loadLanguage(doc: Doc) {
    const language = await languageFor(doc.path);
    if (!language || this.docs.get(doc.path) !== doc) return;
    this.languages.set(doc.path, language);
    doc.update({ effects: this.languageC.reconfigure(language.support) });
    this.events.changed();
  }

  /** Put the cursor on `line` of `path` once it is shown and loaded. */
  reveal(path: string, line: number) {
    this.reveals.set(path, line);
    const doc = this.docs.get(path);
    if (doc?.view) this.applyReveal(doc);
  }

  private applyReveal(doc: Doc) {
    const line = this.reveals.get(doc.path);
    const state = doc.state;
    if (line === undefined || !state || !doc.status.ready) return;
    this.reveals.delete(doc.path);
    const target = state.doc.line(Math.max(1, Math.min(line, state.doc.lines)));
    doc.update({
      selection: EditorSelection.cursor(target.from),
      effects: EditorView.scrollIntoView(target.from, { y: 'center' }),
    });
  }

  /** Turn vim keybindings on or off in every open file, keeping their views. */
  setVim(on: boolean) {
    if (on === this.vim) return;
    this.vim = on;
    if (!on || this.vimLib) return this.applyVim();
    void (vimLoader ??= import('./vim')).then((lib) => {
      if (!this.vimLib) {
        this.vimLib = lib;
        lib.setVimHost(this.view, this.vimHost);
        lib.setInsertMappings(this.insertMaps);
      }
      this.applyVim();
    });
  }

  private vimExtension(): Extension {
    return this.vim && this.vimLib ? Prec.high(this.vimLib.vimExtension) : [];
  }

  private applyVim() {
    for (const doc of this.docs.values()) doc.update({ effects: this.vimC.reconfigure(this.vimExtension()) });
  }

  /** Show diagnostics' messages at the end of their lines, or not. */
  setVirtualText(on: boolean) {
    if (on === this.virtualText) return;
    this.virtualText = on;
    for (const doc of this.docs.values()) doc.update({ effects: this.virtualTextC.reconfigure(on ? virtualText : []) });
  }

  setAppearance(appearance: EditorAppearance) {
    this.appearance = appearance;
    this.view.dispatch({ effects: this.themeC.reconfigure(editorTheme(appearance)) });
  }

  status(path: string): FileStatus | null {
    const doc = this.docs.get(path);
    if (!doc) return null;
    return {
      ...doc.status,
      language: this.languages.get(path)?.name ?? null,
      conflict: this.conflicts.has(path),
      servers: doc.servers,
    };
  }

  get activePath() {
    return this.active;
  }

  focus() {
    // Not out of a panel (quick open, search).
    if (!this.view.dom.contains(document.activeElement)) this.view.focus();
  }

  /** Close the shown file. */
  close(force: boolean): boolean {
    if (!this.active) return false;
    this.events.close(this.active, force);
    return true;
  }

  /** Show quick open, once the active file is loaded. */
  quickOpen(): boolean {
    if (this.quickOpenWaiting) return true;
    const show = () => {
      if (this.destroyed) return;
      const pane = findPaneState(useStore.getState(), this.paneId);
      const root = workspaceFolder(pane);
      if (!pane || !root) return useStore.getState().showToast(QUICK_OPEN_NEEDS_FOLDER, 'info');
      this.view.focus();
      quickOpen(this.view, { root, files: pane.editor.files, active: this.active }, (path, line) =>
        this.events.open(path, line),
      );
    };
    // Loading replaces the state, and the picker with it.
    const doc = this.active ? this.docs.get(this.active) : undefined;
    if (doc && !doc.view && !doc.status.error) {
      this.quickOpenWaiting = true;
      void this.whenShown(doc.path, QUICK_OPEN_WAIT_MS).then(() => {
        this.quickOpenWaiting = false;
        show();
      });
    } else show();
    return true;
  }

  /** Run a palette command (`editorCommands`, quick open) on the shown file. */
  command(action: string): boolean {
    if (action === 'editor-quick-open') return this.quickOpen();
    if (action === 'editor-go-back' || action === 'editor-go-forward')
      return this.navigate(action === 'editor-go-back');
    const run = editorCommands[action];
    const doc = this.active ? this.docs.get(this.active) : undefined;
    if (!run || !doc?.view) return false;
    this.view.focus();
    const ran = run(this.view);
    return ran instanceof Promise ? true : ran;
  }

  /** Shown over its pane, or hidden. */
  setVisible(visible: boolean) {
    if (visible === this.visible) return;
    this.visible = visible;
    this.epoch++;
    this.keys.reset();
  }

  /**
   * Write the active file; `force` overwrites outside changes. Resolves
   * true once the file holds every local change.
   */
  async save(force: boolean, path = this.active): Promise<boolean> {
    const doc = path ? this.docs.get(path) : undefined;
    if (!doc?.buffer) return false;
    if (this.formatOnSave && doc.view && !force) {
      const timeout = new Promise((resolve) => setTimeout(resolve, FORMAT_TIMEOUT_MS));
      await Promise.race([format(doc.view).catch(() => {}), timeout]);
    }
    doc.push();
    // Edits the server doesn't have, or dropped as stale, aren't in what it writes.
    const mark = doc.saveMark();
    const result = await editorConnection.save(doc.buffer, force);
    const complete = mark !== null && doc.confirmed(mark);
    if (result.conflict) this.conflicts.add(doc.path);
    else this.conflicts.delete(doc.path);
    if (result.error && !result.conflict) this.events.error(result.error);
    else if (!result.error && !complete) this.events.error("Some edits weren't in the save yet; save again.");
    this.events.changed();
    return !result.error && !result.conflict && complete;
  }

  /** Reload the active file from disk, dropping unsaved changes. */
  async revert(path = this.active) {
    const doc = path ? this.docs.get(path) : undefined;
    if (!doc?.buffer) return;
    const result = await editorConnection.revert(doc.buffer);
    this.conflicts.delete(doc.path);
    if (result.error) this.events.error(result.error);
    this.events.changed();
  }

  destroy() {
    this.destroyed = true;
    this.stopStatus();
    for (const doc of this.docs.values()) {
      doc.attach(null);
      doc.close();
    }
    this.docs.clear();
    this.view.destroy();
  }
}
