// Server completions, after @codemirror/lsp-client's `serverCompletionSource`,
// plus `completionItem/resolve`: documentation and detail for the selected
// item, and additional edits (auto-imports) once one is applied, also after
// a snippet. Label details show beside the label.
import {
  insertCompletionText,
  snippet,
  type Completion,
  type CompletionContext,
  type CompletionResult,
  type CompletionSource,
} from '@codemirror/autocomplete';
import { LSPPlugin, type WorkspaceMapping } from '@codemirror/lsp-client';
import { EditorState, MapMode, type Text } from '@codemirror/state';
import type { EditorView } from '@codemirror/view';

type Position = { line: number; character: number };
type Range = { start: Position; end: Position };
type TextEdit = { range: Range; newText: string };
type Markup = string | { kind: 'markdown' | 'plaintext'; value: string };

type Item = {
  label: string;
  labelDetails?: { detail?: string; description?: string };
  kind?: number;
  detail?: string;
  documentation?: Markup;
  sortText?: string;
  filterText?: string;
  insertText?: string;
  insertTextFormat?: number;
  textEdit?: { newText: string; range?: Range; insert?: Range };
  textEditText?: string;
  additionalTextEdits?: TextEdit[];
  commitCharacters?: string[];
  data?: unknown;
};

type List = {
  isIncomplete?: boolean;
  items: Item[];
  itemDefaults?: {
    commitCharacters?: string[];
    editRange?: Range | { insert: Range; replace: Range };
    insertTextFormat?: number;
    data?: unknown;
  };
};

const KIND_TYPES: Record<number, string> = {
  1: 'text',
  2: 'method',
  3: 'function',
  4: 'class',
  5: 'property',
  6: 'variable',
  7: 'class',
  8: 'interface',
  9: 'namespace',
  10: 'property',
  11: 'keyword',
  12: 'constant',
  13: 'constant',
  14: 'keyword',
  16: 'constant',
  20: 'constant',
  21: 'constant',
  22: 'class',
  25: 'type',
};

const textOf = (item: Item) => item.textEdit?.newText || item.textEditText || item.insertText || item.label;

const lspToSnippet = (text: string) => text.replace(/\\([$}\\])|\$(\d+)/g, (_m, esc, field) => esc || `\${${field}}`);

function triggerReason(plugin: LSPPlugin, context: CompletionContext) {
  if (context.explicit) return { triggerKind: 1 };
  const ch = context.state.sliceDoc(context.pos - 1, context.pos);
  const triggers = plugin.client.serverCapabilities?.completionProvider?.triggerCharacters ?? [];
  if (triggers.includes(ch)) return { triggerKind: 2, triggerCharacter: ch };
  return /[a-zA-Z_]/.test(ch) ? { triggerKind: 1 } : null;
}

/** Non-word prefixes of the items' text, which typing them shouldn't end the result on. */
function validFor(items: Item[]) {
  const step = Math.ceil(items.length / 50);
  const prefixes: string[] = [];
  for (let i = 0; i < items.length; i += step) {
    const text = textOf(items[i]);
    if (/^\w/.test(text)) continue;
    const prefix = /^[^\w]*/.exec(text)![0];
    if (!prefixes.includes(prefix)) prefixes.push(prefix);
  }
  if (!prefixes.length) return /^\w*$/;
  return new RegExp(`^(?:${prefixes.map((p) => p.replace(/[^\w\s]/g, '\\$&')).join('|')})?\\w*$`);
}

function resultRange(context: CompletionContext, list: List) {
  const first = list.items[0];
  if (!first) return { from: context.pos, to: context.pos };
  const defaults = list.itemDefaults?.editRange;
  const range = defaults
    ? 'insert' in defaults
      ? defaults.insert
      : defaults
    : (first.textEdit?.range ?? first.textEdit?.insert ?? null);
  if (!range) return context.state.wordAt(context.pos) ?? { from: context.pos, to: context.pos };
  const line = context.state.doc.lineAt(context.pos);
  return { from: line.from + range.start.character, to: line.from + range.end.character };
}

function offset(doc: Text, pos: Position) {
  if (pos.line < 0 || pos.line >= doc.lines) return null;
  const line = doc.line(pos.line + 1);
  return pos.character < 0 || pos.character > line.length ? null : line.from + pos.character;
}

/** The latest completion's mapping per view; older ones are released once no edits wait for them. */
const mappings = new WeakMap<EditorView, WorkspaceMapping>();
const waiting = new WeakSet<WorkspaceMapping>();

const source: CompletionSource = async (context) => {
  const view = context.view;
  const plugin = view && LSPPlugin.get(view);
  if (!view || !plugin) return null;
  const capabilities = plugin.client.serverCapabilities;
  if (capabilities && !capabilities.completionProvider) return null;
  const reason = triggerReason(plugin, context);
  if (!reason) return null;
  plugin.client.sync();
  // Follows edits from here on, for additional edits applied later.
  const old = mappings.get(view);
  if (old && !waiting.has(old)) old.destroy();
  const mapping = plugin.client.workspaceMapping();
  mappings.set(view, mapping);
  const start = plugin.syncedDoc;
  const uri = plugin.uri;
  const params = { textDocument: { uri }, position: plugin.toPosition(context.pos), context: reason };
  context.addEventListener('abort', () => plugin.client.cancelRequest(params));
  let result: List | Item[] | null;
  try {
    result = await plugin.client.request<object, List | Item[] | null>('textDocument/completion', params);
  } catch (error) {
    if ((error as { code?: number }).code === -32800) return null;
    throw error;
  }
  if (!result) return null;
  const list: List = Array.isArray(result) ? { items: result } : result;
  const defaults = list.itemDefaults;
  const resolvable = !!plugin.client.serverCapabilities?.completionProvider?.resolveProvider;
  const language = plugin.client.workspace.getFile(uri)?.languageId ?? '';

  const options = list.items.map((original): Completion => {
    const item: Item =
      defaults?.data !== undefined && original.data === undefined ? { ...original, data: defaults.data } : original;
    let resolved: Promise<Item> | null = null;
    const resolve = () =>
      (resolved ??=
        resolvable && (!item.documentation || !item.additionalTextEdits)
          ? plugin.client.request<Item, Item>('completionItem/resolve', item).then(
              (r) => ({ ...item, ...r }),
              () => item,
            )
          : Promise.resolve(item));
    const text = textOf(item);
    const snippetText = (item.insertTextFormat ?? defaults?.insertTextFormat) === 2;

    /** `edits`, in the document the request was made in, into this view if it still shows it. */
    const applyExtra = (edits: TextEdit[] | undefined) => {
      if (!edits?.length || LSPPlugin.get(view) !== plugin) return;
      const changed = mapping.getMapping(uri);
      const changes = [];
      for (const edit of edits) {
        let from = offset(start, edit.range.start);
        let to = offset(start, edit.range.end);
        if (from === null || to === null) continue;
        if (changed) {
          if (changed.touchesRange(from, to)) continue;
          from = changed.mapPos(from, 1, MapMode.TrackDel);
          to = changed.mapPos(to, -1, MapMode.TrackDel);
          if (from === null || to === null) continue;
        }
        changes.push({ from, to, insert: edit.newText });
      }
      if (changes.length) view.dispatch({ changes, userEvent: 'input.complete' });
    };

    const detail = item.labelDetails?.description ?? item.detail;
    return {
      label: item.filterText || item.label,
      displayLabel: item.label + (item.labelDetails?.detail ?? ''),
      type: item.kind ? KIND_TYPES[item.kind] : undefined,
      detail,
      sortText: item.sortText,
      commitCharacters:
        item.commitCharacters && item.commitCharacters !== defaults?.commitCharacters
          ? item.commitCharacters
          : undefined,
      apply: (v, completion, from, to) => {
        if (snippetText) snippet(lspToSnippet(text))(v, completion, from, to);
        else v.dispatch(insertCompletionText(v.state, text, from, to));
        // Already there, or once resolved.
        if (item.additionalTextEdits) applyExtra(item.additionalTextEdits);
        else if (resolvable) {
          waiting.add(mapping);
          void resolve()
            .then((r) => applyExtra(r.additionalTextEdits))
            .finally(() => {
              waiting.delete(mapping);
              if (mappings.get(view) !== mapping) mapping.destroy();
            });
        }
      },
      info:
        item.documentation || resolvable
          ? async () => {
              const r = await resolve();
              let html = '';
              // A signature the list didn't show.
              if (r.detail && r.detail !== detail)
                html += plugin.docToHTML({ kind: 'markdown', value: '```' + language + '\n' + r.detail + '\n```' });
              if (r.documentation) html += plugin.docToHTML(r.documentation);
              if (!html) return null;
              const dom = document.createElement('div');
              dom.className = 'cm-lsp-documentation cm-lsp-completion-documentation';
              dom.innerHTML = html;
              return dom;
            }
          : undefined,
    };
  });

  const completions: CompletionResult = {
    ...resultRange(context, list),
    options,
    commitCharacters: defaults?.commitCharacters,
    validFor: list.isIncomplete ? undefined : validFor(list.items),
  };
  return completions;
};

/** Completions from the view's language server. */
export const serverCompletions = EditorState.languageData.of(() => [{ autocomplete: source }]);
