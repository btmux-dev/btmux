import {
  findReferencesKeymap,
  hoverTooltips,
  jumpToDefinition,
  jumpToDefinitionKeymap,
  LSPClient,
  LSPPlugin,
  serverCompletion,
  signatureHelp,
  Workspace,
  type Transport,
  type WorkspaceFile,
} from '@codemirror/lsp-client';
import { StateEffect, StateField, type Extension, type Text } from '@codemirror/state';
import { EditorView, keymap, showPanel, type Panel } from '@codemirror/view';
import { getIndentUnit, indentUnit, LanguageDescription } from '@codemirror/language';
import { setDiagnostics, type Diagnostic } from '@codemirror/lint';
import { languages } from '@codemirror/language-data';
import DOMPurify from 'dompurify';
import { useStore } from '../../state/store';
import type { Overlay } from '../../state/types';
import type { EditorServerMessage } from '../../generated/protocol';
import { IS_MAC } from '../nvimKeys';
import { editorConnection } from './connection';
import { viewDocs, type Doc } from './doc';
import { REFRESH_ALL, lspFeatures, refreshEffect, setRawDiagnostics } from './features';

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

class BtmuxWorkspace extends Workspace {
  files: File[] = [];

  syncFiles() {
    const result = [];
    for (const file of this.files) {
      const doc = viewDocs.get(file.view);
      doc?.push();
      const plugin = LSPPlugin.get(file.view);
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
    queueMicrotask(() => {
      const params = diagnostics.get(this.client)?.get(uri);
      if (params) showDiagnostics(this.client, view, params);
    });
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
  }[];
};

/** The latest diagnostics per client and URI. */
const diagnostics = new Map<LSPClient, Map<string, PublishParams>>();

function offset(doc: Text, pos: { line: number; character: number }) {
  if (pos.line >= doc.lines) return doc.length;
  const line = doc.line(pos.line + 1);
  return Math.min(line.from + pos.character, line.to);
}

function showDiagnostics(client: LSPClient, view: EditorView, params: PublishParams) {
  const plugin = LSPPlugin.get(view);
  if (!plugin || plugin.client !== client) return;
  // Positions are in the synced document; map them over edits since.
  const synced = plugin.syncedDoc;
  const changes = plugin.unsyncedChanges;
  const list: Diagnostic[] = params.diagnostics.map((d) => {
    const from = changes.mapPos(offset(synced, d.range.start), 1);
    const to = Math.max(from, changes.mapPos(offset(synced, d.range.end), -1));
    const severity = d.severity === 1 ? 'error' : d.severity === 2 ? 'warning' : d.severity === 3 ? 'info' : 'hint';
    return { from, to, severity, message: d.message, source: d.source };
  });
  view.dispatch(setDiagnostics(view.state, list));
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

const setRename = StateEffect.define<boolean>();

const renameField = StateField.define<boolean>({
  create: () => false,
  update: (open, tr) => tr.effects.reduce((v, e) => (e.is(setRename) ? e.value : v), open),
  provide: (f) => showPanel.from(f, (open) => (open ? renamePanel : null)),
});

function renamePanel(view: EditorView): Panel {
  const word = view.state.wordAt(view.state.selection.main.head);
  const form = document.createElement('form');
  form.className = 'cm-btmux-rename';
  const label = document.createElement('label');
  label.textContent = 'Rename to ';
  const input = document.createElement('input');
  input.name = 'name';
  input.value = word ? view.state.sliceDoc(word.from, word.to) : '';
  input.setAttribute('aria-label', 'New name');
  label.append(input);
  form.append(label);
  const close = () => {
    view.dispatch({ effects: setRename.of(false) });
    view.focus();
  };
  form.addEventListener('submit', (e) => {
    e.preventDefault();
    const name = input.value.trim();
    close();
    if (name) void rename(view, name);
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

const startRename = (view: EditorView) => {
  const plugin = LSPPlugin.get(view);
  if (!plugin || !view.state.wordAt(view.state.selection.main.head)) return false;
  view.dispatch({ effects: setRename.of(true) });
  return true;
};

const toast = (message: string, error: unknown) =>
  useStore.getState().showToast(message, 'error', {
    body: error instanceof Error ? error.message : String((error as any)?.message ?? error),
  });

/** Rename across files: the server applies the edit to every buffer, open or not. */
async function rename(view: EditorView, newName: string) {
  const plugin = LSPPlugin.get(view);
  const doc = viewDocs.get(view);
  const word = view.state.wordAt(view.state.selection.main.head);
  if (!plugin || !doc || !word) return;
  plugin.client.sync();
  const version = doc.version();
  try {
    const edit = await plugin.client.request('textDocument/rename', {
      textDocument: { uri: plugin.uri },
      position: plugin.toPosition(word.from),
      newName,
    });
    if (!edit) return;
    const result = await editorConnection.applyEdit(edit, doc.buffer, version);
    if (result.error) toast('Rename failed', result.error);
  } catch (error) {
    toast('Rename failed', error);
  }
}

type TextEdit = { range: PublishParams['diagnostics'][number]['range']; newText: string };

/** Format with the language server, if it can; resolves once applied. */
export async function format(view: EditorView) {
  const plugin = LSPPlugin.get(view);
  const doc = viewDocs.get(view);
  if (!plugin || !doc || (plugin.client as any).hasCapability('documentFormattingProvider') === false) return;
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
    if (LSPPlugin.get(view) !== plugin) {
      // Still the formatted text: apply it there, shown or not.
      if (doc.state?.doc !== start) return;
      const changes = edits.map((e) => ({
        from: offset(start, e.range.start),
        to: offset(start, e.range.end),
        insert: e.newText,
      }));
      doc.update({ changes, userEvent: 'format' });
      return;
    }
    const changed = mapping.getMapping(uri);
    const changes = [];
    for (const edit of edits) {
      let from = mapping.mapPosition(uri, edit.range.start);
      let to = mapping.mapPosition(uri, edit.range.end);
      if (changed) {
        // Typed into meanwhile: leave it.
        if (changed.touchesRange(from, to)) return;
        from = changed.mapPos(from, 1);
        to = changed.mapPos(to, -1);
      }
      changes.push({ from, to, insert: edit.newText });
    }
    view.dispatch({ changes, userEvent: 'format' });
  });
}

/** Instead of `formatKeymap`, whose command would format whatever file the view shows by then. */
const formatCommand = (view: EditorView) => {
  if (!LSPPlugin.get(view)) return false;
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
          if (file.uri === params.uri) showDiagnostics(c, file.view, params);
        return true;
      },
      'window/showMessage': (_, params: { type: number; message: string }) => {
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
      serverCompletion(),
      hoverTooltips(),
      signatureHelp(),
      keymap.of([
        { key: 'Shift-Alt-f', run: formatCommand, preventDefault: true },
        ...jumpToDefinitionKeymap,
        ...findReferencesKeymap,
        { key: 'F2', run: startRename, preventDefault: true },
      ]),
      renameField,
      { clientCapabilities: { textDocument: { publishDiagnostics: { versionSupport: true } } } },
    ],
  });
  if (editorConnection.connected) connect(server, created);
  clients.set(server, created);
  return created;
}

function connect(server: number, c: LSPClient) {
  c.connect(transport(server, () => c.workspace as BtmuxWorkspace));
  c.initializing.then(
    // Requesters that ran before the capabilities arrived ask again.
    () => {
      for (const file of (c.workspace as BtmuxWorkspace).files)
        file.view.dispatch({ effects: refreshEffect.of(REFRESH_ALL) });
    },
    // A server that fails to start shows that in the status line.
    () => {},
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
      items: prompt.actions.map((action) => ({ id: action, label: action })),
      onSelect: (action) => {
        answered = true;
        editorConnection.answerPrompt(prompt.server, prompt.token, action);
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
  const path = pathOf(msg.uri);
  if (!path || !display) return;
  const selection = msg.selection as { start: Position } | null;
  void display(path).then((view) => {
    if (!view || !selection) return;
    const at = offset(view.state.doc, selection.start);
    view.dispatch({ selection: { anchor: at }, effects: EditorView.scrollIntoView(at, { y: 'center' }) });
  });
};

type Position = { line: number; character: number };

setRawDiagnostics((p) => diagnostics.get(p.client)?.get(p.uri)?.diagnostics ?? []);

/** The server failed to start (not installed) or crashed too often. */
export const serverFailed = (server: number | undefined) =>
  editorConnection.lspStatus.find((s) => s.id === server)?.state === 'failed';

/** The language server features for `doc`, through its first server. */
export function lspExtension(doc: Doc): Extension {
  const server = doc.servers[0];
  // A failed server's features would only report errors.
  if (server === undefined || !doc.uri || !doc.languageId || serverFailed(server)) return [];
  return [
    client(server).plugin(doc.uri, doc.languageId),
    lspFeatures,
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
    }),
  ];
}
