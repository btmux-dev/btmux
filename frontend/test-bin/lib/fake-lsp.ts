// A fake language server for browser tests, over stdio. It keeps the
// documents it is sent and logs each method to $BTMUX_FAKE_LSP/log (if set):
// - diagnostics: an error on every "bad"; a hint tagged unnecessary on every
//   "unused", a warning tagged deprecated, with a code and its link and
//   other.fake as related information, on every "old"
// - implementation: other.fake's first line
// - pulled diagnostics depend on other files: each pull is logged
// - code actions on "withdraw": ask the user, then cancel the question
// - completion: "fakeCompletion", with 100 paragraphs of docs, and
//   "importedThing", whose resolve adds documentation and an import ("use lib;"
//   on line 1)
// - signature help: in "sig(", the parameter after as many commas
// - hover: "Fake hover for WORD", 700 ms late for "slow", 200 paragraphs for "use"
// - definition: WORD's first line in other.fake beside the document
// - prepare rename: WORD's range and name; an error for "use" and "let",
//   null where there's no word
// - rename: every WORD in the .fake files of the document's directory,
//   open or not (as edits to the open text or the file on disk)
// - formatting: trailing spaces removed, 700 ms late for text with "slow"
// - pulled diagnostics: an error on every "worse"
// - semantic tokens: every "thing" is a function
// - inlay hints: ": hint" after every "let NAME", which resolve with a
//   tooltip, a command on ": " (fake.hint), the location of other.fake on
//   "hint", and an edit inserting the hint
// - document highlights, symbols, call hierarchy: occurrences of WORD
// - code lenses: "fake.ask" on line 1, which asks the user (showMessageRequest)
// - code actions: replace "bad" with "good"; on a last line "end", append "!"
//   and uppercase it, which needs confirmation, or have the server uppercase
//   "end!" (workspace/applyEdit, needing confirmation); on "move me", create
//   mod/extra.fake with a line, move the file to mod/moved.fake, and delete
//   gone.fake if it exists
// - folding: the first three lines; selection ranges: word, then line
// - file operations on *.fake: renames replace the old name with the new in
//   the directory's other .fake files (willRenameFiles); did*Files logged
// - watched files: **/*.fake, logged; a new crash.fake makes it exit, once
// - opened documents: logged with their language id
// - progress: "Indexing" at 50% while any document contains "busy"
import { appendFileSync, existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';

type Position = { line: number; character: number };
type Range = { start: Position; end: Position };

const docs = new Map<string, { text: string; version: number }>();
const log = (line: string) => {
  const dir = process.env.BTMUX_FAKE_LSP;
  if (dir) appendFileSync(join(dir, 'log'), `${line}\n`);
};

const pathOf = (uri: string) => decodeURIComponent(new URL(uri).pathname);
const uriOf = (path: string) => `file://${path.split('/').map(encodeURIComponent).join('/')}`;

function offset(text: string, pos: Position) {
  const lines = text.split('\n');
  let at = 0;
  for (let i = 0; i < pos.line && i < lines.length; i++) at += lines[i].length + 1;
  return Math.min(at + Math.min(pos.character, lines[pos.line]?.length ?? 0), text.length);
}

function position(text: string, at: number): Position {
  const before = text.slice(0, at).split('\n');
  return { line: before.length - 1, character: before[before.length - 1].length };
}

function wordRange(text: string, pos: Position) {
  const at = offset(text, pos);
  let from = at;
  let to = at;
  while (from > 0 && /\w/.test(text[from - 1])) from--;
  while (to < text.length && /\w/.test(text[to])) to++;
  return { from, to };
}

function wordAt(text: string, pos: Position) {
  const { from, to } = wordRange(text, pos);
  return text.slice(from, to);
}

function occurrences(text: string, word: string): Range[] {
  const ranges: Range[] = [];
  // No word at the cursor (an empty line): `\b\b` would match forever.
  if (!word) return ranges;
  const re = new RegExp(`\\b${word}\\b`, 'g');
  for (let m = re.exec(text); m; m = re.exec(text))
    ranges.push({ start: position(text, m.index), end: position(text, m.index + word.length) });
  return ranges;
}

function send(message: unknown) {
  const body = Buffer.from(JSON.stringify(message));
  process.stdout.write(`Content-Length: ${body.length}\r\n\r\n`);
  process.stdout.write(body);
}

function publish(uri: string) {
  const doc = docs.get(uri)!;
  const diagnostics = [
    ...occurrences(doc.text, 'bad').map((range) => ({ range, severity: 1, message: 'bad word' })),
    ...occurrences(doc.text, 'unused').map((range) => ({ range, severity: 4, message: 'never used', tags: [1] })),
    ...occurrences(doc.text, 'old').map((range) => ({
      range,
      severity: 2,
      message: 'old is deprecated',
      tags: [2],
      code: 'D1',
      codeDescription: { href: 'https://example.com/D1' },
      relatedInformation: [
        {
          location: {
            uri: uriOf(join(dirname(pathOf(uri)), 'other.fake')),
            range: { start: { line: 0, character: 0 }, end: { line: 0, character: 5 } },
          },
          message: 'replaced here',
        },
      ],
    })),
  ];
  send({
    jsonrpc: '2.0',
    method: 'textDocument/publishDiagnostics',
    params: { uri, version: doc.version, diagnostics },
  });
}

function text(uri: string) {
  return docs.get(uri)?.text ?? readFileSync(pathOf(uri), 'utf8');
}

function handle(msg: any): unknown {
  const p = msg.params;
  switch (msg.method) {
    case 'initialize':
      return {
        capabilities: {
          textDocumentSync: { openClose: true, change: 2, save: { includeText: false } },
          completionProvider: { resolveProvider: true },
          signatureHelpProvider: { triggerCharacters: ['(', ','] },
          hoverProvider: true,
          definitionProvider: true,
          renameProvider: { prepareProvider: true },
          documentFormattingProvider: true,
          diagnosticProvider: { interFileDependencies: true, workspaceDiagnostics: false },
          implementationProvider: true,
          semanticTokensProvider: { legend: { tokenTypes: ['function'], tokenModifiers: [] }, full: true },
          inlayHintProvider: { resolveProvider: true },
          documentHighlightProvider: true,
          documentSymbolProvider: true,
          workspaceSymbolProvider: true,
          callHierarchyProvider: true,
          codeLensProvider: {},
          codeActionProvider: true,
          foldingRangeProvider: true,
          selectionRangeProvider: true,
          executeCommandProvider: { commands: ['fake.ask', 'fake.shout', 'fake.hint', 'fake.withdraw'] },
          workspace: {
            fileOperations: Object.fromEntries(
              ['willRename', 'didRename', 'didCreate', 'didDelete'].map((op) => [
                op,
                { filters: [{ scheme: 'file', pattern: { glob: '**/*.fake' } }] },
              ]),
            ),
          },
        },
        serverInfo: { name: 'fake-lsp' },
      };
    case 'shutdown':
      return null;
    case 'textDocument/completion':
      return {
        isIncomplete: false,
        itemDefaults: { data: { from: 'defaults' } },
        items: [
          {
            label: 'fakeCompletion',
            kind: 3,
            documentation: {
              kind: 'markdown',
              value: Array.from({ length: 100 }, (_, i) => `Completion paragraph ${i}.`).join('\n\n'),
            },
          },
          { label: 'importedThing', kind: 6, labelDetails: { description: 'lib' } },
        ],
      };
    case 'textDocument/signatureHelp': {
      const line = text(p.textDocument.uri).split('\n')[p.position.line] ?? '';
      const call = /\bsig\(([^)]*)$/.exec(line.slice(0, p.position.character));
      if (!call) return null;
      return {
        signatures: [{ label: 'sig(a, b)', parameters: [{ label: 'a' }, { label: 'b' }] }],
        activeSignature: 0,
        activeParameter: call[1].split(',').length - 1,
      };
    }
    case 'completionItem/resolve':
      log(`resolve ${p.label} ${JSON.stringify(p.data)}`);
      return {
        ...p,
        detail: `fn ${p.label}()`,
        documentation: p.documentation ?? { kind: 'markdown', value: `Docs for **${p.label}**` },
        additionalTextEdits:
          p.label === 'importedThing'
            ? [{ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } }, newText: 'use lib;\n' }]
            : undefined,
      };
    case 'textDocument/hover': {
      const word = wordAt(text(p.textDocument.uri), p.position);
      if (word === 'use') {
        const value = Array.from({ length: 200 }, (_, i) => `Long hover paragraph ${i}.`).join('\n\n');
        return { contents: { kind: 'markdown', value } };
      }
      return { contents: { kind: 'markdown', value: `Fake hover for **${word}**` } };
    }
    case 'textDocument/implementation': {
      const other = uriOf(join(dirname(pathOf(p.textDocument.uri)), 'other.fake'));
      return [{ uri: other, range: { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } } }];
    }
    case 'textDocument/definition': {
      const word = wordAt(text(p.textDocument.uri), p.position);
      const other = uriOf(join(dirname(pathOf(p.textDocument.uri)), 'other.fake'));
      const range = occurrences(text(other), word)[0];
      return range ? { uri: other, range } : null;
    }
    case 'workspace/willRenameFiles': {
      const changes: Record<string, unknown[]> = {};
      for (const { oldUri, newUri } of p.files) {
        const [from, to] = [basename(pathOf(oldUri)), basename(pathOf(newUri))];
        const dir = dirname(pathOf(oldUri));
        for (const name of readdirSync(dir).filter((n) => n.endsWith('.fake') && n !== from)) {
          const uri = uriOf(join(dir, name));
          const t = text(uri);
          const edits = [];
          for (let at = t.indexOf(from); at >= 0; at = t.indexOf(from, at + from.length))
            edits.push({ range: { start: position(t, at), end: position(t, at + from.length) }, newText: to });
          if (edits.length) changes[uri] = edits;
        }
      }
      return { changes };
    }
    case 'textDocument/prepareRename': {
      const t = text(p.textDocument.uri);
      const { from, to } = wordRange(t, p.position);
      const word = t.slice(from, to);
      if (!word) return null;
      if (word === 'use' || word === 'let') throw new Error(`${word} is a keyword`);
      return { range: { start: position(t, from), end: position(t, to) }, placeholder: word };
    }
    case 'textDocument/rename': {
      const word = wordAt(text(p.textDocument.uri), p.position);
      const dir = dirname(pathOf(p.textDocument.uri));
      const changes: Record<string, unknown[]> = {};
      for (const name of readdirSync(dir).filter((n) => n.endsWith('.fake'))) {
        const uri = uriOf(join(dir, name));
        const edits = occurrences(text(uri), word).map((range) => ({ range, newText: p.newName }));
        if (edits.length) changes[uri] = edits;
      }
      return { changes };
    }
    case 'textDocument/diagnostic': {
      log(`pull ${pathOf(p.textDocument.uri)}`);
      const items = occurrences(text(p.textDocument.uri), 'worse').map((range) => ({
        range,
        severity: 1,
        message: 'worse word',
      }));
      return { kind: 'full', items };
    }
    case 'textDocument/semanticTokens/full': {
      const data: number[] = [];
      let line = 0;
      let char = 0;
      for (const r of occurrences(text(p.textDocument.uri), 'thing')) {
        const dl = r.start.line - line;
        data.push(dl, dl ? r.start.character : r.start.character - char, 5, 0, 0);
        line = r.start.line;
        char = r.start.character;
      }
      return { data };
    }
    case 'textDocument/inlayHint': {
      const t = text(p.textDocument.uri);
      return [...t.matchAll(/let (\w+)/g)].map((m) => ({
        position: position(t, m.index! + m[0].length),
        label: [{ value: ': ' }, { value: 'hint' }],
        data: { uri: p.textDocument.uri, name: m[1] },
      }));
    }
    case 'inlayHint/resolve': {
      const other = uriOf(join(dirname(pathOf(p.data.uri)), 'other.fake'));
      const start = { line: 0, character: 0 };
      return {
        ...p,
        tooltip: { kind: 'markdown', value: `Type of **${p.data.name}**` },
        label: [
          { value: ': ', command: { title: 'Hint', command: 'fake.hint', arguments: [p.data.name] } },
          { value: 'hint', location: { uri: other, range: { start, end: start } } },
        ],
        textEdits: [{ range: { start: p.position, end: p.position }, newText: ': hint' }],
      };
    }
    case 'textDocument/documentHighlight': {
      const t = text(p.textDocument.uri);
      return occurrences(t, wordAt(t, p.position)).map((range) => ({ range }));
    }
    case 'textDocument/documentSymbol': {
      const t = text(p.textDocument.uri);
      return occurrences(t, 'thing').map((range) => ({
        name: 'thing',
        kind: 12,
        range,
        selectionRange: range,
      }));
    }
    case 'workspace/symbol': {
      const results = [];
      for (const [uri, doc] of docs)
        for (const range of occurrences(doc.text, 'thing'))
          if ('thing'.includes(p.query)) results.push({ name: 'thing', kind: 12, location: { uri, range } });
      const other = uriOf(join(dirname(pathOf([...docs.keys()][0])), 'other.fake'));
      for (const range of occurrences(text(other), 'thing'))
        if ('thing'.includes(p.query)) results.push({ name: 'thing', kind: 12, location: { uri: other, range } });
      return results;
    }
    case 'textDocument/prepareCallHierarchy': {
      const t = text(p.textDocument.uri);
      const word = wordAt(t, p.position);
      const range = occurrences(t, word)[0];
      return range ? [{ name: word, kind: 12, uri: p.textDocument.uri, range, selectionRange: range }] : null;
    }
    case 'callHierarchy/incomingCalls': {
      const item = p.item;
      return occurrences(text(item.uri), item.name).map((range) => ({
        from: { name: `caller@${range.start.line + 1}`, kind: 12, uri: item.uri, range, selectionRange: range },
        fromRanges: [range],
      }));
    }
    case 'textDocument/codeLens':
      return [
        {
          range: { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } },
          command: { title: 'Ask me', command: 'fake.ask' },
        },
      ];
    case 'workspace/executeCommand':
      log(`execute ${p.command} ${JSON.stringify(p.arguments ?? [])}`);
      if (p.command === 'fake.withdraw') {
        const id = nextId;
        request('window/showMessageRequest', { type: 3, message: 'Withdrawn soon?', actions: [{ title: 'Never' }] });
        setTimeout(() => send({ jsonrpc: '2.0', method: '$/cancelRequest', params: { id } }), 700);
      }
      if (p.command === 'fake.ask')
        request('window/showMessageRequest', {
          type: 3,
          message: 'Proceed?',
          actions: [{ title: 'Yes' }, { title: 'No' }],
        });
      if (p.command === 'fake.shout') {
        const [uri, line] = p.arguments;
        const range = { start: { line, character: 0 }, end: { line, character: 4 } };
        request('workspace/applyEdit', {
          label: 'Shout',
          edit: shout(uri, [{ range, newText: 'END!', annotationId: 'shout' }]),
        });
      }
      return null;
    case 'textDocument/codeAction': {
      const uri = p.textDocument.uri;
      const t = text(uri);
      const edits = occurrences(t, 'bad').map((range) => ({ range, newText: 'good' }));
      const actions: unknown[] = edits.length
        ? [{ title: 'Replace bad with good', kind: 'quickfix', edit: { changes: { [uri]: edits } } }]
        : [];
      const lines = t.split('\n');
      const line = lines.length - 2;
      const at = (character: number) => ({ line, character });
      if (lines[line] === 'end')
        actions.push({
          title: 'Shout',
          edit: shout(uri, [
            { range: { start: at(0), end: at(3) }, newText: 'END', annotationId: 'shout' },
            { range: { start: at(3), end: at(3) }, newText: '!', annotationId: 'plain' },
          ]),
        });
      if (t.includes('withdraw'))
        actions.push({ title: 'Ask, then withdraw', command: { title: 'Withdraw', command: 'fake.withdraw' } });
      if (t.includes('move me')) {
        const dir = dirname(pathOf(uri));
        const extra = uriOf(join(dir, 'mod', 'extra.fake'));
        const start = { line: 0, character: 0 };
        actions.push({
          title: 'Move into a module',
          edit: {
            documentChanges: [
              { kind: 'create', uri: extra },
              {
                textDocument: { uri: extra, version: null },
                edits: [{ range: { start, end: start }, newText: 'created\n' }],
              },
              { kind: 'rename', oldUri: uri, newUri: uriOf(join(dir, 'mod', 'moved.fake')) },
              { kind: 'delete', uri: uriOf(join(dir, 'gone.fake')), options: { ignoreIfNotExists: true } },
            ],
          },
        });
      }
      if (lines[line] === 'end!')
        actions.push({
          title: 'Shout later',
          command: { title: 'Shout later', command: 'fake.shout', arguments: [uri, line] },
        });
      return actions;
    }
    case 'textDocument/foldingRange':
      return [{ startLine: 0, endLine: 2 }];
    case 'textDocument/selectionRange': {
      const t = text(p.textDocument.uri);
      return p.positions.map((pos: Position) => {
        const at = offset(t, pos);
        let from = at;
        let to = at;
        while (from > 0 && /\w/.test(t[from - 1])) from--;
        while (to < t.length && /\w/.test(t[to])) to++;
        const line = {
          start: { line: pos.line, character: 0 },
          end: { line: pos.line, character: t.split('\n')[pos.line].length },
        };
        return { range: { start: position(t, from), end: position(t, to) }, parent: { range: line } };
      });
    }
    case 'textDocument/formatting': {
      const t = text(p.textDocument.uri);
      const formatted = t.replace(/[ \t]+$/gm, '');
      if (formatted === t) return [];
      return [{ range: { start: position(t, 0), end: position(t, t.length) }, newText: formatted }];
    }
  }
  return undefined;
}

/** `edits` in `uri`, annotated: "shout" needs confirmation. */
function shout(uri: string, edits: unknown[]) {
  return {
    documentChanges: [{ textDocument: { uri, version: null }, edits }],
    changeAnnotations: {
      shout: { label: 'Uppercase end', needsConfirmation: true },
      plain: { label: 'Exclaim' },
    },
  };
}

let busy = false;
function progress() {
  const now = [...docs.values()].some((d) => d.text.includes('busy'));
  if (now === busy) return;
  busy = now;
  const token = 'fake-index';
  const value = (v: object) => send({ jsonrpc: '2.0', method: '$/progress', params: { token, value: v } });
  if (!busy) return value({ kind: 'end' });
  request('window/workDoneProgress/create', { token });
  value({ kind: 'begin', title: 'Indexing', percentage: 0 });
  value({ kind: 'report', message: '1/2', percentage: 50 });
}

function notify(msg: any) {
  const p = msg.params;
  switch (msg.method) {
    case 'textDocument/didOpen':
      log(`open ${pathOf(p.textDocument.uri)} ${p.textDocument.languageId}`);
      docs.set(p.textDocument.uri, { text: p.textDocument.text, version: p.textDocument.version });
      publish(p.textDocument.uri);
      progress();
      break;
    case 'textDocument/didChange': {
      const doc = docs.get(p.textDocument.uri);
      if (!doc) break;
      for (const change of p.contentChanges) {
        if (!change.range) doc.text = change.text;
        else
          doc.text =
            doc.text.slice(0, offset(doc.text, change.range.start)) +
            change.text +
            doc.text.slice(offset(doc.text, change.range.end));
      }
      doc.version = p.textDocument.version;
      log(`version ${doc.version} ${JSON.stringify(doc.text)}`);
      publish(p.textDocument.uri);
      progress();
      break;
    }
    case 'workspace/didRenameFiles':
    case 'workspace/didCreateFiles':
    case 'workspace/didDeleteFiles':
      log(`${msg.method} ${JSON.stringify(p.files)}`);
      break;
    case 'textDocument/didClose':
      docs.delete(p.textDocument.uri);
      break;
    case 'exit':
      process.exit(0);
  }
}

let nextId = 1000;
const asked = new Map<number, string>();
function request(method: string, params: unknown) {
  const id = nextId++;
  asked.set(id, method);
  send({ jsonrpc: '2.0', id, method, params });
}

let buffer = Buffer.alloc(0);
process.stdin.on('data', (chunk: Buffer) => {
  buffer = Buffer.concat([buffer, chunk]);
  for (;;) {
    const end = buffer.indexOf('\r\n\r\n');
    if (end < 0) return;
    const length = Number(/Content-Length: (\d+)/i.exec(buffer.subarray(0, end).toString())?.[1]);
    if (buffer.length < end + 4 + length) return;
    const msg = JSON.parse(buffer.subarray(end + 4, end + 4 + length).toString());
    buffer = buffer.subarray(end + 4 + length);
    if (msg.method === undefined && asked.has(msg.id)) {
      log(`answer ${asked.get(msg.id)} ${JSON.stringify(msg.result)}`);
      asked.delete(msg.id);
      continue;
    }
    log(msg.method ?? 'response');
    if (msg.method === 'workspace/didChangeWatchedFiles') {
      log(`watched ${JSON.stringify(msg.params.changes)}`);
      const marker = join(process.env.BTMUX_FAKE_LSP ?? '/nonexistent', 'crashed');
      if (msg.params.changes.some((c: { uri: string }) => c.uri.endsWith('/crash.fake')) && !existsSync(marker)) {
        writeFileSync(marker, '');
        process.exit(1);
      }
    }
    if (msg.method === 'initialized')
      request('client/registerCapability', {
        registrations: [
          {
            id: 'watch',
            method: 'workspace/didChangeWatchedFiles',
            registerOptions: { watchers: [{ globPattern: '**/*.fake' }] },
          },
        ],
      });
    if (msg.id !== undefined && msg.method) {
      let result: unknown;
      let failed: string | null = null;
      try {
        result = handle(msg);
      } catch (error) {
        failed = (error as Error).message;
      }
      const reply = () =>
        send(
          failed !== null
            ? { jsonrpc: '2.0', id: msg.id, error: { code: -32803, message: failed } }
            : result === undefined
              ? { jsonrpc: '2.0', id: msg.id, error: { code: -32601, message: msg.method } }
              : { jsonrpc: '2.0', id: msg.id, result },
        );
      // Late capabilities: features must ask once they arrive.
      if (msg.method === 'initialize') setTimeout(reply, 300);
      else if (msg.method === 'textDocument/formatting' && text(msg.params.textDocument.uri).includes('slow'))
        setTimeout(reply, 700);
      else if (
        msg.method === 'textDocument/hover' &&
        wordAt(text(msg.params.textDocument.uri), msg.params.position) === 'slow'
      )
        setTimeout(reply, 700);
      else reply();
    } else if (msg.method) notify(msg);
  }
});
process.stdin.on('end', () => process.exit(0));
