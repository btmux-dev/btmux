import { describe, expect, test } from 'bun:test';
import { ensureSyntaxTree, getIndentation, LanguageDescription } from '@codemirror/language';
import { EditorState } from '@codemirror/state';
import { highlightTree, tagHighlighter, tags as t } from '@lezer/highlight';
import { just } from './src/lib/nativeEditor/just';
import { languages } from './src/lib/nativeEditor/languages';

function state(source: string) {
  return EditorState.create({ doc: source, extensions: just() });
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
  { tag: t.brace, class: 'brace' },
  { tag: t.keyword, class: 'kw' },
  { tag: t.bool, class: 'bool' },
  { tag: t.string, class: 'str' },
  { tag: t.special(t.string), class: 'cmd' },
  { tag: t.comment, class: 'comment' },
  { tag: t.meta, class: 'meta' },
  { tag: t.escape, class: 'esc' },
]);

/** Each highlighted span of a state as `text=class`. */
function spansOf(s: EditorState) {
  const source = s.doc.toString();
  const tree = ensureSyntaxTree(s, s.doc.length, 5000)!;
  const spans: string[] = [];
  highlightTree(tree, highlighter, (from, to, classes) => spans.push(`${source.slice(from, to)}=${classes}`));
  return spans;
}

const highlight = (...lines: string[]) => spansOf(state(lines.join('\n')));

/** Whether a span contains `text` at all, i.e. it was highlighted. */
const styled = (spans: string[], text: string) => spans.some((s) => s.includes(text));

describe('just', () => {
  test('matches justfile names', () => {
    for (const name of ['justfile', 'Justfile', '.justfile', 'JUSTFILE', '.JUSTFILE', 'build.just']) {
      expect(LanguageDescription.matchFilename(languages, name)?.name).toBe('Just');
    }
    for (const name of ['notjustfile', 'notes.justfile']) {
      expect(LanguageDescription.matchFilename(languages, name)?.name).not.toBe('Just');
    }
  });

  test('settings, assignments, and recipes', () => {
    const spans = highlight(
      'set dotenv-load := true',
      'export PORT := env("PORT", "8000")',
      '# Build it',
      '[group("dev")]',
      '@build target="debug" *args: (fetch "x") && check',
      '    cargo build --profile {{target}} {{args}}',
      '',
      'check:',
      '  -echo "{{ PORT }} $HOME"',
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

  test('quoted strings of every kind span lines', () => {
    const spans = highlight(
      "name := 'one",
      "two'",
      'other := "three',
      'four"',
      'cmd := `echo',
      'five`',
      'build:',
      '  echo {{name}} $HOME',
    );
    expect(spans).toContain("two'=str");
    expect(spans).toContain('four"=str');
    expect(spans).toContain('five`=cmd');
    expect(spans).toContain('build=recipe');
    expect(spans).toContain('$HOME=def');
  });

  test('interpreter settings apply to the whole file', () => {
    const shellAfter = highlight('demo:', '  print("hello # world")', "set shell := ['python3', '-c']");
    expect(styled(shellAfter, '# world')).toBe(false);

    const split = highlight('set shell := [', '  "python3",', '  "-c",', ']', 'demo:', '  print("# x")');
    expect(styled(split, '# x')).toBe(false);

    const scripts = highlight(
      "set script-interpreter := ['powershell.exe', '-File']",
      'a:',
      "  echo '# shell'",
      '[script]',
      'b:',
      "  Write-Host '# powershell'",
    );
    expect(scripts).toContain("'# shell'=str");
    expect(styled(scripts, '# powershell')).toBe(false);

    const defaultScript = highlight(
      "set script-interpreter := ['python3']",
      'set default-script',
      'a:',
      '  print("# x")',
    );
    expect(styled(defaultScript, '# x')).toBe(false);
    const notDefault = highlight(
      "set script-interpreter := ['python3']",
      'set default-script := false',
      'a:',
      '  echo "# x"',
    );
    expect(notDefault).toContain('"# x"=str');

    const windows = highlight('[windows]', "set shell := ['powershell.exe', '-c']", 'a:', '  echo "# x"');
    expect(windows).toContain('"# x"=str');
  });

  test('changing a setting rehighlights recipes above it', () => {
    const before = state('a:\n  echo "# x"\n');
    expect(spansOf(before)).toContain('"# x"=str');
    const after = before.update({
      changes: { from: before.doc.length, insert: "set shell := ['python3', '-c']\n" },
    }).state;
    expect(styled(spansOf(after), '# x')).toBe(false);
  });

  test('a shell shebang overrides a non-shell default', () => {
    const spans = highlight("set shell := ['python3', '-c']", 'a:', '  #!/usr/bin/env bash', '  echo "$HOME"');
    expect(spans).toContain('$HOME=def');
  });

  test('the script attribute is read by name', () => {
    const group = highlight("set script-interpreter := ['python3']", "[group('script')]", 'a:', '  echo "# x"');
    expect(group).toContain('"# x"=str');
    const colon = highlight("[script: 'python3']", 'a:', '  print("# x")');
    expect(styled(colon, '# x')).toBe(false);
    const split = highlight('[script(', "  'python3'", ')]', 'a:', '  print("# x")');
    expect(styled(split, '# x')).toBe(false);
    const both = highlight("[script, group('x')]", 'a:', '  echo "# x"');
    expect(both).toContain('"# x"=str');
    expect(both).toContain('group=attr');
  });

  test('a heredoc ends at the recipe indentation', () => {
    const spans = highlight('build:', '  #!/usr/bin/env bash', "  cat <<'EOF'", '  value', '  EOF', '  echo "$HOME"');
    expect(spans).toContain('$HOME=def');
  });

  test('interpolation ends after nested expression braces', () => {
    const spans = highlight(
      'build:',
      '  echo {{if "x" == "x" {if "x" == "y" {"hi"} else {"there"}} else {"bye"}}} "$HOME"',
      'a := if "x" == "x" {',
      '  "y"',
      '} else { "z" }',
      'next:',
    );
    expect(spans).toContain('"bye"=str');
    // The four closing braces of the if-blocks, then the interpolation's.
    const braces = spans.filter((s) => s.endsWith('=brace')).map((s) => s.slice(0, -'=brace'.length));
    expect(braces.join('').replace(/[^}]/g, '')).toHaveLength(4 + 2);
    expect(spans.filter((s) => s === '}}=interp')).toHaveLength(1);
    expect(spans).toContain('$HOME=def');
    expect(spans).toContain('next=recipe');
  });

  test('indents recipe bodies', () => {
    const s = state('build:\n    cargo build\n    x\n');
    expect(getIndentation(s, s.doc.line(3).from)).toBe(4);
    expect(getIndentation(s, s.doc.line(4).from)).toBe(4);
    const fresh = state('build:\n');
    expect(getIndentation(fresh, fresh.doc.line(2).from)).toBe(2);
  });
});
