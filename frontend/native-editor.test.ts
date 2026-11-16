// Run with `just test-frontend` (bun test).
import { expect, test } from 'bun:test';
import { ChangeSet, EditorState, Text } from '@codemirror/state';
import { collab, getSyncedVersion, receiveUpdates, sendableUpdates } from '@codemirror/collab';
import { freshFrom } from './src/lib/nativeEditor/doc';
import { detectIndent } from './src/lib/nativeEditor/languages';
import { decodeTokens } from './src/lib/nativeEditor/features';
import { fuzzyMatch } from './src/lib/nativeEditor/picker';

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
