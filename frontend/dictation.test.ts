// Run with `just test-frontend` (bun test).
import { expect, test } from 'bun:test';
import { textEdit } from './src/lib/dictation';

test('dictation edits become backspaces and text', () => {
  expect(textEdit('', 'He')).toEqual({ deletes: 0, text: 'He' });
  expect(textEdit('He', 'Hello world')).toEqual({ deletes: 0, text: 'llo world' });
  expect(textEdit('Hello world', 'Hello, world.')).toEqual({ deletes: 6, text: ', world.' });
  expect(textEdit('ok 👍🏽', 'ok')).toEqual({ deletes: 3, text: '' });
});
