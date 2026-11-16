// A fake language server for browser tests, over stdio. It keeps the
// documents it is sent and logs each method to $BTMUX_FAKE_LSP/log (if set):
// - diagnostics: an error on every "bad"
// - completion: "fakeCompletion"
// - hover: "Fake hover for WORD"
// - definition: WORD's first line in other.fake beside the document
// - rename: every WORD in the .fake files of the document's directory,
//   open or not (as edits to the open text or the file on disk)
// - formatting: trailing spaces removed
import { appendFileSync, readdirSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

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

function wordAt(text: string, pos: Position) {
  const at = offset(text, pos);
  let from = at;
  let to = at;
  while (from > 0 && /\w/.test(text[from - 1])) from--;
  while (to < text.length && /\w/.test(text[to])) to++;
  return text.slice(from, to);
}

function occurrences(text: string, word: string): Range[] {
  const ranges: Range[] = [];
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
  const diagnostics = occurrences(doc.text, 'bad').map((range) => ({ range, severity: 1, message: 'bad word' }));
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
          completionProvider: {},
          hoverProvider: true,
          definitionProvider: true,
          renameProvider: true,
          documentFormattingProvider: true,
        },
        serverInfo: { name: 'fake-lsp' },
      };
    case 'shutdown':
      return null;
    case 'textDocument/completion':
      return { isIncomplete: false, items: [{ label: 'fakeCompletion', kind: 3 }] };
    case 'textDocument/hover': {
      const word = wordAt(text(p.textDocument.uri), p.position);
      return { contents: { kind: 'markdown', value: `Fake hover for **${word}**` } };
    }
    case 'textDocument/definition': {
      const word = wordAt(text(p.textDocument.uri), p.position);
      const other = uriOf(join(dirname(pathOf(p.textDocument.uri)), 'other.fake'));
      const range = occurrences(text(other), word)[0];
      return range ? { uri: other, range } : null;
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
    case 'textDocument/formatting': {
      const t = text(p.textDocument.uri);
      const formatted = t.replace(/[ \t]+$/gm, '');
      if (formatted === t) return [];
      return [{ range: { start: position(t, 0), end: position(t, t.length) }, newText: formatted }];
    }
  }
  return undefined;
}

function notify(msg: any) {
  const p = msg.params;
  switch (msg.method) {
    case 'textDocument/didOpen':
      docs.set(p.textDocument.uri, { text: p.textDocument.text, version: p.textDocument.version });
      publish(p.textDocument.uri);
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
      break;
    }
    case 'textDocument/didClose':
      docs.delete(p.textDocument.uri);
      break;
    case 'exit':
      process.exit(0);
  }
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
    log(msg.method ?? 'response');
    if (msg.id !== undefined && msg.method) {
      const result = handle(msg);
      send(
        result === undefined
          ? { jsonrpc: '2.0', id: msg.id, error: { code: -32601, message: msg.method } }
          : { jsonrpc: '2.0', id: msg.id, result },
      );
    } else if (msg.method) notify(msg);
  }
});
process.stdin.on('end', () => process.exit(0));
