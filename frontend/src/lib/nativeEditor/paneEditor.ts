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
  rectangularSelection,
} from '@codemirror/view';
import { Compartment, EditorSelection, EditorState, type Extension } from '@codemirror/state';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { bracketMatching, foldGutter, foldKeymap, indentOnInput, indentUnit } from '@codemirror/language';
import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from '@codemirror/autocomplete';
import { highlightSelectionMatches, searchKeymap } from '@codemirror/search';
import { lintKeymap } from '@codemirror/lint';
import { Doc, type DocStatus } from './doc';
import { editorConnection } from './connection';
import { editorTheme, type EditorAppearance } from './theme';
import { detectIndent, languageFor } from './languages';

/** Max gap between two Esc presses that hide the editor. */
const DOUBLE_ESC_MS = 300;

export interface PaneEditorEvents {
  /** Status, cursor, or the active file changed. */
  changed(): void;
  /** A quick double Esc. */
  hide(): void;
  /** A save or revert failed. */
  error(message: string): void;
}

export interface FileStatus extends DocStatus {
  language: string | null;
  /** The last save found the file changed on disk. */
  conflict: boolean;
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
  private lastEsc = -Infinity;
  cursor = { line: 1, column: 1, selected: 0 };

  constructor(
    parent: HTMLElement,
    private readonly paneId: string,
    appearance: EditorAppearance,
    private readonly events: PaneEditorEvents,
  ) {
    this.appearance = appearance;
    this.view = new EditorView({ parent, state: this.placeholder() });
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
      EditorView.domEventHandlers({ keydown: (e) => this.keydown(e) }),
    ];
  }

  private extensions(doc: Doc, text: string): Extension {
    return [
      this.common(),
      lineNumbers(),
      highlightActiveLineGutter(),
      highlightSpecialChars(),
      history(),
      foldGutter(),
      drawSelection(),
      dropCursor(),
      EditorState.allowMultipleSelections.of(true),
      indentOnInput(),
      bracketMatching(),
      closeBrackets(),
      autocompletion(),
      rectangularSelection(),
      crosshairCursor(),
      highlightActiveLine(),
      highlightSelectionMatches(),
      indentUnit.of(detectIndent(text)),
      this.languageC.of(this.languages.get(doc.path)?.support ?? []),
      keymap.of([
        { key: 'Mod-s', run: () => (void this.save(false), true), preventDefault: true },
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
        if (doc.view && (update.docChanged || update.selectionSet)) this.updateCursor();
      }),
    ];
  }

  private keydown(e: KeyboardEvent): boolean {
    // A quick second Esc hides the editor; the first still closes panels and completions.
    if (e.key === 'Escape' && !e.repeat && !e.ctrlKey && !e.altKey && !e.metaKey && !e.shiftKey) {
      if (e.timeStamp - this.lastEsc < DOUBLE_ESC_MS) {
        this.lastEsc = -Infinity;
        e.preventDefault();
        this.events.hide();
        return true;
      }
      this.lastEsc = e.timeStamp;
    } else this.lastEsc = -Infinity;
    return false;
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
      if (this.active) this.docs.get(this.active)?.attach(null);
      this.active = active && this.docs.has(active) ? active : null;
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
  }

  private docChanged(doc: Doc) {
    if (doc.path === this.active && !doc.view && doc.state) {
      const focused = this.view.hasFocus;
      doc.attach(this.view);
      this.afterShow(doc);
      if (focused) this.view.focus();
    } else if (doc.view) this.applyReveal(doc);
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
    };
  }

  get activePath() {
    return this.active;
  }

  focus() {
    this.view.focus();
  }

  /** Write the active file; `force` overwrites outside changes. */
  async save(force: boolean, path = this.active) {
    const doc = path ? this.docs.get(path) : undefined;
    if (!doc?.buffer) return;
    doc.push();
    const result = await editorConnection.save(doc.buffer, force);
    if (result.conflict) this.conflicts.add(doc.path);
    else this.conflicts.delete(doc.path);
    if (result.error && !result.conflict) this.events.error(result.error);
    this.events.changed();
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
    for (const doc of this.docs.values()) {
      doc.attach(null);
      doc.close();
    }
    this.docs.clear();
    this.view.destroy();
  }
}
