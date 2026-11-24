import {
  closeReferencePanel,
  LSPClient,
  LSPPlugin,
  showSignatureHelp,
  signatureHelp,
  Workspace,
  type Transport,
  type WorkspaceFile,
} from '@codemirror/lsp-client';
import { EditorState, Facet, Prec, StateEffect, StateField, type ChangeSpec, type Extension } from '@codemirror/state';
import { EditorView, keymap, showPanel, type Panel, type Tooltip } from '@codemirror/view';
import { getIndentUnit, indentUnit, LanguageDescription } from '@codemirror/language';
import { setDiagnostics, type Diagnostic } from '@codemirror/lint';
import DOMPurify from 'dompurify';
import { useStore } from '../../state/store';
import type { Overlay } from '../../state/types';
import type { EditorServerMessage, LspServerStatus } from '../../generated/protocol';
import { IS_MAC } from '../nvimKeys';
import { completionsFrom, serverCompletions } from './completion';
import { languages } from './languages';
import { minimalChanges, offset, type Position, type Range, type TextEdit } from './lspText';
import { inlineCompletions } from './inlineCompletion';
import { editorConnection } from './connection';
import { viewDocs, type Doc } from './doc';
import {
  REFRESH_ALL,
  applyWorkspaceEdit,
  findReferences,
  jumpToDefinition,
  lspFeatures,
  refreshEffect,
  setRawDiagnostics,
} from './features';
import { mayHave, otherServers, pluginFor, pluginOf, pluginReady, plugins, serverIds } from './servers';

/** btmux's language server clients speak to the server through `/ws/editor`; it syncs documents itself. */

type File = WorkspaceFile & { view: EditorView };

/** Opens a file in the focused pane editor, for jumps to other files. */
let display: ((path: string) => Promise<EditorView | null>) | null = null;

export function setDisplay(fn: typeof display) {
  display = fn;
}

const pathOf = (uri: string) => {
  try {
    const path = decodeURIComponent(new URL(uri).pathname);
    // file:///C:/… on Windows.
    return /^\/[A-Za-z]:\//.test(path) ? path.slice(1) : path;
  } catch {
    return null;
  }
};

/** Set while one client's sync syncs the others. */
let syncing = false;

class BtmuxWorkspace extends Workspace {
  files: File[] = [];

  syncFiles() {
    // Every client syncs together, so a view's plugins all hold the same
    // unsynced changes: lsp-client maps positions with the first's.
    if (!syncing) {
      syncing = true;
      try {
        for (const c of clients.values()) if (c !== this.client) c.sync();
      } finally {
        syncing = false;
      }
    }
    const result = [];
    for (const file of this.files) {
      const doc = viewDocs.get(file.view);
      doc?.push();
      const plugin = pluginOf(file.view, this.client);
      if (plugin && !plugin.unsyncedChanges.empty) {
        result.push({ changes: plugin.unsyncedChanges, file, prevDoc: file.doc });
        file.doc = file.view.state.doc;
        plugin.clear();
      }
      file.version = doc?.version() ?? file.version;
    }
    return result;
  }

  openFile(uri: string, languageId: string, view: EditorView) {
    const doc = viewDocs.get(view);
    this.files.push({
      uri,
      languageId,
      version: doc?.version() ?? 0,
      doc: view.state.doc,
      view,
      getView: () => view,
    });
    // Diagnostics published while it was hidden.
    queueMicrotask(() => showDiagnostics(view, this.client));
  }

  closeFile(uri: string, view: EditorView) {
    this.files = this.files.filter((f) => f.uri !== uri || f.view !== view);
  }

  // btmux opens documents in the server itself.
  connected() {}

  updateFile(uri: string, update: Parameters<Workspace['updateFile']>[1]) {
    this.files.find((f) => f.uri === uri)?.view.dispatch(update);
  }

  async displayFile(uri: string) {
    const open = this.getFile(uri)?.getView();
    if (open?.hasFocus) return open;
    const path = pathOf(uri);
    return path && display ? display(path) : (open ?? null);
  }
}

type PublishParams = {
  uri: string;
  diagnostics: {
    range: { start: { line: number; character: number }; end: { line: number; character: number } };
    severity?: number;
    message: string;
    source?: string;
    code?: string | number;
    codeDescription?: { href: string };
    /** 1 unnecessary (unused), 2 deprecated. */
    tags?: number[];
    relatedInformation?: { location: { uri: string; range: Range }; message: string }[];
  }[];
};

const TAG_CLASSES: Record<number, string> = { 1: 'cm-lsp-unnecessary', 2: 'cm-lsp-deprecated' };

type ServerDiagnostic = PublishParams['diagnostics'][number];

/**
 * The message with the diagnostic's code, linked to its documentation if
 * given, and its related information, each linked to its location.
 */
function renderMessage(d: ServerDiagnostic) {
  const dom = document.createElement('span');
  dom.append(d.message);
  if (d.code !== undefined && d.code !== '') {
    const href = d.codeDescription?.href;
    const label = document.createElement(href && /^https?:/.test(href) ? 'a' : 'span');
    label.className = 'cm-lsp-diagnostic-code';
    label.textContent = `(${d.code})`;
    if (label instanceof HTMLAnchorElement) {
      label.href = href!;
      label.addEventListener('click', (e) => {
        e.preventDefault();
        window.open(href, '_blank', 'noopener');
      });
    }
    dom.append(' ', label);
  }
  if (d.relatedInformation?.length) {
    const list = document.createElement('ul');
    list.className = 'cm-lsp-related';
    for (const related of d.relatedInformation) {
      const item = document.createElement('li');
      const link = document.createElement('a');
      const name = pathOf(related.location.uri)?.split('/').pop() ?? related.location.uri;
      link.textContent = `${name}:${related.location.range.start.line + 1}`;
      link.href = '#';
      link.addEventListener('click', (e) => {
        e.preventDefault();
        reveal(related.location.uri, related.location.range.start);
      });
      item.append(link, ` ${related.message}`);
      list.append(item);
    }
    dom.append(list);
  }
  return dom;
}

/** Show `uri` in the focused pane editor, at `at`. */
function reveal(uri: string, at: Position | null) {
  const path = pathOf(uri);
  if (!path || !display) return;
  void display(path).then((view) => {
    if (!view || !at) return;
    const pos = offset(view.state.doc, at);
    view.dispatch({
      selection: { anchor: pos },
      effects: EditorView.scrollIntoView(pos, { y: 'center' }),
      userEvent: 'select.jump',
    });
    view.focus();
  });
}

/** The latest diagnostics per client and URI. */
const diagnostics = new Map<LSPClient, Map<string, PublishParams>>();

const diagnosticListeners = new Set<() => void>();

/** Calls `listener` when any server publishes diagnostics; returns the unsubscribe function. */
export function onDiagnostics(listener: () => void): () => void {
  diagnosticListeners.add(listener);
  return () => diagnosticListeners.delete(listener);
}

/** A diagnostic in any file. */
export interface FileDiagnostic {
  path: string;
  /** Zero-based, in UTF-16 code units, as LSP counts. */
  start: Position;
  /** 1 error, 2 warning, 3 info, 4 hint. */
  severity: number;
  message: string;
  source?: string;
  code?: string | number;
}

/** A shown diagnostic, with what its server sent. */
type LspDiagnostic = Diagnostic & { lsp?: ServerDiagnostic };

const fromServer = (path: string, d: ServerDiagnostic, start: Position): FileDiagnostic => ({
  path,
  start,
  // Unset is a hint, as `serverDiagnostics` shows it.
  severity: d.severity ?? 4,
  message: d.message,
  source: d.source,
  code: d.code,
});

/**
 * Every file's diagnostics from `servers` that haven't failed. In files with
 * a state (`stateOf`), at their positions mapped through edits since.
 */
export function fileDiagnostics(
  servers: Iterable<number>,
  stateOf: (path: string) => EditorState | null,
): FileDiagnostic[] {
  const found: FileDiagnostic[] = [];
  for (const server of new Set(servers)) {
    const c = clients.get(server);
    if (!c || serverFailed(server)) continue;
    for (const params of diagnostics.get(c)?.values() ?? []) {
      const path = pathOf(params.uri);
      if (!path) continue;
      const state = stateOf(path);
      const mapped = state?.field(serverDiagnosticsField, false)?.get(c) as LspDiagnostic[] | undefined;
      if (state && mapped) {
        for (const d of mapped) {
          if (!d.lsp) continue;
          const line = state.doc.lineAt(d.from);
          found.push(fromServer(path, d.lsp, { line: line.number - 1, character: d.from - line.from }));
        }
      } else for (const d of params.diagnostics) found.push(fromServer(path, d, d.range.start));
    }
  }
  return found;
}

const setServerDiagnostics = StateEffect.define<{ client: LSPClient; list: Diagnostic[] }>();
/** Drop the diagnostics of servers not in the set. */
const keepServerDiagnostics = StateEffect.define<ReadonlySet<LSPClient>>();

/**
 * Each server's diagnostics, mapped through edits since it published them: a
 * server answering an edit later keeps its ranges in place meanwhile.
 */
const serverDiagnosticsField = StateField.define<ReadonlyMap<LSPClient, Diagnostic[]>>({
  create: () => new Map(),
  update(value, tr) {
    let next = value;
    if (tr.docChanged && value.size) {
      const changes = tr.changes;
      next = new Map(
        [...value].map(([client, list]) => [
          client,
          list.flatMap((d) => {
            const from = changes.mapPos(d.from, 1);
            const to = Math.max(from, changes.mapPos(d.to, -1));
            // Its text was deleted, which lint drops it for too.
            return d.to > d.from && to === from ? [] : [{ ...d, from, to }];
          }),
        ]),
      );
    }
    for (const e of tr.effects) {
      if (e.is(setServerDiagnostics)) {
        if (next === value) next = new Map(value);
        (next as Map<LSPClient, Diagnostic[]>).set(e.value.client, e.value.list);
      } else if (e.is(keepServerDiagnostics)) {
        const keep = e.value;
        next = new Map([...next].filter(([client]) => keep.has(client)));
      }
    }
    return next;
  },
});

/**
 * Before `doc`'s servers are reconfigured: drop the diagnostics of servers
 * that failed since, which would otherwise stay until another publishes (or
 * for good, with none left). Null if there are none.
 */
export function dropFailedDiagnostics(doc: Doc) {
  const state = doc.state;
  const shown = state?.field(serverDiagnosticsField, false);
  if (!state || !shown) return null;
  const working = new Set(
    workingServers(doc)
      .map((s) => clients.get(s))
      .filter((c) => c !== undefined),
  );
  if ([...shown.keys()].every((c) => working.has(c))) return null;
  const kept = [...shown].filter(([c]) => working.has(c)).flatMap(([, list]) => list);
  return [setDiagnostics(state, kept), { effects: keepServerDiagnostics.of(working) }];
}

/** `client`'s latest diagnostics for the file `view` shows, with the other servers' as they are. */
function showDiagnostics(view: EditorView, client: LSPClient) {
  const plugin = pluginOf(view, client);
  if (!plugin) return;
  const params = diagnostics.get(client)?.get(plugin.uri);
  const list = params ? serverDiagnostics(plugin, params) : [];
  const live = new Set(plugins(view).map((p) => p.client));
  const others = [...(view.state.field(serverDiagnosticsField, false) ?? [])]
    .filter(([c]) => c !== client && live.has(c))
    .flatMap(([, list]) => list);
  view.dispatch(setDiagnostics(view.state, [...others, ...list]), {
    effects: setServerDiagnostics.of({ client, list }),
  });
}

function serverDiagnostics(plugin: LSPPlugin, params: PublishParams): Diagnostic[] {
  // Positions are in the synced document; map them over edits since.
  const synced = plugin.syncedDoc;
  const changes = plugin.unsyncedChanges;
  return params.diagnostics.flatMap((d) => {
    const start = offset(synced, d.range.start);
    const end = offset(synced, d.range.end);
    const from = changes.mapPos(start, 1);
    const to = Math.max(from, changes.mapPos(end, -1));
    // Its text was deleted since (a formatter replacing the file): left
    // in, it would sit where the deletion was.
    if (end > start && to === from) return [];
    const severity = d.severity === 1 ? 'error' : d.severity === 2 ? 'warning' : d.severity === 3 ? 'info' : 'hint';
    const markClass = (d.tags ?? [])
      .map((t) => TAG_CLASSES[t] ?? '')
      .join(' ')
      .trim();
    const code = d.code;
    const shown: LspDiagnostic = {
      lsp: d,
      from,
      to,
      severity,
      message: d.message,
      source: d.source,
      markClass: markClass || undefined,
      renderMessage:
        (code === undefined || code === '') && !d.relatedInformation?.length ? undefined : () => renderMessage(d),
    };
    return [shown];
  });
}

function transport(server: number, workspace: () => BtmuxWorkspace): Transport {
  const unsubscribe = new Map<(value: string) => void, () => void>();
  return {
    send(message: string) {
      const msg = JSON.parse(message);
      let buffer: string | null = null;
      let version: number | null = null;
      const uri = msg.params?.textDocument?.uri;
      // A request's positions refer to the document as pushed so far.
      if (msg.id !== undefined && msg.method && uri) {
        const file = workspace().files.find((f) => f.uri === uri);
        const doc = file && viewDocs.get(file.view);
        if (doc) {
          doc.push();
          buffer = doc.buffer;
          version = doc.version();
        }
      }
      if (!editorConnection.sendLsp(server, msg, buffer, version)) throw new Error('The editor is not connected');
    },
    subscribe(handler) {
      unsubscribe.set(handler, editorConnection.onLsp(server, handler));
    },
    unsubscribe(handler) {
      unsubscribe.get(handler)?.();
      unsubscribe.delete(handler);
    },
  };
}

/** What the rename panel renames: the symbol at `pos`, now named `name`. */
type RenameTarget = { pos: number; name: string };

const setRename = StateEffect.define<RenameTarget | null>();

const renameField = StateField.define<RenameTarget | null>({
  create: () => null,
  update: (target, tr) => {
    for (const e of tr.effects) if (e.is(setRename)) return e.value;
    return target && tr.docChanged ? { ...target, pos: tr.changes.mapPos(target.pos) } : target;
  },
  provide: (f) => showPanel.from(f, (target) => (target ? renamePanel : null)),
});

function renamePanel(view: EditorView): Panel {
  const form = document.createElement('form');
  form.className = 'cm-btmux-rename';
  const label = document.createElement('label');
  label.textContent = 'Rename to ';
  const input = document.createElement('input');
  input.name = 'name';
  input.value = view.state.field(renameField)?.name ?? '';
  input.setAttribute('aria-label', 'New name');
  label.append(input);
  form.append(label);
  const close = () => {
    view.dispatch({ effects: setRename.of(null) });
    view.focus();
  };
  form.addEventListener('submit', (e) => {
    e.preventDefault();
    const name = input.value.trim();
    const target = view.state.field(renameField);
    close();
    if (name && target) void rename(view, target.pos, name);
  });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
    }
  });
  return {
    dom: form,
    top: true,
    mount() {
      input.focus();
      input.select();
    },
  };
}

type PrepareRename = Range | { range: Range; placeholder: string } | { defaultBehavior: boolean } | null;

/** The word at `pos`, which servers without `prepareRename` (or deferring to the client) rename. */
function wordTarget(view: EditorView, pos: number): RenameTarget | null {
  const word = view.state.wordAt(pos);
  return word && { pos: word.from, name: view.state.sliceDoc(word.from, word.to) };
}

export const startRename = (view: EditorView) => {
  if (!mayHave(view, 'renameProvider')) return false;
  void pluginReady(view, 'renameProvider').then((plugin) => plugin && openRename(view, plugin));
  return true;
};

function openRename(view: EditorView, plugin: LSPPlugin) {
  const head = view.state.selection.main.head;
  const provider = plugin.client.serverCapabilities?.renameProvider;
  if (typeof provider === 'object' && provider.prepareProvider) {
    void prepareRename(view, plugin, head);
    return true;
  }
  const target = wordTarget(view, head);
  if (!target) return false;
  view.dispatch({ effects: setRename.of(target) });
  return true;
}

/** Ask the server what's renamed at `head` and under which name, then open the panel. */
async function prepareRename(view: EditorView, plugin: LSPPlugin, head: number) {
  plugin.client.sync();
  const start = view.state.doc;
  let result: PrepareRename;
  try {
    result = await plugin.client.request<object, PrepareRename>('textDocument/prepareRename', {
      textDocument: { uri: plugin.uri },
      position: plugin.toPosition(head),
    });
  } catch (error) {
    // Servers explain why in the error (a keyword, a library symbol).
    toast('Cannot rename this', error);
    return;
  }
  // Typed meanwhile, or showing another file.
  if (!plugins(view).includes(plugin) || view.state.doc !== start) return;
  let target: RenameTarget | null;
  if (!result) target = null;
  else if ('defaultBehavior' in result) target = wordTarget(view, head);
  else {
    const range = 'range' in result ? result.range : result;
    const from = offset(start, range.start);
    const to = offset(start, range.end);
    target = { pos: from, name: 'placeholder' in result ? result.placeholder : start.sliceString(from, to) };
  }
  if (!target) {
    useStore.getState().showToast('Nothing to rename here', 'info');
    return;
  }
  view.dispatch({ effects: setRename.of(target) });
}

const toast = (message: string, error: unknown) =>
  useStore.getState().showToast(message, 'error', {
    body: error instanceof Error ? error.message : String((error as any)?.message ?? error),
  });

/** Rename the symbol at `pos` across files: the server applies the edit to every buffer, open or not. */
async function rename(view: EditorView, pos: number, newName: string) {
  const plugin = pluginFor(view, 'renameProvider');
  const doc = viewDocs.get(view);
  if (!plugin || !doc) return;
  plugin.client.sync();
  const version = doc.version();
  try {
    const edit = await plugin.client.request<object, Parameters<typeof applyWorkspaceEdit>[1] | null>(
      'textDocument/rename',
      { textDocument: { uri: plugin.uri }, position: plugin.toPosition(pos), newName },
    );
    if (edit) await applyWorkspaceEdit('Rename', edit, plugin, doc, version);
  } catch (error) {
    toast('Rename failed', error);
  }
}

/** Format with the first language server that can; resolves once applied. */
export async function format(view: EditorView) {
  const plugin = await pluginReady(view, 'documentFormattingProvider');
  const doc = viewDocs.get(view);
  if (!plugin || !doc) return;
  plugin.client.sync();
  const uri = plugin.uri;
  const start = view.state.doc;
  await plugin.client.withMapping(async (mapping) => {
    const edits = await plugin.client.request<unknown, TextEdit[] | null>('textDocument/formatting', {
      textDocument: { uri },
      options: {
        tabSize: getIndentUnit(view.state),
        insertSpaces: !view.state.facet(indentUnit).includes('\t'),
      },
    });
    if (!edits?.length) return;
    // The view is shared: it may show another file by now, or this one
    // through a new plugin, which the mapping doesn't follow.
    if (!plugins(view).includes(plugin)) {
      // Still the formatted text: apply it there, shown or not.
      if (doc.state?.doc !== start) return;
      const changes = edits.flatMap((e) =>
        minimalChanges(start, offset(start, e.range.start), offset(start, e.range.end), e.newText),
      );
      if (changes.length) doc.update({ changes, userEvent: 'format' });
      return;
    }
    const changed = mapping.getMapping(uri);
    const changes: ChangeSpec[] = [];
    for (const edit of edits) {
      let from = mapping.mapPosition(uri, edit.range.start);
      let to = mapping.mapPosition(uri, edit.range.end);
      if (changed) {
        // Typed into meanwhile: leave it.
        if (changed.touchesRange(from, to)) return;
        from = changed.mapPos(from, 1);
        to = changed.mapPos(to, -1);
      }
      changes.push(...minimalChanges(view.state.doc, from, to, edit.newText));
    }
    if (changes.length) view.dispatch({ changes, userEvent: 'format' });
  });
}

/** Instead of `formatKeymap`, whose command would format whatever file the view shows by then. */
export const formatCommand = (view: EditorView) => {
  if (!plugins(view).length) return false;
  format(view).catch((error) => toast('Formatting failed', error));
  return true;
};

const clients = new Map<number, LSPClient>();

function highlightLanguage(name: string) {
  const description = LanguageDescription.matchLanguageName(languages, name, true);
  if (!description) return null;
  if (!description.support) void description.load();
  return description.support?.language ?? null;
}

function client(server: number): LSPClient {
  let found = clients.get(server);
  if (found) return found;
  const created: LSPClient = new LSPClient({
    workspace: (c) => new BtmuxWorkspace(c),
    // Servers can be slow while they index.
    timeout: 15_000,
    sanitizeHTML: (html) => DOMPurify.sanitize(html),
    highlightLanguage,
    notificationHandlers: {
      'textDocument/publishDiagnostics': (c, params: PublishParams) => {
        let byUri = diagnostics.get(c);
        if (!byUri) diagnostics.set(c, (byUri = new Map()));
        byUri.set(params.uri, params);
        for (const file of (c.workspace as BtmuxWorkspace).files)
          if (file.uri === params.uri) showDiagnostics(file.view, c);
        for (const listener of diagnosticListeners) listener();
        return true;
      },
      'window/showMessage': (_, params: { type: number; message: string }) => {
        // Log and debug messages aren't for the user.
        if (params.type >= 4) return true;
        const level = params.type === 1 ? 'error' : params.type === 2 ? 'attention' : 'info';
        useStore.getState().showToast(params.message, level);
        return true;
      },
      'window/logMessage': () => true,
      '$/progress': () => true,
      '$/btmux/refresh': (c, params: { method: string }) => {
        for (const file of (c.workspace as BtmuxWorkspace).files)
          file.view.dispatch({ effects: refreshEffect.of(params.method) });
        return true;
      },
    },
    extensions: [
      serverCompletions,
      signatureHelp(),
      keymap.of([
        { key: 'Shift-Alt-f', run: formatCommand, preventDefault: true },
        { key: 'F12', run: jumpToDefinition, preventDefault: true },
        { key: 'Shift-F12', run: findReferences, preventDefault: true },
        { key: 'Escape', run: closeReferencePanel },
        { key: 'F2', run: startRename, preventDefault: true },
      ]),
      renameField,
      { clientCapabilities: { textDocument: { publishDiagnostics: { versionSupport: true } } } },
    ],
  });
  serverIds.set(created, server);
  if (editorConnection.connected) connect(server, created);
  clients.set(server, created);
  return created;
}

function connect(server: number, c: LSPClient) {
  c.connect(transport(server, () => c.workspace as BtmuxWorkspace));
  const attempt = c.initializing;
  attempt.then(
    // Requesters that ran before the capabilities arrived ask again.
    () => {
      for (const file of (c.workspace as BtmuxWorkspace).files)
        file.view.dispatch({ effects: refreshEffect.of(REFRESH_ALL) });
    },
    // A server that fails to start shows that in the status line. One that
    // started after this client gave up (its timeout is shorter than the
    // server's), or restarted after a crash, is initialized again.
    () => {
      let stop: (() => void) | null = null;
      let done = false;
      const check = (servers: LspServerStatus[]) => {
        if (done) return;
        const state = servers.find((s) => s.id === server)?.state;
        // Not reconnected or replaced since.
        const current = clients.get(server) === c && c.initializing === attempt && editorConnection.connected;
        if (current && state === 'starting') return;
        done = true;
        // Subscribing calls `check` before `stop` is set.
        queueMicrotask(() => {
          stop?.();
          if (current && state === 'ready') {
            c.disconnect();
            connect(server, c);
          }
        });
      };
      stop = editorConnection.onLspStatus(check);
    },
  );
}

editorConnection.onConnection((open) => {
  for (const [server, c] of clients) {
    if (open) connect(server, c);
    else c.disconnect();
  }
});

type Prompt = Extract<EditorServerMessage, { type: 'lsp_prompt' }>;

/** Prompts nobody answered yet, waiting for this tab's focus or the one shown. */
let prompts: Prompt[] = [];
let shown: { prompt: Prompt; close: () => void } | null = null;

const samePrompt = (a: { server: number; token: number }, b: { server: number; token: number }) =>
  a.server === b.server && a.token === b.token;

function showPrompts() {
  while (!shown && document.hasFocus()) {
    const prompt = prompts.shift();
    if (!prompt) return;
    const store = useStore.getState();
    if (!prompt.actions.length) {
      const level = prompt.level === 1 ? 'error' : prompt.level === 2 ? 'attention' : 'info';
      store.showToast(prompt.message, level);
      editorConnection.answerPrompt(prompt.server, prompt.token, null);
      continue;
    }
    let answered = false;
    const overlay: Overlay = {
      mode: 'picker',
      title: prompt.message,
      // By index: titles may repeat.
      items: prompt.actions.map((action, i) => ({ id: String(i), label: action })),
      onSelect: (id) => {
        answered = true;
        editorConnection.answerPrompt(prompt.server, prompt.token, Number(id));
        // The picker stays open for previews; one answer is all a prompt takes.
        useStore.getState().setOverlay(null);
      },
    };
    // Dismissed (Esc) or replaced: the server still gets its answer.
    const stop = useStore.subscribe((s) => {
      if (s.overlay === overlay) return;
      stop();
      shown = null;
      if (!answered) editorConnection.answerPrompt(prompt.server, prompt.token, null);
      showPrompts();
    });
    shown = {
      prompt,
      close: () => {
        answered = true;
        if (useStore.getState().overlay === overlay) useStore.getState().setOverlay(null);
      },
    };
    store.setOverlay(overlay);
  }
}

window.addEventListener('focus', showPrompts);

// The server answered for tabs that went away.
editorConnection.onConnection((open) => {
  if (open) return;
  prompts = [];
  shown?.close();
});

// The first tab focused acts on what a server asks of the user.
editorConnection.serverUi = (msg) => {
  if (msg.type === 'lsp_prompt') {
    prompts.push(msg);
    showPrompts();
    return;
  }
  if (msg.type === 'lsp_prompt_done') {
    prompts = prompts.filter((p) => !samePrompt(p, msg));
    if (shown && samePrompt(shown.prompt, msg)) shown.close();
    return;
  }
  if (!document.hasFocus()) return;
  if (msg.external) {
    if (/^https?:/.test(msg.uri)) window.open(msg.uri, '_blank', 'noopener');
    return;
  }
  reveal(msg.uri, (msg.selection as { start: Position } | null)?.start ?? null);
};

setRawDiagnostics((p) => diagnostics.get(p.client)?.get(p.uri)?.diagnostics ?? []);

/** The server failed to start (not installed) or crashed too often. */
export const serverFailed = (server: number | undefined) =>
  editorConnection.lspStatus.find((s) => s.id === server)?.state === 'failed';

/** `doc`'s servers that haven't failed, whose features it gets. */
export const workingServers = (doc: Doc) => doc.servers.filter((s) => !serverFailed(s));

// lsp-client's signature help has no key to close it, so Esc hides the
// tooltip shown until another replaces it (a new parameter) or a trigger
// character is typed (a new call, which lsp-client may answer with the same
// tooltip) or help is asked for. Its state field is the first extension
// `signatureHelp` returns.
const signatureField = (signatureHelp() as readonly Extension[])[0] as StateField<{ tooltip: Tooltip } | null>;
const dismissSignature = StateEffect.define<Tooltip['create'] | null>();
/** The server's signature help trigger characters. */
const signatureTriggers = Facet.define<() => readonly string[], () => readonly string[]>({
  combine: (values) => values[0] ?? (() => []),
});
const dismissedSignature = StateField.define<Tooltip['create'] | null>({
  create: () => null,
  update: (value, tr) => tr.effects.reduce((v, e) => (e.is(dismissSignature) ? e.value : v), value),
});
/** Show signature help, even if Esc dismissed it. */
export const signatureHelpCommand = (view: EditorView) => {
  if (view.state.field(dismissedSignature, false)) view.dispatch({ effects: dismissSignature.of(null) });
  return showSignatureHelp(view);
};

const signatureDismissal: Extension = [
  dismissedSignature,
  EditorView.editorAttributes.compute([signatureField, dismissedSignature], (state): Record<string, string> => {
    const dismissed = state.field(dismissedSignature);
    return dismissed && state.field(signatureField, false)?.tooltip.create === dismissed
      ? { class: 'cm-btmux-signature-dismissed' }
      : {};
  }),
  EditorState.transactionExtender.of((tr) => {
    if (!tr.docChanged || !tr.isUserEvent('input.type') || !tr.startState.field(dismissedSignature)) return null;
    const triggers = tr.startState.facet(signatureTriggers)();
    let typed = false;
    tr.changes.iterChanges((_fromA, _toA, _fromB, _toB, inserted) => {
      const text = inserted.toString();
      if (triggers.some((c) => text.includes(c))) typed = true;
    });
    return typed ? { effects: dismissSignature.of(null) } : null;
  }),
  Prec.highest(
    keymap.of([
      {
        key: 'Mod-Shift-Space',
        run: signatureHelpCommand,
      },
    ]),
  ),
  // An observer, so Esc still reaches vim and closes completions.
  EditorView.domEventObservers({
    keydown(e, view) {
      if (e.key !== 'Escape' || e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) return;
      const create = view.state.field(signatureField, false)?.tooltip.create;
      if (create && create !== view.state.field(dismissedSignature))
        view.dispatch({ effects: dismissSignature.of(create) });
    },
  }),
  EditorView.theme({ '&.cm-btmux-signature-dismissed .cm-lsp-signature-tooltip': { display: 'none' } }),
];

/**
 * The language server features for `doc`, through each of its servers that
 * hasn't failed, whose features would only report errors. lsp-client's own
 * features (signature help, the reference panel) go through the first.
 */
export function lspExtension(doc: Doc): Extension {
  const [server, ...others] = workingServers(doc);
  const uri = doc.uri;
  const languageID = doc.languageId;
  if (server === undefined || !uri || !languageID) return [];
  return [
    client(server).plugin(uri, languageID),
    serverDiagnosticsField,
    otherServers(others.map((s) => ({ client: client(s), uri, languageID }))),
    others.map((s) => completionsFrom(client(s))),
    lspFeatures,
    inlineCompletions,
    signatureDismissal,
    signatureTriggers.of(() => client(server).serverCapabilities?.signatureHelpProvider?.triggerCharacters ?? []),
    // Cmd-click (Ctrl elsewhere) jumps to the definition.
    EditorView.domEventHandlers({
      mousedown(e, view) {
        if (e.button !== 0 || !(IS_MAC ? e.metaKey : e.ctrlKey) || e.altKey || e.shiftKey) return false;
        const pos = view.posAtCoords({ x: e.clientX, y: e.clientY });
        if (pos === null) return false;
        e.preventDefault();
        view.dispatch({ selection: { anchor: pos } });
        jumpToDefinition(view);
        return true;
      },
    }),
    EditorView.theme({
      '.cm-btmux-rename': { padding: '4px 8px' },
      '.cm-btmux-rename input': { marginLeft: '4px', font: 'inherit' },
      // Diagnostic tags, as other editors show them: hints only fade or strike.
      '.cm-lsp-unnecessary': { opacity: '0.55' },
      '.cm-lsp-deprecated': { textDecoration: 'line-through' },
      '.cm-lintRange-hint.cm-lsp-unnecessary, .cm-lintRange-hint.cm-lsp-deprecated': { backgroundImage: 'none' },
      '.cm-lsp-diagnostic-code': { opacity: '0.7' },
      '.cm-lsp-related': { margin: '2px 0 0', paddingLeft: '1.2em' },
      '.cm-lsp-related a': { color: 'inherit', textDecoration: 'underline' },
    }),
  ];
}
