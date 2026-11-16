import { expect, test } from 'bun:test';
// The VS Code extension's merges (CommonJS, embedded in the binary).
import { mergeKeybindings, mergeSettings, parseJsonc } from '../extras/vscode/merge.js';

test('parses settings with comments and trailing commas', () => {
  const text = '\uFEFF\n{\n  // line\n  "a": "//not a comment", /* block */\n  "b": [1, 2, /* x */],\n  "c": "q\\",", // c\n}\n';
  expect(parseJsonc(text)).toEqual({ a: '//not a comment', b: [1, 2], c: 'q",' });
  expect(() => parseJsonc('{"a": }')).toThrow();
});

test('settings changed here survive until the shared file changes them', () => {
  const keep = new Set<string>();
  // The first sync applies the shared file and keeps settings only made here.
  expect(mergeSettings(undefined, { a: 1, b: 1 }, { b: 2, local: true }, keep)).toEqual({ a: 1, b: 1, local: true });
  // Unchanged shared settings leave changes made here.
  const base = { a: 1, b: 1, gone: 1 };
  expect(mergeSettings(base, base, { a: 5, b: 1, gone: 1, 'btmux.syncTheme': false }, keep)).toEqual({
    a: 5,
    b: 1,
    gone: 1,
    'btmux.syncTheme': false,
  });
  // Changed, added, and removed shared settings apply.
  expect(mergeSettings(base, { a: 1, b: 2, c: 3 }, { a: 5, b: 1, gone: 1 }, keep)).toEqual({ a: 5, b: 2, c: 3 });
  // Settings btmux wrote stay.
  expect(mergeSettings(base, { a: 2, b: 1, gone: 1 }, { a: 'btmux' }, new Set(['a']))).toEqual({ a: 'btmux' });
});

test('keybindings added here stay, and shared ones follow the shared file', () => {
  const x = { key: 'ctrl+x', command: 'x' };
  const y = { key: 'ctrl+y', command: 'y' };
  const z = { key: 'ctrl+z', command: 'z' };
  const mine = { key: 'ctrl+m', command: 'm' };
  expect(mergeKeybindings(undefined, [x, y], [mine, x])).toEqual([mine, x, y]);
  expect(mergeKeybindings([x, y], [y, z], [x, mine, y])).toEqual([mine, y, z]);
});
