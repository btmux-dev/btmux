// Run with `just test-frontend` (bun test).
import { expect, test } from 'bun:test';
import { ChangeSet } from '@codemirror/state';
import { eventToken, keyCapKeys, KeyTable, keyTables, showKeys, step } from './src/lib/nativeEditor/keyLayer';
import { describe, vimPresets, whichKeyRows } from './src/lib/nativeEditor/whichKey';
import { JumpList } from './src/lib/nativeEditor/jumps';
import { optionChordKey } from './src/lib/macOption';

const key = (
  key: string,
  init: { ctrl?: boolean; alt?: boolean; shift?: boolean; meta?: boolean; code?: string } = {},
) =>
  ({
    key,
    code: init.code ?? '',
    ctrlKey: !!init.ctrl,
    altKey: !!init.alt,
    shiftKey: !!init.shift,
    metaKey: !!init.meta,
  }) as KeyboardEvent;

test('events get the key names codemirror-vim and the config use', () => {
  expect(eventToken(key('g'), false)).toBe('g');
  expect(eventToken(key('G', { shift: true }), false)).toBe('G');
  expect(eventToken(key(' '), false)).toBe('<Space>');
  expect(eventToken(key('w', { ctrl: true }), false)).toBe('<C-w>');
  expect(eventToken(key('X', { ctrl: true, shift: true }), false)).toBe('<C-S-X>');
  expect(eventToken(key(']', { ctrl: true }), false)).toBe('<C-]>');
  expect(eventToken(key('Tab', { shift: true }), false)).toBe('<S-Tab>');
  expect(eventToken(key('Escape'), false)).toBe('<Esc>');
  expect(eventToken(key('Enter'), false)).toBe('<CR>');
  expect(eventToken(key('k', { meta: true }), true)).toBe('<M-k>');
  expect(eventToken(key('x', { alt: true }), false)).toBe('<A-x>');
  expect(eventToken(key('Shift'), false)).toBeNull();
});

test('Option chords on macOS are named by their physical key, unless Option types ASCII', () => {
  // Option-F types ƒ on a US layout.
  expect(eventToken(key('ƒ', { alt: true, code: 'KeyF' }), true)).toBe('<A-f>');
  expect(eventToken(key('Ï', { alt: true, shift: true, code: 'KeyF' }), true)).toBe('<A-S-F>');
  // Option-L types @ on a German layout: text, not a chord.
  expect(eventToken(key('@', { alt: true, code: 'KeyL' }), true)).toBe('@');
  expect(eventToken(key('“', { alt: true, code: 'BracketLeft' }), true)).toBe('<A-[>');
  expect(optionChordKey(key('Ï', { alt: true, shift: true, code: 'KeyF' }))).toBe('f');
  expect(optionChordKey(key('@', { alt: true, code: 'KeyL' }))).toBeNull();
  expect(optionChordKey(key('f', { code: 'KeyF' }))).toBeNull();
  // Real events keep their properties as getters on the prototype.
  class Event {
    get key() {
      return 'Ï';
    }
    get code() {
      return 'KeyF';
    }
    get altKey() {
      return true;
    }
    get shiftKey() {
      return true;
    }
    get ctrlKey() {
      return false;
    }
    get metaKey() {
      return false;
    }
  }
  expect(optionChordKey(new Event() as unknown as KeyboardEvent)).toBe('f');
});

const entry = (keys: string[], action: string) => ({ keys, action, rhs: null, desc: null });

test('steps follow Vim: the longest mapping wins, and others replay', () => {
  const table = new KeyTable([
    entry(['g', 'd'], 'editor-go-to-definition'),
    entry(['g', 'r', 'n'], 'editor-rename'),
    entry(['<Space>', 'h'], 'editor-hover'),
    entry(['<Space>', 'h', 's'], 'editor-stage-hunk'),
  ]);
  expect(step(table, [], 'x')).toEqual({ kind: 'pass' });
  expect(step(table, [], 'g')).toEqual({ kind: 'wait', keys: ['g'], ambiguous: false });
  expect(step(table, ['g'], 'd')).toMatchObject({ kind: 'run', entry: { action: 'editor-go-to-definition' } });
  expect(step(table, ['g'], 'g')).toEqual({ kind: 'replay', keys: ['g', 'g'] });
  expect(step(table, ['g'], 'r')).toEqual({ kind: 'wait', keys: ['g', 'r'], ambiguous: false });
  expect(step(table, ['<Space>'], 'h')).toEqual({ kind: 'wait', keys: ['<Space>', 'h'], ambiguous: true });
  expect(step(table, ['<Space>', 'h'], 's')).toMatchObject({ kind: 'run', entry: { action: 'editor-stage-hunk' } });
});

test('tables are per mode', () => {
  const tables = keyTables([
    { keys: ['g', 'r', 'a'], mode: 'normal', action: 'editor-code-actions', rhs: null, desc: null },
    { keys: ['g', 'r', 'a'], mode: 'visual', action: 'editor-code-actions', rhs: null, desc: null },
    { keys: ['j', 'k'], mode: 'insert', action: null, rhs: ['<Esc>'], desc: null },
  ]);
  expect(tables.get('normal')?.size).toBe(1);
  expect(tables.get('insert')?.lookup(['j', 'k']).entry?.rhs).toEqual(['<Esc>']);
});

test('which-key rows name groups and describe actions', () => {
  const table = new KeyTable([
    entry(['<Space>', 'h', 's'], 'editor-stage-hunk'),
    entry(['<Space>', 'h', 'r'], 'editor-reset-hunk'),
    entry(['<Space>', 'f'], 'editor-format'),
    { keys: ['<Space>', 'x'], action: 'editor-problems', rhs: null, desc: 'Problems list' },
  ]);
  const groups = new Map([[['<Space>', 'h'].join('\u0000'), 'hunk']]);
  expect(whichKeyRows(table, ['<Space>'], groups)).toEqual([
    { key: 'f', label: 'Format', group: false },
    { key: 'h', label: 'hunk', group: true },
    { key: 'x', label: 'Problems list', group: false },
  ]);
  expect(describe(entry(['g', 'd'], 'editor-go-to-definition'))).toBe('Go to definition');
  // Mappings replace vim's own keys after the same prefix.
  const g = whichKeyRows(new KeyTable([entry(['g', 'd'], 'editor-go-to-definition')]), ['g'], new Map(), vimPresets(['g'], false, () => []));
  expect(g.find((r) => r.key === 'd')?.label).toBe('Go to definition');
  expect(g.find((r) => r.key === 'g')?.label).toBe('First line');
});

test('vim presets follow operators, counts, and registers', () => {
  const registers = () => [{ name: 'a', text: 'some\n  text' }];
  expect(vimPresets(['d'], true, registers).some((r) => r.key === 'w')).toBe(true);
  expect(vimPresets(['2', 'd', 'i'], true, registers).find((r) => r.key === 'h')?.label).toBe('Inner hunk');
  expect(vimPresets(['g', 'u', 'a'], true, registers).find((r) => r.key === 'p')?.label).toBe('Around paragraph');
  expect(vimPresets(['"'], false, registers)).toEqual([{ key: 'a', label: 'some text', group: false }]);
  expect(vimPresets(['z'], false, registers).map((r) => r.key)).toEqual(['z', 't', 'b']);
  expect(vimPresets(['x'], false, registers)).toEqual([]);
});

test('keys show as key caps and as typed', () => {
  expect(keyCapKeys('<C-w>')).toBe('ctrl+w');
  expect(keyCapKeys('<S-Tab>')).toBe('shift+tab');
  expect(keyCapKeys('<M-k>')).toBe('mod+k');
  expect(keyCapKeys('<Space>')).toBe('space');
  expect(keyCapKeys('G')).toBe('G');
  expect(showKeys(['<Space>', 'h'])).toBe('␣h');
});

test('the jump list goes back and forward across files', () => {
  const list = new JumpList();
  const at = (path: string, line: number) => ({ path, pos: line * 10, line });
  list.record(at('a', 1));
  list.record(at('b', 5));
  // From c:9 back to b:5, then a:1, then nothing further.
  expect(list.back(at('c', 9))).toEqual(at('b', 5));
  expect(list.back(at('b', 5))).toEqual(at('a', 1));
  expect(list.back(at('a', 1))).toBeNull();
  expect(list.forward(at('a', 1))).toEqual(at('b', 5));
  expect(list.forward(at('b', 5))).toEqual(at('c', 9));
  expect(list.forward(at('c', 9))).toBeNull();
  // A new jump after going back drops what forward had.
  list.back(at('c', 9));
  list.record(at('b', 5));
  expect(list.forward(at('d', 2))).toBeNull();
  expect(list.back(at('d', 2))).toEqual(at('b', 5));
  // Edits move entries.
  const edits = new JumpList();
  edits.record({ path: 'a', pos: 10, line: 2 });
  edits.map('a', ChangeSet.of([{ from: 0, insert: 'xyz' }], 20));
  expect(edits.back({ path: 'a', pos: 0, line: 1 })?.pos).toBe(13);
});
