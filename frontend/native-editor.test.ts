// Run with `just test-frontend` (bun test).
import { expect, test } from 'bun:test';
import { ChangeSet, EditorState, Text } from '@codemirror/state';
import { collab, getSyncedVersion, receiveUpdates, sendableUpdates } from '@codemirror/collab';
import { freshFrom } from './src/lib/nativeEditor/doc';
import { detectIndent } from './src/lib/nativeEditor/languages';
import { decodeTokens } from './src/lib/nativeEditor/features';
import { fuzzyMatch } from './src/lib/nativeEditor/picker';
import { parseQuery } from './src/lib/nativeEditor/quickOpen';
import { diff, rebase } from './src/lib/nativeEditor/rebase';

test('updates are skipped up to the synced version, and a gap is detected', () => {
  expect(freshFrom(3, 3)).toBe(0);
  expect(freshFrom(5, 3)).toBe(2);
  expect(freshFrom(3, 4)).toBe('gap');
});

test('indent detection prefers tabs, else the common space step', () => {
  expect(detectIndent('a\n\tb\n\t\tc\n')).toBe('\t');
  expect(detectIndent('a\n    b\n        c\n    d\n')).toBe('    ');
  expect(detectIndent('a\n  b\n    c\n')).toBe('  ');
  expect(detectIndent('no indent')).toBe('  ');
});

// The server applies pushes in order and echoes them; this is the round trip
// `Doc` relies on, with two editors editing at once.
test('two editors converge through the server order', () => {
  const make = (id: string) => EditorState.create({ doc: 'ab', extensions: collab({ startVersion: 0, clientID: id }) });
  let a = make('a');
  let b = make('b');
  a = a.update({ changes: { from: 0, insert: 'x' } }).state;
  b = b.update({ changes: { from: 2, insert: 'y' } }).state;
  // The server takes a's push first; b's (also on version 0) is stale.
  const log = sendableUpdates(a).map((u) => ({ changes: u.changes.toJSON(), clientID: u.clientID }));
  const receive = (state: EditorState, from: number) =>
    receiveUpdates(
      state,
      log.slice(from).map((u) => ({ changes: ChangeSet.fromJSON(u.changes), clientID: u.clientID })),
    ).state;
  a = receive(a, 0);
  b = receive(b, 0);
  expect(sendableUpdates(a)).toHaveLength(0);
  // b rebased its change and pushes it again, now on version 1.
  const rebased = sendableUpdates(b);
  expect(getSyncedVersion(b)).toBe(1);
  log.push(...rebased.map((u) => ({ changes: u.changes.toJSON(), clientID: u.clientID })));
  a = receive(a, 1);
  b = receive(b, 1);
  expect(a.doc.toString()).toBe('xaby');
  expect(b.doc.toString()).toBe('xaby');
});

test('semantic tokens decode relative positions and modifiers', () => {
  const doc = Text.of(['let thing = thing;', '  thing']);
  const legend = { tokenTypes: ['variable', 'function'], tokenModifiers: ['readonly', 'static'] };
  // thing@0:4 (function), thing@0:12 (variable, readonly|static), thing@1:2 (function)
  const data = [0, 4, 5, 1, 0, 0, 8, 5, 0, 3, 1, 2, 5, 1, 0];
  expect(decodeTokens(doc, data, legend)).toEqual([
    [4, 9, 'cm-sem-function'],
    [12, 17, 'cm-sem-variable cm-sem-m-readonly cm-sem-m-static'],
    [21, 26, 'cm-sem-function'],
  ]);
});

test('the picker matches characters in order', () => {
  expect(fuzzyMatch('workspaceSymbols', 'wsym')).toBe(true);
  expect(fuzzyMatch('workspaceSymbols', 'mys')).toBe(false);
  expect(fuzzyMatch('Thing', 'thi')).toBe(true);
});

const text = (s: string) => Text.of(s.split('\n'));

test('line diffs turn one text into the other, touching only changed lines', () => {
  const cases = [
    ['a\nb\nc\n', 'a\nB\nc\n'],
    ['a\nb\nc\n', 'x\na\nc\ny\n'],
    ['', 'new\n'],
    ['old', ''],
    ['same\n', 'same\n'],
    ['1\n2\n3\n4\n5\n6\n', '1\n3\n4\nX\n5\n'],
  ];
  for (const [a, b] of cases) expect(diff(text(a), text(b)).apply(text(a)).toString()).toBe(b);
  const changed: string[] = [];
  diff(text('a\nb\nc\nd\n'), text('a\nB\nc\nD\n')).iterChanges((fa, ta) =>
    changed.push(text('a\nb\nc\nd\n').sliceString(fa, ta)),
  );
  expect(changed).toEqual(['b\n', 'd\n']);
});

test('edits made while disconnected rebase onto the reopened text', () => {
  const base = text('one\ntwo\nthree\n');
  // Typed at the end of "two".
  const local = ChangeSet.of({ from: 7, insert: ' UNSENT' }, base.length);
  const same = rebase(base, local, base);
  expect(same.exact).toBe(true);
  expect(same.changes.apply(base).toString()).toBe('one\ntwo UNSENT\nthree\n');
  // The file changed on disk meanwhile, above and below.
  const server = text('zero\none\ntwo\nthree\nfour\n');
  const merged = rebase(base, local, server);
  expect(merged.exact).toBe(false);
  expect(merged.changes.apply(server).toString()).toBe('zero\none\ntwo UNSENT\nthree\nfour\n');
  // The cursor, after the typed text, stays after it.
  expect(merged.after.mapPos(14)).toBe(19);
});

test('quick open splits a trailing line (and ignored column) from the query', () => {
  expect(parseQuery('main.rs')).toEqual({ query: 'main.rs', line: null });
  expect(parseQuery('main.rs:42')).toEqual({ query: 'main.rs', line: 42 });
  expect(parseQuery('main.rs:42:7')).toEqual({ query: 'main.rs', line: 42 });
  expect(parseQuery('main.rs:')).toEqual({ query: 'main.rs', line: null });
  expect(parseQuery('main.rs:0')).toEqual({ query: 'main.rs', line: null });
  expect(parseQuery(':12')).toEqual({ query: '', line: 12 });
  expect(parseQuery('a:b')).toEqual({ query: 'a:b', line: null });
});
