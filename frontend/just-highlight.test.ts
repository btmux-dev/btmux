import { describe, expect, test } from 'bun:test';
import { LanguageDescription } from '@codemirror/language';
import { highlightTree, tagHighlighter, tags as t } from '@lezer/highlight';
import { EditorState } from '@codemirror/state';
import { ensureSyntaxTree, getIndentation } from '@codemirror/language';
import { justLanguage } from './src/lib/nativeEditor/just';
import { languages } from './src/lib/nativeEditor/languages';

function state(source: string) {
  return EditorState.create({ doc: source, extensions: justLanguage });
}

const highlighter = tagHighlighter([
  { tag: t.function(t.definition(t.variableName)), class: 'recipe' },
  { tag: t.function(t.variableName), class: 'call' },
  { tag: t.definition(t.variableName), class: 'def' },
  { tag: t.variableName, class: 'var' },
  { tag: t.attributeName, class: 'attr' },
  { tag: t.propertyName, class: 'prop' },
  { tag: t.modifier, class: 'mod' },
  { tag: t.special(t.brace), class: 'interp' },
  { tag: t.keyword, class: 'kw' },
  { tag: t.bool, class: 'bool' },
  { tag: t.string, class: 'str' },
  { tag: t.special(t.string), class: 'cmd' },
  { tag: t.comment, class: 'comment' },
  { tag: t.meta, class: 'meta' },
  { tag: t.escape, class: 'esc' },
]);

/** Each highlighted span as `text=class`. */
function highlight(source: string) {
  const s = state(source);
  const tree = ensureSyntaxTree(s, s.doc.length, 5000)!;
  const spans: string[] = [];
  highlightTree(tree, highlighter, (from, to, classes) => spans.push(`${source.slice(from, to)}=${classes}`));
  return spans;
}

describe('just', () => {
  test('matches justfile names', () => {
    for (const name of ['justfile', 'Justfile', '.justfile', 'JUSTFILE', 'build.just']) {
      expect(LanguageDescription.matchFilename(languages, name)?.name).toBe('Just');
    }
    expect(LanguageDescription.matchFilename(languages, 'notjustfile')?.name).not.toBe('Just');
  });

  test('settings, assignments, and recipes', () => {
    const spans = highlight(
      [
        'set dotenv-load := true',
        'export PORT := env("PORT", "8000")',
        '# Build it',
        '[group("dev")]',
        '@build target="debug" *args: (fetch "x") && check',
        '    cargo build --profile {{target}} {{args}}',
        '',
        'check:',
        '  -echo "{{ PORT }} $HOME"',
      ].join('\n'),
    );
    expect(spans).toContain('set=kw');
    expect(spans).toContain('dotenv-load=prop');
    expect(spans).toContain('true=bool');
    expect(spans).toContain('PORT=def');
    expect(spans).toContain('env=call');
    expect(spans).toContain('# Build it=comment');
    expect(spans).toContain('group=attr');
    expect(spans).toContain('build=recipe');
    expect(spans).toContain('fetch=call');
    expect(spans).toContain('check=call');
    expect(spans).toContain('{{=interp');
    expect(spans).toContain('target=var');
    expect(spans).toContain('-=mod');
    expect(spans).toContain('@=mod');
    expect(spans).toContain('check=recipe');
    expect(spans).toContain('$HOME=def');
  });

  test('shell bodies, other interpreters, and strings', () => {
    const spans = highlight(
      [
        'a:',
        "  echo 'quoted {{x}} still' # done",
        'b:',
        '  #!/usr/bin/env python3',
        '  print("# not a comment")',
        "[script('node')]",
        'c:',
        '  console.log("# nor this")',
        "s := '''",
        'multi # line',
        "'''",
        "after := f'{{s}} x'",
      ].join('\n'),
    );
    expect(spans).toContain("'quoted =str");
    expect(spans).toContain(" still'=str");
    expect(spans).toContain('# done=comment');
    expect(spans).toContain('#!/usr/bin/env python3=meta');
    expect(spans.some((s) => s.startsWith('# not'))).toBe(false);
    expect(spans.some((s) => s.startsWith('# nor'))).toBe(false);
    expect(spans).toContain('multi # line=str');
    expect(spans).toContain('after=def');
    expect(spans).toContain('s=var');
  });

  test('set shell and script-interpreter pick the body language', () => {
    const spans = highlight(
      [
        "set script-interpreter := ['powershell.exe', '-File']",
        'a:',
        "  echo '# shell'",
        '[script]',
        'b:',
        "  Write-Host '# powershell'",
        "set shell := ['nu', '-c']",
        'c:',
        "  echo '# nu'",
      ].join('\n'),
    );
    expect(spans).toContain("'# shell'=str");
    expect(spans.some((s) => s.includes('# powershell'))).toBe(false);
    expect(spans.some((s) => s.includes('# nu'))).toBe(false);
  });

  test('indents recipe bodies', () => {
    const s = state('build:\n    cargo build\n    x\n');
    expect(getIndentation(s, s.doc.line(3).from)).toBe(4);
    expect(getIndentation(s, s.doc.line(4).from)).toBe(4);
    const fresh = state('build:\n');
    expect(getIndentation(fresh, fresh.doc.line(2).from)).toBe(2);
  });
});
