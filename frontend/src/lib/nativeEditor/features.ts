// Language server features beyond @codemirror/lsp-client's: highlights,
// inlay hints, semantic tokens, code lenses, folding, selection ranges,
// range and on-type formatting, code actions, symbols, and hierarchies.
import { LSPPlugin } from '@codemirror/lsp-client';
import {
  EditorSelection,
  Prec,
  RangeSetBuilder,
  StateEffect,
  StateField,
  type Extension,
  type Text,
} from '@codemirror/state';
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
  keymap,
  type DecorationSet,
  type ViewUpdate,
} from '@codemirror/view';
import { foldService, getIndentUnit, indentUnit } from '@codemirror/language';
import { useStore } from '../../state/store';
import { editorConnection } from './connection';
import { viewDocs } from './doc';
import { openPicker, type PickItem } from './picker';

type Position = { line: number; character: number };
type Range = { start: Position; end: Position };
type Location = { uri: string; range: Range };
type Command = { title: string; command: string; arguments?: unknown[] };

/** A server-sent `$/btmux/refresh` (semantic tokens, inlay hints, code lenses). */
export const refreshEffect = StateEffect.define<string>();
/** A `refreshEffect` every requester takes: the client (re)initialized. */
export const REFRESH_ALL = 'initialized';

const plugin = (view: EditorView) => LSPPlugin.get(view);
export const has = (p: LSPPlugin, capability: string) =>
  !!(p.client.serverCapabilities as Record<string, unknown> | null)?.[capability];

function offset(doc: Text, pos: Position) {
  if (pos.line >= doc.lines) return doc.length;
  const line = doc.line(pos.line + 1);
  return Math.min(line.from + pos.character, line.to);
}

const toast = (message: string, error?: unknown) =>
  useStore.getState().showToast(message, 'error', {
    body:
      error === undefined
        ? undefined
        : error instanceof Error
          ? error.message
          : String((error as any)?.message ?? error),
  });

/** Ask the server; `null` when the doc changed meanwhile (the result is stale). */
export async function ask<T>(view: EditorView, method: string, params: (p: LSPPlugin) => object): Promise<T | null> {
  const p = plugin(view);
  if (!p) return null;
  p.client.sync();
  const doc = view.state.doc;
  const result = await p.client.request<object, T>(method, { textDocument: { uri: p.uri }, ...params(p) });
  return view.state.doc === doc ? result : null;
}

/** Re-ask on changes (debounced), on the viewport for `viewport`, and on refreshes. */
function requester(
  delay: number,
  run: (view: EditorView) => Promise<void>,
  opts: { viewport?: boolean; refresh?: string } = {},
) {
  return ViewPlugin.fromClass(
    class {
      timer: ReturnType<typeof setTimeout> | undefined;
      constructor(readonly view: EditorView) {
        this.schedule(50);
      }
      schedule(ms = delay) {
        clearTimeout(this.timer);
        this.timer = setTimeout(() => void run(this.view).catch(() => {}), ms);
      }
      update(u: ViewUpdate) {
        const refreshed = u.transactions.some((tr) =>
          tr.effects.some(
            (e) => e.is(refreshEffect) && (!opts.refresh || e.value === opts.refresh || e.value === REFRESH_ALL),
          ),
        );
        if (u.docChanged || refreshed || (opts.viewport && u.viewportChanged)) this.schedule();
      }
      destroy() {
        clearTimeout(this.timer);
      }
    },
  );
}

/** A decoration set replaced by `set` and mapped through changes. */
function decorationField(provide: (f: StateField<DecorationSet>) => Extension = (f) => EditorView.decorations.from(f)) {
  const set = StateEffect.define<DecorationSet>();
  const field = StateField.define<DecorationSet>({
    create: () => Decoration.none,
    update: (value, tr) => {
      value = value.map(tr.changes);
      for (const e of tr.effects) if (e.is(set)) value = e.value;
      return value;
    },
    provide,
  });
  return { set, field };
}

// Document highlights: other uses of the symbol under the cursor.
const highlights = decorationField();
const highlightMark = Decoration.mark({ class: 'cm-lsp-highlight' });
const highlightPlugin = ViewPlugin.fromClass(
  class {
    timer: ReturnType<typeof setTimeout> | undefined;
    constructor(readonly view: EditorView) {}
    update(u: ViewUpdate) {
      if (!u.selectionSet && !u.docChanged) return;
      clearTimeout(this.timer);
      this.timer = setTimeout(() => void this.run().catch(() => {}), 250);
    }
    async run() {
      const view = this.view;
      const p = plugin(view);
      if (!p || !has(p, 'documentHighlightProvider')) return;
      const head = view.state.selection.main.head;
      const result = await ask<{ range: Range }[]>(view, 'textDocument/documentHighlight', (p) => ({
        position: p.toPosition(head),
      }));
      const builder = new RangeSetBuilder<Decoration>();
      const doc = view.state.doc;
      const ranges = (result ?? [])
        .map((h) => [offset(doc, h.range.start), offset(doc, h.range.end)] as const)
        .filter(([from, to]) => to > from)
        .sort((a, b) => a[0] - b[0]);
      for (const [from, to] of ranges) builder.add(from, to, highlightMark);
      view.dispatch({ effects: highlights.set.of(builder.finish()) });
    }
    destroy() {
      clearTimeout(this.timer);
    }
  },
);

// Inlay hints for the visible lines.
class HintWidget extends WidgetType {
  constructor(
    readonly text: string,
    readonly left: boolean,
    readonly right: boolean,
  ) {
    super();
  }
  eq(other: HintWidget) {
    return other.text === this.text && other.left === this.left && other.right === this.right;
  }
  toDOM() {
    const span = document.createElement('span');
    span.className = 'cm-lsp-inlay-hint';
    span.textContent = `${this.left ? ' ' : ''}${this.text}${this.right ? ' ' : ''}`;
    return span;
  }
}
const hints = decorationField();
const hintRequester = requester(
  300,
  async (view) => {
    const p = plugin(view);
    if (!p || !has(p, 'inlayHintProvider')) return;
    const { from, to } = view.viewport;
    const result = await ask<
      { position: Position; label: string | { value: string }[]; paddingLeft?: boolean; paddingRight?: boolean }[]
    >(view, 'textDocument/inlayHint', (p) => ({ range: { start: p.toPosition(from), end: p.toPosition(to) } }));
    if (!result) return;
    const doc = view.state.doc;
    const widgets = result
      .map((h) => {
        const text = typeof h.label === 'string' ? h.label : h.label.map((part) => part.value).join('');
        return {
          at: offset(doc, h.position),
          deco: Decoration.widget({ widget: new HintWidget(text, !!h.paddingLeft, !!h.paddingRight), side: 1 }),
        };
      })
      .sort((a, b) => a.at - b.at);
    const builder = new RangeSetBuilder<Decoration>();
    for (const w of widgets) builder.add(w.at, w.at, w.deco);
    view.dispatch({ effects: hints.set.of(builder.finish()) });
  },
  { viewport: true, refresh: 'workspace/inlayHint/refresh' },
);

/** Decode `semanticTokens/full` data into sorted [from, to, classes]. */
export function decodeTokens(
  doc: Text,
  data: number[],
  legend: { tokenTypes: string[]; tokenModifiers: string[] },
): [number, number, string][] {
  const out: [number, number, string][] = [];
  let line = 0;
  let char = 0;
  for (let i = 0; i + 4 < data.length; i += 5) {
    const [deltaLine, deltaStart, length, type, modifiers] = data.slice(i, i + 5);
    line += deltaLine;
    char = deltaLine ? deltaStart : char + deltaStart;
    const from = offset(doc, { line, character: char });
    const to = offset(doc, { line, character: char + length });
    const name = legend.tokenTypes[type];
    if (!name || to <= from) continue;
    let classes = `cm-sem-${name}`;
    legend.tokenModifiers.forEach((m, bit) => {
      if (modifiers & (1 << bit)) classes += ` cm-sem-m-${m}`;
    });
    out.push([from, to, classes]);
  }
  return out;
}

// Semantic tokens, beneath the parser's highlighting so they color the text.
const semantic = decorationField((f) => Prec.lowest(EditorView.decorations.from(f)));
const semanticRequester = requester(
  400,
  async (view) => {
    const p = plugin(view);
    const provider = (p?.client.serverCapabilities as any)?.semanticTokensProvider;
    if (!p || !provider?.full) return;
    const result = await ask<{ data: number[] }>(view, 'textDocument/semanticTokens/full', () => ({}));
    if (!result) return;
    const builder = new RangeSetBuilder<Decoration>();
    for (const [from, to, classes] of decodeTokens(view.state.doc, result.data, provider.legend))
      builder.add(from, to, Decoration.mark({ class: classes }));
    view.dispatch({ effects: semantic.set.of(builder.finish()) });
  },
  { refresh: 'workspace/semanticTokens/refresh' },
);

// Code lenses, as lines above the code.
async function execute(view: EditorView, command: Command) {
  const p = plugin(view);
  if (!p) return;
  try {
    await p.client.request('workspace/executeCommand', { command: command.command, arguments: command.arguments });
  } catch (error) {
    toast(`${command.title} failed`, error);
  }
}

class LensWidget extends WidgetType {
  constructor(
    readonly commands: Command[],
    readonly indent: string,
  ) {
    super();
  }
  eq(other: LensWidget) {
    return (
      other.indent === this.indent &&
      JSON.stringify(other.commands.map((c) => c.title)) === JSON.stringify(this.commands.map((c) => c.title))
    );
  }
  toDOM(view: EditorView) {
    const div = document.createElement('div');
    div.className = 'cm-lsp-code-lens';
    div.textContent = this.indent;
    this.commands.forEach((command, i) => {
      if (i) div.append(' | ');
      const button = document.createElement('button');
      button.textContent = command.title;
      button.addEventListener('mousedown', (e) => e.preventDefault());
      button.addEventListener('click', () => void execute(view, command));
      div.append(button);
    });
    return div;
  }
  ignoreEvent() {
    return true;
  }
}
const lenses = decorationField();
const lensRequester = requester(
  600,
  async (view) => {
    const p = plugin(view);
    const provider = (p?.client.serverCapabilities as any)?.codeLensProvider;
    if (!p || !provider) return;
    let result = await ask<{ range: Range; command?: Command; data?: unknown }[]>(
      view,
      'textDocument/codeLens',
      () => ({}),
    );
    if (!result) return;
    if (provider.resolveProvider)
      result = await Promise.all(
        result.map((lens) =>
          lens.command ? lens : p.client.request<object, typeof lens>('codeLens/resolve', lens).catch(() => lens),
        ),
      );
    const doc = view.state.doc;
    const byLine = new Map<number, Command[]>();
    for (const lens of result) {
      if (!lens.command?.title) continue;
      const line = doc.line(Math.min(lens.range.start.line + 1, doc.lines));
      byLine.set(line.from, [...(byLine.get(line.from) ?? []), lens.command]);
    }
    const builder = new RangeSetBuilder<Decoration>();
    for (const at of [...byLine.keys()].sort((a, b) => a - b)) {
      const indent = /^\s*/.exec(doc.lineAt(at).text)![0];
      builder.add(
        at,
        at,
        Decoration.widget({ widget: new LensWidget(byLine.get(at)!, indent), block: true, side: -1 }),
      );
    }
    view.dispatch({ effects: lenses.set.of(builder.finish()) });
  },
  { refresh: 'workspace/codeLens/refresh' },
);

// Folding ranges from the server, besides the parser's.
const setFolds = StateEffect.define<{ from: number; to: number }[]>();
/** For `foldGutter`, which redraws only on its own triggers. */
export const serverFoldsChanged = (u: ViewUpdate) =>
  u.transactions.some((tr) => tr.effects.some((e) => e.is(setFolds)));
const foldsField = StateField.define<{ from: number; to: number }[]>({
  create: () => [],
  update: (value, tr) => {
    for (const e of tr.effects) if (e.is(setFolds)) return e.value;
    if (!tr.docChanged) return value;
    return value.map(({ from, to }) => ({ from: tr.changes.mapPos(from), to: tr.changes.mapPos(to) }));
  },
});
const foldRequester = requester(800, async (view) => {
  const p = plugin(view);
  if (!p || !has(p, 'foldingRangeProvider')) return;
  const result = await ask<{ startLine: number; endLine: number; startCharacter?: number }[]>(
    view,
    'textDocument/foldingRange',
    () => ({}),
  );
  if (!result) return;
  const doc = view.state.doc;
  const folds = result
    .filter((r) => r.endLine > r.startLine && r.endLine < doc.lines)
    .map((r) => {
      const start = doc.line(r.startLine + 1);
      return {
        from: r.startCharacter !== undefined ? Math.min(start.from + r.startCharacter, start.to) : start.to,
        to: doc.line(r.endLine + 1).to,
      };
    });
  view.dispatch({ effects: setFolds.of(folds) });
});
const serverFolds = foldService.of((state, lineStart, lineEnd) => {
  const fold = state.field(foldsField, false)?.find((f) => f.from >= lineStart && f.from <= lineEnd && f.to > lineEnd);
  return fold ?? null;
});

// Selection ranges: grow the selection to the next enclosing syntax node.
async function expandSelection(view: EditorView) {
  const p = plugin(view);
  if (!p || !has(p, 'selectionRangeProvider')) return false;
  const { from, to, head } = view.state.selection.main;
  type SelectionRange = { range: Range; parent?: SelectionRange };
  const result = await ask<SelectionRange[]>(view, 'textDocument/selectionRange', (p) => ({
    positions: [p.toPosition(head)],
  }));
  const doc = view.state.doc;
  for (let r = result?.[0]; r; r = r.parent) {
    const start = offset(doc, r.range.start);
    const end = offset(doc, r.range.end);
    if (start <= from && end >= to && end - start > to - from) {
      view.dispatch({ selection: EditorSelection.range(start, end), userEvent: 'select' });
      return true;
    }
  }
  return false;
}

// Range and on-type formatting.
type TextEdit = { range: Range; newText: string };
const formatOptions = (view: EditorView) => ({
  tabSize: getIndentUnit(view.state),
  insertSpaces: !view.state.facet(indentUnit).includes('\t'),
});

function applyEdits(view: EditorView, edits: TextEdit[] | null, userEvent: string) {
  if (!edits?.length) return;
  const doc = view.state.doc;
  const changes = edits.map((e) => ({
    from: offset(doc, e.range.start),
    to: offset(doc, e.range.end),
    insert: e.newText,
  }));
  view.dispatch({ changes, userEvent });
}

async function formatSelection(view: EditorView) {
  const p = plugin(view);
  const { from, to } = view.state.selection.main;
  if (!p || from === to || !has(p, 'documentRangeFormattingProvider')) return false;
  const edits = await ask<TextEdit[]>(view, 'textDocument/rangeFormatting', (p) => ({
    range: { start: p.toPosition(from), end: p.toPosition(to) },
    options: formatOptions(view),
  }));
  applyEdits(view, edits, 'format');
  return true;
}

const onTypeFormatting = EditorView.updateListener.of((u) => {
  const p = plugin(u.view);
  const provider = (p?.client.serverCapabilities as any)?.documentOnTypeFormattingProvider;
  if (!p || !provider || !u.transactions.some((tr) => tr.isUserEvent('input.type'))) return;
  const triggers = [provider.firstTriggerCharacter, ...(provider.moreTriggerCharacter ?? [])];
  let typed: { ch: string; at: number } | null = null;
  u.changes.iterChanges((_fa, _ta, _fb, toB, inserted) => {
    const text = inserted.toString();
    if (triggers.includes(text.slice(-1))) typed = { ch: text.slice(-1), at: toB };
  });
  if (!typed) return;
  const { ch, at } = typed as { ch: string; at: number };
  void ask<TextEdit[]>(u.view, 'textDocument/onTypeFormatting', (p) => ({
    position: p.toPosition(at),
    ch,
    options: formatOptions(u.view),
  }))
    .then((edits) => applyEdits(u.view, edits, 'format'))
    .catch(() => {});
});

// Code actions.
type CodeAction = {
  title: string;
  kind?: string;
  edit?: unknown;
  command?: Command;
  disabled?: unknown;
  isPreferred?: boolean;
  data?: unknown;
};

async function applyWorkspaceEdit(view: EditorView, edit: unknown, version: number | null) {
  const doc = viewDocs.get(view);
  const result = await editorConnection.applyEdit(edit, doc?.buffer ?? null, version);
  if (result.error) toast('Could not apply the change', result.error);
}

function codeActions(view: EditorView) {
  const p = plugin(view);
  if (!p || !has(p, 'codeActionProvider')) return false;
  const doc = viewDocs.get(view);
  const { from, to } = view.state.selection.main;
  const context = { diagnostics: lspDiagnostics(p, from, to), triggerKind: 1 };
  void (async () => {
    p.client.sync();
    const version = doc?.version() ?? null;
    const actions = await ask<(CodeAction | Command)[]>(view, 'textDocument/codeAction', (p) => ({
      range: { start: p.toPosition(from), end: p.toPosition(to) },
      context,
    }));
    const usable = (actions ?? []).filter((a) => !('disabled' in a && a.disabled));
    if (!usable.length) {
      useStore.getState().showToast('No code actions here', 'info');
      return;
    }
    const resolve = (p.client.serverCapabilities as any)?.codeActionProvider?.resolveProvider;
    openPicker(view, {
      title: 'Code actions',
      items: usable.map((action) => ({
        label: action.title,
        detail: 'kind' in action && action.kind ? action.kind : undefined,
        run: async () => {
          try {
            if (typeof (action as Command).command === 'string') return void execute(view, action as Command);
            let a = action as CodeAction;
            if (!a.edit && !a.command && resolve) a = await p.client.request('codeAction/resolve', a);
            if (a.edit) await applyWorkspaceEdit(view, a.edit, version);
            if (a.command) await execute(view, a.command);
          } catch (error) {
            toast(`${action.title} failed`, error);
          }
        },
      })),
    });
  })().catch((error) => toast('Code actions failed', error));
  return true;
}

/** The server's own diagnostics overlapping from..to, for code action context. */
let rawDiagnostics: (p: LSPPlugin) => { range: Range }[] = () => [];
export function setRawDiagnostics(fn: typeof rawDiagnostics) {
  rawDiagnostics = fn;
}
function lspDiagnostics(p: LSPPlugin, from: number, to: number) {
  const doc = p.view.state.doc;
  return rawDiagnostics(p).filter((d) => offset(doc, d.range.start) <= to && offset(doc, d.range.end) >= from);
}

// Jumps, symbols, and hierarchies.
async function jump(view: EditorView, location: Location) {
  const p = plugin(view);
  if (!p) return;
  const target = location.uri === p.uri ? view : await p.client.workspace.displayFile(location.uri);
  if (!target) return;
  const at = offset(target.state.doc, location.range.start);
  target.dispatch({ selection: { anchor: at }, effects: EditorView.scrollIntoView(at, { y: 'center' }) });
  target.focus();
}

const SYMBOL_KINDS = [
  '',
  'file',
  'module',
  'namespace',
  'package',
  'class',
  'method',
  'property',
  'field',
  'constructor',
  'enum',
  'interface',
  'function',
  'variable',
  'constant',
  'string',
  'number',
  'boolean',
  'array',
  'object',
  'key',
  'null',
  'enum member',
  'struct',
  'event',
  'operator',
  'type parameter',
];

type DocumentSymbol = {
  name: string;
  detail?: string;
  kind: number;
  range: Range;
  selectionRange: Range;
  children?: DocumentSymbol[];
};
type SymbolInformation = { name: string; kind: number; location: Location; containerName?: string };

function documentSymbols(view: EditorView) {
  const p = plugin(view);
  if (!p || !has(p, 'documentSymbolProvider')) return false;
  void ask<(DocumentSymbol | SymbolInformation)[]>(view, 'textDocument/documentSymbol', () => ({}))
    .then((symbols) => {
      const items: PickItem[] = [];
      const add = (s: DocumentSymbol | SymbolInformation, depth: number) => {
        const location = 'location' in s ? s.location : { uri: p.uri, range: s.selectionRange };
        items.push({
          label: `${'  '.repeat(depth)}${s.name}`,
          detail: SYMBOL_KINDS[s.kind],
          run: () => void jump(view, location),
        });
        if ('children' in s) for (const child of s.children ?? []) add(child, depth + 1);
      };
      for (const s of symbols ?? []) add(s, 0);
      openPicker(view, { title: 'Go to symbol in file', items });
    })
    .catch((error) => toast('Symbols failed', error));
  return true;
}

function workspaceSymbols(view: EditorView) {
  const p = plugin(view);
  if (!p || !has(p, 'workspaceSymbolProvider')) return false;
  const relative = (uri: string) => {
    const path = decodeURIComponent(new URL(uri).pathname);
    const root = viewDocs.get(view)?.path.split('/').slice(0, -1).join('/') ?? '';
    return path.startsWith(root) ? path.slice(root.length + 1) : path;
  };
  openPicker(view, {
    title: 'Go to symbol in workspace',
    query: async (query) => {
      const symbols = await p.client.request<
        object,
        { name: string; kind: number; location: { uri: string; range?: Range } }[] | null
      >('workspace/symbol', { query });
      return (symbols ?? []).slice(0, 200).map((s) => ({
        label: s.name,
        detail: `${SYMBOL_KINDS[s.kind]}  ${relative(s.location.uri)}`,
        run: async () => {
          let location = s.location;
          if (!location.range)
            location = (await p.client.request<object, typeof s>('workspaceSymbol/resolve', s)).location;
          void jump(view, location as Location);
        },
      }));
    },
  });
  return true;
}

type HierarchyItem = { name: string; kind: number; uri: string; selectionRange: Range; detail?: string };

function hierarchy(kind: 'incoming' | 'outgoing' | 'supertypes' | 'subtypes') {
  return (view: EditorView) => {
    const p = plugin(view);
    const calls = kind === 'incoming' || kind === 'outgoing';
    if (!p || !has(p, calls ? 'callHierarchyProvider' : 'typeHierarchyProvider')) return false;
    const head = view.state.selection.main.head;
    void (async () => {
      const prepared = await ask<HierarchyItem[]>(
        view,
        calls ? 'textDocument/prepareCallHierarchy' : 'textDocument/prepareTypeHierarchy',
        (p) => ({ position: p.toPosition(head) }),
      );
      const item = prepared?.[0];
      if (!item) return useStore.getState().showToast('Nothing here to look up', 'info');
      const method = calls ? `callHierarchy/${kind}Calls` : `typeHierarchy/${kind}`;
      const result = await p.client.request<object, unknown[] | null>(method, { item });
      const items = (result ?? []).map((r) => {
        const entry = (
          kind === 'incoming' ? (r as any).from : kind === 'outgoing' ? (r as any).to : r
        ) as HierarchyItem;
        return {
          label: entry.name,
          detail: entry.detail ?? decodeURIComponent(new URL(entry.uri).pathname).split('/').pop(),
          run: () => void jump(view, { uri: entry.uri, range: entry.selectionRange }),
        };
      });
      const titles = {
        incoming: 'Calls to',
        outgoing: 'Calls from',
        supertypes: 'Supertypes of',
        subtypes: 'Subtypes of',
      };
      openPicker(view, { title: `${titles[kind]} ${item.name}`, items });
    })().catch((error) => toast('Lookup failed', error));
    return true;
  };
}

/** Commands for the btmux command palette, by action name. */
export const editorCommands: Record<string, (view: EditorView) => boolean | Promise<boolean>> = {
  'editor-code-actions': codeActions,
  'editor-document-symbols': documentSymbols,
  'editor-workspace-symbols': workspaceSymbols,
  'editor-incoming-calls': hierarchy('incoming'),
  'editor-outgoing-calls': hierarchy('outgoing'),
  'editor-supertypes': hierarchy('supertypes'),
  'editor-subtypes': hierarchy('subtypes'),
  'editor-expand-selection': expandSelection,
};

export const lspFeatures: Extension = [
  highlights.field,
  highlightPlugin,
  hints.field,
  hintRequester,
  semantic.field,
  semanticRequester,
  lenses.field,
  lensRequester,
  foldsField,
  foldRequester,
  serverFolds,
  onTypeFormatting,
  // Ahead of the default keymap's Mod-i and lsp-client's Shift-Alt-f.
  Prec.high(
    keymap.of([
      { key: 'Mod-.', run: codeActions, preventDefault: true },
      { key: 'Mod-Shift-o', run: documentSymbols, preventDefault: true },
      { key: 'Mod-Alt-o', run: workspaceSymbols, preventDefault: true },
      { key: 'Shift-Alt-h', run: hierarchy('incoming'), preventDefault: true },
      { key: 'Mod-i', run: (view) => (void expandSelection(view), true) },
      {
        key: 'Shift-Alt-f',
        run: (view) => {
          if (view.state.selection.main.empty) return false;
          void formatSelection(view);
          return true;
        },
      },
    ]),
  ),
  EditorView.theme({
    '.cm-lsp-highlight': { backgroundColor: 'color-mix(in srgb, var(--btm-editor-fg) 12%, transparent)' },
    '.cm-lsp-inlay-hint': {
      opacity: 0.6,
      fontSize: '90%',
      padding: '0 2px',
      borderRadius: '3px',
      backgroundColor: 'color-mix(in srgb, var(--btm-editor-fg) 8%, transparent)',
    },
    '.cm-lsp-code-lens': { opacity: 0.65, fontSize: '85%', whiteSpace: 'pre' },
    '.cm-lsp-code-lens button': {
      font: 'inherit',
      color: 'inherit',
      background: 'none',
      border: 'none',
      padding: 0,
      cursor: 'pointer',
    },
    '.cm-lsp-code-lens button:hover': { textDecoration: 'underline' },
    '.cm-sem-m-deprecated': { textDecoration: 'line-through' },
  }),
];
