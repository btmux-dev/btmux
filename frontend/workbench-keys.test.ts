// Run with `just test-frontend` (bun test).
import { expect, test } from 'bun:test';
import { workbenchKey } from './src/lib/workbenchKeys';

const press = (key: string, code: string, mods: { meta?: boolean; ctrl?: boolean; shift?: boolean; alt?: boolean }) => ({
  key,
  code,
  metaKey: !!mods.meta,
  ctrlKey: !!mods.ctrl,
  shiftKey: !!mods.shift,
  altKey: !!mods.alt,
});

const action = (e: ReturnType<typeof press>, mac: boolean) => workbenchKey(e, mac)?.action;

test('macOS chords', () => {
  expect(action(press('P', 'KeyP', { meta: true, shift: true }), true)).toBe('command-palette');
  expect(action(press('p', 'KeyP', { meta: true }), true)).toBe('editor-quick-open');
  expect(action(press('E', 'KeyE', { meta: true, shift: true }), true)).toBe('file-browser');
  expect(action(press('G', 'KeyG', { ctrl: true, shift: true }), true)).toBe('git-view');
  expect(action(press('~', 'Backquote', { ctrl: true, shift: true }), true)).toBe('new-window');
  expect(action(press(',', 'Comma', { meta: true }), true)).toBe('open-settings');
  expect(action(press('f', 'KeyF', { meta: true }), true)).toBe('search-pane');
  expect(action(press('\\', 'Backslash', { meta: true }), true)).toBe('split-vertical');
});

test('Windows and Linux chords', () => {
  expect(action(press('P', 'KeyP', { ctrl: true, shift: true }), false)).toBe('command-palette');
  expect(action(press('E', 'KeyE', { ctrl: true, shift: true }), false)).toBe('file-browser');
  expect(action(press('G', 'KeyG', { ctrl: true, shift: true }), false)).toBe('git-view');
  expect(action(press('~', 'Backquote', { ctrl: true, shift: true }), false)).toBe('new-window');
  expect(action(press(',', 'Comma', { ctrl: true }), false)).toBe('open-settings');
  expect(action(press('%', 'Digit5', { ctrl: true, shift: true }), false)).toBe('split-vertical');
});

test('plain Ctrl chords stay the shell’s', () => {
  for (const [key, code] of [
    ['p', 'KeyP'],
    ['f', 'KeyF'],
    ['k', 'KeyK'],
    ['\\', 'Backslash'],
  ])
    expect(action(press(key, code, { ctrl: true }), false)).toBeUndefined();
});

test('modifiers must match exactly', () => {
  expect(action(press('P', 'KeyP', { meta: true, shift: true, alt: true }), true)).toBeUndefined();
  expect(action(press('P', 'KeyP', { meta: true, ctrl: true, shift: true }), true)).toBeUndefined();
  expect(action(press('F', 'KeyF', { meta: true, shift: true }), true)).toBeUndefined();
  expect(action(press('e', 'KeyE', { meta: true }), true)).toBeUndefined();
});
