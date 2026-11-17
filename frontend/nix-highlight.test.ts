import { describe, expect, test } from 'bun:test';
import { classHighlighter, highlightTree } from '@lezer/highlight';
import { nixLanguage } from './src/lib/nativeEditor/nix';

/** Each highlighted span as `text=class`, and whether the parse has errors. */
function highlight(source: string) {
  const tree = nixLanguage.parser.parse(source);
  const spans: string[] = [];
  highlightTree(tree, classHighlighter, (from, to, classes) => spans.push(`${source.slice(from, to)}=${classes}`));
  let errors = false;
  tree.iterate({ enter: (node) => void (node.type.isError && (errors = true)) });
  return { spans, errors };
}

describe('nix strings', () => {
  test("''$ escapes a dollar in an indented string", () => {
    const { spans, errors } = highlight('{\n  script = \'\'\n    echo "\'\'${HOME}"\n  \'\';\n  enable = true;\n}\n');
    expect(errors).toBe(false);
    expect(spans).toContain('enable=tok-propertyName');
    expect(spans).toContain('true=tok-bool');
    expect(spans).not.toContain('HOME=tok-propertyName');
  });

  test("''' and ''\\x escape in an indented string", () => {
    const { spans, errors } = highlight("{ a = '' x ''' y ''\\n z ''; b = 1; }");
    expect(errors).toBe(false);
    expect(spans).toContain('b=tok-propertyName');
    expect(spans).toContain('1=tok-number');
  });

  test('$${ is literal, ${ interpolates', () => {
    const { spans, errors } = highlight('{ a = "$${x} ${y}"; b = \'\'$${z} ${w}\'\'; c = "q\\"r"; d = 1; }');
    expect(errors).toBe(false);
    expect(spans).not.toContain('x=tok-propertyName');
    expect(spans).not.toContain('z=tok-propertyName');
    expect(spans).toContain('y=tok-propertyName');
    expect(spans).toContain('w=tok-propertyName');
    expect(spans).toContain('d=tok-propertyName');
  });

  test("a backslash doesn't escape '' in an indented string", () => {
    const { spans, errors } = highlight("{ a = '' x\\''; b = 1; }");
    expect(errors).toBe(false);
    expect(spans).toContain('b=tok-propertyName');
  });
});
