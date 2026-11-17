// Inline completions (`textDocument/inlineCompletion`): a server's suggestion
// as ghost text at the cursor, asked for once typing pauses (or with Alt-\).
// Typing what it suggests keeps it; Tab accepts it, Mod-Right its next word,
// Alt-] and Alt-[ cycle through the server's items, and Esc dismisses it.
import { snippet } from '@codemirror/autocomplete';
import { LSPPlugin } from '@codemirror/lsp-client';
import { Prec, StateEffect, StateField, type EditorState, type Extension, type Text } from '@codemirror/state';
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
  keymap,
  type DecorationSet,
  type ViewUpdate,
} from '@codemirror/view';
import { execute } from './features';

type Position = { line: number; character: number };
type Range = { start: Position; end: Position };
type Command = { title: string; command: string; arguments?: unknown[] };
type Item = {
  insertText: string | { kind: 'snippet'; value: string };
  filterText?: string;
  range?: Range;
  command?: Command;
};

/** An item where it applies: replacing `from`..`to`, `text` shown as typed. */
type Suggestion = { from: number; to: number; text: string; filter: string; snippet: string | null; command?: Command };
type Ghost = { suggestions: Suggestion[]; index: number };

/** How long typing pauses before the server is asked. */
const DELAY_MS = 300;

/** Snippet text as it reads: placeholders' defaults, no tab stops. */
const snippetText = (s: string) =>
  s
    .replace(/\$\{\d+:([^}]*)\}/g, '$1')
    .replace(/\$\{\d+\}|\$\d+/g, '')
    .replace(/\\([$}\\])/g, '$1');

const lspToSnippet = (text: string) => text.replace(/\\([$}\\])|\$(\d+)/g, (_m, esc, field) => esc || `\${${field}}`);

function offset(doc: Text, pos: Position) {
  if (pos.line >= doc.lines) return doc.length;
  const line = doc.line(pos.line + 1);
  return Math.min(line.from + pos.character, line.to);
}

/** What `s` would still insert at the cursor, or null if it no longer applies. */
function remaining(state: EditorState, s: Suggestion): string | null {
  const { main } = state.selection;
  if (!main.empty || main.head < s.from || main.head > s.to || s.to > state.doc.length) return null;
  const typed = state.sliceDoc(s.from, main.head);
  if (!s.filter.startsWith(typed) || !s.text.startsWith(typed)) return null;
  let rest = s.text.slice(typed.length);
  // Text after the cursor it replaces (a closing bracket) shows as it is.
  const after = state.sliceDoc(main.head, s.to);
  if (after && rest.endsWith(after)) rest = rest.slice(0, rest.length - after.length);
  return rest ? rest : null;
}

const setGhost = StateEffect.define<Ghost | null>();

const ghostField = StateField.define<Ghost | null>({
  create: () => null,
  update(ghost, tr) {
    for (const e of tr.effects) if (e.is(setGhost)) return e.value;
    if (!ghost || (!tr.docChanged && !tr.selection)) return ghost;
    // Follow typing that matches it; anything else ends it.
    const suggestions = ghost.suggestions.map((s) => ({
      ...s,
      from: tr.changes.mapPos(s.from, -1),
      to: tr.changes.mapPos(s.to, 1),
    }));
    const current = suggestions[ghost.index];
    return remaining(tr.state, current) === null ? null : { suggestions, index: ghost.index };
  },
  provide: (f) =>
    EditorView.decorations.compute([f, 'selection', 'doc'], (state): DecorationSet => {
      const ghost = state.field(f);
      const text = ghost && remaining(state, ghost.suggestions[ghost.index]);
      if (!text) return Decoration.none;
      return Decoration.set([
        Decoration.widget({ widget: new GhostWidget(text), side: 1 }).range(state.selection.main.head),
      ]);
    }),
});

class GhostWidget extends WidgetType {
  constructor(readonly text: string) {
    super();
  }
  eq(other: GhostWidget) {
    return other.text === this.text;
  }
  toDOM() {
    const span = document.createElement('span');
    span.className = 'cm-inline-completion';
    span.textContent = this.text;
    return span;
  }
}

/** Ask for suggestions at the cursor; `explicit` when the user asked. */
async function request(view: EditorView, explicit: boolean, previous: { cancel(): void } | null) {
  const p = LSPPlugin.get(view);
  if (!p || !(p.client.serverCapabilities as Record<string, unknown> | null)?.inlineCompletionProvider) return null;
  const state = view.state;
  if (!state.selection.main.empty) return null;
  previous?.cancel();
  p.client.sync();
  const head = state.selection.main.head;
  const params = {
    textDocument: { uri: p.uri },
    position: p.toPosition(head),
    context: { triggerKind: explicit ? 1 : 2 },
  };
  const pending = p.client.request<object, { items: Item[] } | Item[] | null>('textDocument/inlineCompletion', params);
  const handle = { cancel: () => p.client.cancelRequest(params) };
  return {
    handle,
    done: pending.then(
      (result) => {
        // Typed or moved meanwhile: a new request follows, or nothing applies.
        if (!result || view.state.doc !== state.doc || view.state.selection.main.head !== head) return;
        if (LSPPlugin.get(view) !== p) return;
        const items = Array.isArray(result) ? result : result.items;
        const suggestions: Suggestion[] = items.map((item) => {
          const raw = item.insertText;
          const text = typeof raw === 'string' ? raw : snippetText(raw.value);
          return {
            from: item.range ? offset(state.doc, item.range.start) : head,
            to: item.range ? offset(state.doc, item.range.end) : head,
            text,
            filter: item.filterText ?? text,
            snippet: typeof raw === 'string' ? null : raw.value,
            command: item.command,
          };
        });
        const usable = suggestions.filter((s) => remaining(view.state, s) !== null);
        view.dispatch({ effects: setGhost.of(usable.length ? { suggestions: usable, index: 0 } : null) });
      },
      () => {},
    ),
  };
}

const requester = ViewPlugin.fromClass(
  class {
    timer: ReturnType<typeof setTimeout> | undefined;
    inFlight: { cancel(): void } | null = null;
    constructor(readonly view: EditorView) {}
    update(u: ViewUpdate) {
      const typed = u.transactions.some((tr) => tr.isUserEvent('input') || tr.isUserEvent('delete'));
      if (!typed) return;
      clearTimeout(this.timer);
      this.timer = setTimeout(() => this.ask(false), DELAY_MS);
    }
    ask(explicit: boolean) {
      void request(this.view, explicit, this.inFlight).then((r) => {
        if (!r) return;
        this.inFlight = r.handle;
        void r.done.then(() => {
          if (this.inFlight === r.handle) this.inFlight = null;
        });
      });
    }
    destroy() {
      clearTimeout(this.timer);
      this.inFlight?.cancel();
    }
  },
);

const current = (view: EditorView) => {
  const ghost = view.state.field(ghostField, false);
  return ghost ? ghost.suggestions[ghost.index] : null;
};

function accept(view: EditorView) {
  const s = current(view);
  if (!s || remaining(view.state, s) === null) return false;
  view.dispatch({ effects: setGhost.of(null) });
  if (s.snippet !== null) snippet(lspToSnippet(s.snippet))(view, null, s.from, s.to);
  else
    view.dispatch({
      changes: { from: s.from, to: s.to, insert: s.text },
      selection: { anchor: s.from + s.text.length },
      userEvent: 'input.complete',
      scrollIntoView: true,
    });
  if (s.command) void execute(view, s.command);
  return true;
}

/** Accept up to the end of the suggestion's next word. */
function acceptWord(view: EditorView) {
  const s = current(view);
  const rest = s && remaining(view.state, s);
  if (!rest) return false;
  const word = /^(\s*\w+|\s*[^\w\s]+|\s+)/.exec(rest)?.[0] ?? rest;
  const head = view.state.selection.main.head;
  view.dispatch({
    changes: { from: head, insert: word },
    selection: { anchor: head + word.length },
    userEvent: 'input.complete',
  });
  return true;
}

function cycle(step: number) {
  return (view: EditorView) => {
    const ghost = view.state.field(ghostField, false);
    if (!ghost || ghost.suggestions.length < 2) return !!ghost;
    const n = ghost.suggestions.length;
    for (let i = 1; i < n; i++) {
      const index = (ghost.index + step * i + n * n) % n;
      if (remaining(view.state, ghost.suggestions[index]) !== null) {
        view.dispatch({ effects: setGhost.of({ ...ghost, index }) });
        break;
      }
    }
    return true;
  };
}

const trigger = (view: EditorView) => {
  view.plugin(requester)?.ask(true);
  return true;
};

/** Inline completions from the view's language server. */
export const inlineCompletions: Extension = [
  ghostField,
  requester,
  // Ahead of indentation, snippet fields, and vim.
  Prec.highest(
    keymap.of([
      { key: 'Tab', run: accept },
      { key: 'Mod-ArrowRight', run: acceptWord },
      { key: 'Alt-]', run: cycle(1) },
      { key: 'Alt-[', run: cycle(-1) },
      { key: 'Alt-\\', run: trigger },
      {
        key: 'Escape',
        // Dismissed, and Esc still does what it does.
        run: (view) => {
          if (view.state.field(ghostField, false)) view.dispatch({ effects: setGhost.of(null) });
          return false;
        },
      },
    ]),
  ),
  EditorView.theme({
    '.cm-inline-completion': { opacity: 0.5, whiteSpace: 'pre', fontStyle: 'italic' },
  }),
];
