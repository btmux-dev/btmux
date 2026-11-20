// Server completions, after @codemirror/lsp-client's `serverCompletionSource`,
// plus `completionItem/resolve`: documentation and detail for the selected
// item, and additional edits (auto-imports) once one is applied, also after
// a snippet and into a file no longer shown. Label details show beside the label.
import {
  completionStatus,
  insertCompletionText,
  snippet,
  type Completion,
  type CompletionContext,
  type CompletionResult,
  type CompletionSource,
} from '@codemirror/autocomplete';
import { LSPPlugin, type LSPClient, type WorkspaceMapping } from '@codemirror/lsp-client';
import { ChangeSet, EditorState, StateEffect, StateField, type ChangeDesc, type Extension } from '@codemirror/state';
import { highlightingFor, language } from '@codemirror/language';
import { ViewPlugin, type EditorView } from '@codemirror/view';
import { highlightTree } from '@lezer/highlight';
import { viewDocs, type Doc } from './doc';
import { lspToSnippet, offsetChecked, type Range, type TextEdit } from './lspText';
import { pluginOf } from './servers';

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

/** LSP `CompletionItemKind` → completion type, one per kind for `KIND_ICONS` (theme.ts). */
const KIND_TYPES: Record<number, string> = {
  1: 'text',
  2: 'method',
  3: 'function',
  4: 'constructor',
  5: 'field',
  6: 'variable',
  7: 'class',
  8: 'interface',
  9: 'module',
  10: 'property',
  11: 'unit',
  12: 'value',
  13: 'enum',
  14: 'keyword',
  15: 'snippet',
  16: 'color',
  17: 'file',
  18: 'reference',
  19: 'folder',
  20: 'enumMember',
  21: 'constant',
  22: 'struct',
  23: 'event',
  24: 'operator',
  25: 'typeParameter',
};

const textOf = (item: Item) => item.textEdit?.newText || item.textEditText || item.insertText || item.label;

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

/** Whether `changes` replaced text in `from`..`to` or inserted strictly inside it. */
function overlaps(changes: ChangeDesc, from: number, to: number) {
  let hit = false;
  changes.iterChangedRanges((fromA, toA) => {
    if (fromA === toA ? fromA > from && fromA < to : fromA < to && toA > from) hit = true;
  });
  return hit;
}

/**
 * The latest completion request's mapping per view and server, from the
 * document it was made in to the one an option applies to. Released by the
 * server's next request, or when completion ends or the view shows another state.
 */
const mappings = new WeakMap<EditorView, Map<LSPClient, WorkspaceMapping>>();

function release(view: EditorView, client?: LSPClient) {
  const byClient = mappings.get(view);
  for (const [c, mapping] of byClient ?? []) {
    if (client && c !== client) continue;
    mapping.destroy();
    byClient!.delete(c);
  }
}

const releaseMappings = ViewPlugin.define((view) => ({
  update(u) {
    if (mappings.get(view)?.size && !completionStatus(u.state)) release(view);
  },
  destroy() {
    release(view);
  },
}));

/** Changes since applied options, by id, until their additional edits arrive: shown or not. */
const track = StateEffect.define<{ id: number; on: boolean }>();
const tracked = StateField.define<Map<number, ChangeDesc>>({
  create: () => new Map(),
  update(value, tr) {
    let next = value;
    if (tr.docChanged && value.size) {
      next = new Map();
      for (const [id, changes] of value) next.set(id, changes.composeDesc(tr.changes.desc));
    }
    for (const e of tr.effects) {
      if (!e.is(track)) continue;
      if (next === value) next = new Map(value);
      if (e.value.on) next.set(e.value.id, ChangeSet.empty(tr.state.doc.length).desc);
      else next.delete(e.value.id);
    }
    return next;
  },
});
let nextTrack = 0;

/** Completions from the server whose plugin `pick` finds. */
const sourceFrom =
  (pick: (view: EditorView) => LSPPlugin | null): CompletionSource =>
  async (context) => {
    const view = context.view;
    const plugin = view && pick(view);
    if (!view || !plugin) return null;
    const capabilities = plugin.client.serverCapabilities;
    if (capabilities && !capabilities.completionProvider) return null;
    const reason = triggerReason(plugin, context);
    if (!reason) return null;
    plugin.client.sync();
    // Follows edits from here on, for additional edits applied later.
    release(view, plugin.client);
    const mapping = plugin.client.workspaceMapping();
    let byClient = mappings.get(view);
    if (!byClient) mappings.set(view, (byClient = new Map()));
    byClient.set(plugin.client, mapping);
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

    const options = list.items.map((original): ServerCompletion => {
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

      /**
       * `edits`, in the document the request was made in, into `doc`, changed
       * by `before` until the option applied and by what `id` tracked since.
       */
      const applyExtra = (doc: Doc, id: number, before: ChangeDesc, edits: TextEdit[] | undefined) => {
        const stop = track.of({ id, on: false });
        const since = doc.state?.field(tracked, false)?.get(id);
        // Closed, or reopened from scratch.
        if (doc.closed || !since) return;
        const changed = before.composeDesc(since);
        const changes = [];
        for (const edit of edits ?? []) {
          const from = offsetChecked(start, edit.range.start);
          const to = offsetChecked(start, edit.range.end);
          // Edits beside the completion (a postfix one's receiver) still apply.
          if (from === null || to === null || overlaps(changed, from, to)) continue;
          const at = changed.mapPos(from, 1);
          changes.push({ from: at, to: at + (to - from), insert: edit.newText });
        }
        doc.update({ changes, effects: stop, userEvent: 'input.complete' });
      };

      const detail = item.labelDetails?.description ?? item.detail;
      return {
        label: item.filterText || item.label,
        displayLabel: item.label + (item.labelDetails?.detail ?? ''),
        type: item.kind ? KIND_TYPES[item.kind] : undefined,
        code: detail,
        sortText: item.sortText,
        commitCharacters:
          item.commitCharacters && item.commitCharacters !== defaults?.commitCharacters
            ? item.commitCharacters
            : undefined,
        apply: (v, completion, from, to) => {
          // Additional edits come now or once resolved, maybe once another file
          // shows; positions map from the request through what's tracked from here.
          const pending = item.additionalTextEdits ? Promise.resolve(item) : resolvable ? resolve() : null;
          const doc = viewDocs.get(v);
          const before = mapping.getMapping(uri);
          const id = nextTrack++;
          if (pending && doc && before) v.dispatch({ effects: track.of({ id, on: true }) });
          if (snippetText) snippet(lspToSnippet(text))(v, completion, from, to);
          else v.dispatch(insertCompletionText(v.state, text, from, to));
          if (pending && doc && before) void pending.then((r) => applyExtra(doc, id, before, r.additionalTextEdits));
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

/** A signature or type in place of `detail`, which `codeDetail` highlights. */
type ServerCompletion = Completion & { code?: string };

/**
 * Highlight `text` with the buffer's language. Rust's are anonymous `fn(…)`
 * types, which parse as a function once named, as Zed does.
 */
function highlightCode(state: EditorState, text: string): HTMLElement {
  const dom = document.createElement('span');
  dom.className = 'cm-completionDetail cm-btmux-completionCode';
  const lang = state.facet(language);
  if (!lang) {
    dom.textContent = text;
    return dom;
  }
  const at = lang.name === 'rust' ? (/^[\w ]*\bfn(?=\()/.exec(text)?.[0].length ?? -1) : -1;
  const pad = at < 0 ? '' : ' f';
  const source = at < 0 ? text : text.slice(0, at) + pad + text.slice(at);
  const back = (pos: number) => (at < 0 || pos <= at ? pos : Math.max(at, pos - pad.length));
  let pos = 0;
  const add = (to: number, cls: string) => {
    if (to <= pos) return;
    const piece = text.slice(pos, to);
    if (cls) {
      const span = dom.appendChild(document.createElement('span'));
      span.className = cls;
      span.textContent = piece;
    } else dom.append(piece);
    pos = to;
  };
  highlightTree(lang.parser.parse(source), { style: (tags) => highlightingFor(state, tags) }, (from, to, cls) => {
    add(back(from), '');
    add(back(to), cls);
  });
  add(text.length, '');
  return dom;
}

/** Shows server completions' `code`. */
export const codeDetail = {
  render: (completion: Completion, state: EditorState) => {
    const code = (completion as ServerCompletion).code;
    return code ? highlightCode(state, code) : null;
  },
  position: 80,
};

const firstSource = sourceFrom((view) => LSPPlugin.get(view));

/** Completions from the view's first language server. */
export const serverCompletions = [
  EditorState.languageData.of(() => [{ autocomplete: firstSource }]),
  tracked,
  releaseMappings,
];

// One per client: CodeMirror tells results apart by source.
const otherSources = new WeakMap<LSPClient, Extension>();

/** Completions from another of its servers, besides `serverCompletions`. */
export function completionsFrom(client: LSPClient) {
  let found = otherSources.get(client);
  if (!found) {
    const source = sourceFrom((view) => pluginOf(view, client));
    otherSources.set(client, (found = EditorState.languageData.of(() => [{ autocomplete: source }])));
  }
  return found;
}
