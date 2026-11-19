// Run with `just test-frontend` (bun test).
import { expect, test } from 'bun:test';
import { toggleBlockComment, toggleComment } from '@codemirror/commands';
import { EditorState, type Transaction } from '@codemirror/state';
import { languageFor } from './src/lib/nativeEditor/languages';
import { fileSyntaxLanguage } from './src/lib/syntax';

test('the editor matches added file names ignoring case', async () => {
  for (const [path, name] of [
    ['/a/b.pyi', 'Python'],
    ['/a/.BASH_PROFILE', 'Shell'],
    ['/a/.zshrc', 'Shell'],
    ['/a/PKGBUILD', 'Shell'],
    ['/a/cargo.LOCK', 'TOML'],
    ['/a/flake.lock', 'JSON'],
    ['/a/.gitconfig', 'Properties files'],
    ['/a/b.jsonc', 'JSONC'],
  ]) {
    expect((await languageFor(path))?.name).toBe(name);
  }
});

test('previews ignore inherited names', () => {
  expect(fileSyntaxLanguage('/a/.BASHRC')).toBe('bash');
  expect(fileSyntaxLanguage('/a/Cargo.lock')).toBe('ini');
  expect(fileSyntaxLanguage('/a/b.mts')).toBe('typescript');
  for (const name of ['constructor', '__proto__', 'toString', 'a.constructor']) {
    expect(fileSyntaxLanguage(`/a/${name}`)).toBe('plaintext');
  }
});

test('JSONC comments toggle', async () => {
  const support = (await languageFor('/a/b.jsonc'))!.support;
  const run = (command: typeof toggleComment, doc: string, head = 4) => {
    let state = EditorState.create({ doc, extensions: support, selection: { anchor: 4, head } });
    command({ state, dispatch: (tr: Transaction) => (state = tr.state) });
    return state.doc.toString();
  };
  expect(run(toggleComment, '{\n  "a": 1\n}')).toBe('{\n  // "a": 1\n}');
  expect(run(toggleBlockComment, '{\n  "a": 1\n}', 10)).toBe('{\n  /* "a": 1 */\n}');
});
