// Run with `just test-frontend` (bun test).
import { expect, test } from 'bun:test';
import { ChangeSet, EditorState } from '@codemirror/state';
import { collab, getSyncedVersion, receiveUpdates, sendableUpdates } from '@codemirror/collab';
import { freshFrom } from './src/lib/nativeEditor/doc';
import { detectIndent } from './src/lib/nativeEditor/languages';

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
