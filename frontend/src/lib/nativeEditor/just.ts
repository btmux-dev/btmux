import { LanguageSupport, StreamLanguage, type StreamParser, type StringStream } from '@codemirror/language';
import { shell } from '@codemirror/legacy-modes/mode/shell';

// Justfiles: recipes, settings, and expressions, with recipe bodies
// highlighted as shell unless a shebang or `[script(…)]` names another
// interpreter. `{{ … }}` interpolates in bodies and format strings.

type StringFrame = { kind: 'string'; close: string; escapes: boolean; format: boolean; style: string };
type Frame = StringFrame | { kind: 'interp' };

/** What the rest of a top-level line is. */
type Line = 'header' | 'params' | 'deps' | 'attr' | 'decl' | 'setting' | 'assign' | 'expr';

interface JustState {
  line: Line;
  /** Open strings and interpolations, innermost last. */
  stack: Frame[];
  /** Open parens and brackets, which continue an expression's line. */
  depth: number;
  /** The last line was a recipe header, so indented lines are its body. */
  header: boolean;
  body: boolean;
  /** The body's first line hasn't been read (a shebang may follow). */
  bodyStart: boolean;
  /** The body's indentation. */
  bodyIndent: number | null;
  /** Whether the next recipe's body is shell, from `[script…]`. */
  recipeShell: boolean | null;
  /** Whether `set shell` and `set script-interpreter` are shells. */
  setShell: boolean;
  scriptShell: boolean;
  /** The body's shell state, or null for another interpreter. */
  shell: unknown;
  /** The last body line ended with `\`. */
  continued: boolean;
}

const KEYWORDS = /^(alias|export|unexport|set|import\??|mod\??)(?=\s)/;
const IDENT = /^[A-Za-z_][\w-]*/;
const SHELLS = /^(ba|z|da|k|mk|y)?sh$/;

const shellParser = shell as StreamParser<{ tokens: unknown[] }>;

function isShell(interpreter: string): boolean {
  const words = interpreter.trim().split(/\s+/);
  let program = words[0]?.slice(words[0].lastIndexOf('/') + 1) ?? '';
  if (program === 'env') program = words.find((w, i) => i > 0 && !w.startsWith('-')) ?? '';
  return SHELLS.test(program);
}

function copyShell(state: unknown): unknown {
  return state && { tokens: (state as { tokens: unknown[] }).tokens.slice() };
}

/** A string starting at the stream, as a frame, after reading its opening. */
function openString(stream: StringStream, prefix: string): StringFrame | null {
  for (const close of ["'''", '"""', '```', "'", '"', '`']) {
    if (stream.match(close)) {
      const backtick = close[0] === '`';
      return {
        kind: 'string',
        close,
        escapes: close[0] === '"',
        format: prefix === 'f',
        style: backtick ? 'string.special' : 'string',
      };
    }
  }
  return null;
}

function readString(stream: StringStream, state: JustState, frame: StringFrame): string {
  while (!stream.eol()) {
    if (stream.match(frame.close)) {
      state.stack.pop();
      return frame.style;
    }
    if (frame.format && stream.match('{{{{', false)) {
      stream.pos += 4;
      continue;
    }
    if (frame.format && stream.match('{{', false)) {
      if (stream.pos > stream.start) return frame.style;
      stream.pos += 2;
      state.stack.push({ kind: 'interp' });
      return 'brace.special';
    }
    if (frame.escapes && stream.peek() === '\\') {
      if (stream.pos > stream.start) return frame.style;
      stream.next();
      stream.next();
      return 'escape';
    }
    stream.next();
  }
  // Only `"` strings continue past a line's end, after a backslash.
  if (frame.close.length === 1 && !(frame.escapes && /\\$/.test(stream.string))) state.stack.pop();
  return frame.style;
}

/** One token of an expression, or of a top-level line's other parts. */
function expression(stream: StringStream, state: JustState): string | null {
  if (stream.eatSpace()) return null;
  const ch = stream.peek()!;
  if (ch === '#' && state.stack.length === 0) {
    stream.skipToEnd();
    return 'comment';
  }
  const prefixed = stream.match(/^[xf](?=['"`])/);
  const string = openString(stream, prefixed ? stream.current() : '');
  if (string) {
    state.stack.push(string);
    return string.style;
  }
  if (stream.match('}}')) {
    state.stack.pop();
    return 'brace.special';
  }
  if (stream.match(/^(:=|==|!=|=~|&&|\|\||[+/=?!*$@-])/)) {
    const op = stream.current();
    // `&&` in a header starts subsequent dependencies.
    if (op === '&&' && state.line === 'params') state.line = 'deps';
    return 'operator';
  }
  if (ch === ':') {
    stream.next();
    if (state.line === 'params') state.line = 'deps';
    return 'punctuation';
  }
  if (ch === '(' || ch === '[') {
    stream.next();
    state.depth++;
    return ch === '[' ? 'squareBracket' : 'paren';
  }
  if (ch === ')' || ch === ']') {
    stream.next();
    state.depth = Math.max(0, state.depth - 1);
    return ch === ']' ? 'squareBracket' : 'paren';
  }
  if (ch === ',') {
    stream.next();
    return 'separator';
  }
  if (stream.match(IDENT)) {
    const word = stream.current();
    switch (state.line) {
      case 'header':
        state.line = 'params';
        return 'variableName.function.definition';
      case 'deps':
        return 'variableName.function';
      case 'attr':
        if (state.depth === 1) return 'attributeName';
        break;
      case 'decl':
        state.line = 'expr';
        return 'variableName.definition';
      case 'setting':
        state.line = 'expr';
        return 'propertyName';
      case 'assign':
        state.line = 'expr';
        return 'variableName.definition';
    }
    if (word === 'if' || word === 'else') return 'controlKeyword';
    if (word === 'true' || word === 'false') return 'bool';
    if (stream.match(/^\s*\(/, false)) return 'variableName.function';
    return 'variableName';
  }
  stream.next();
  return null;
}

/** One token of a recipe body. */
function body(stream: StringStream, state: JustState): string | null {
  if (stream.sol()) {
    if (state.bodyIndent === null) state.bodyIndent = stream.indentation();
    if (stream.eatSpace()) return null;
  }
  if (state.bodyStart) {
    state.bodyStart = false;
    if (stream.match('#!')) {
      if (!isShell(stream.string.slice(stream.pos))) state.shell = null;
      stream.skipToEnd();
      return 'meta';
    }
  }
  const lineStart = !stream.string.slice(0, stream.pos).trim();
  const inShellToken = !!(state.shell as { tokens: unknown[] } | null)?.tokens.length;
  if (lineStart && !state.continued && !inShellToken && stream.match(/^[@-]+/)) return 'modifier';
  if (stream.match('{{{{')) return 'escape';
  if (stream.match('{{')) {
    state.stack.push({ kind: 'interp' });
    return 'brace.special';
  }
  // Leave the next interpolation to this mode.
  const rest = stream.string.slice(stream.pos);
  const next = rest.indexOf('{{');
  const end = next < 0 ? stream.string.length : stream.pos + next;
  let style: string | null = null;
  if (state.shell) {
    const line = stream.string;
    stream.string = line.slice(0, end);
    style = shellParser.token(stream, state.shell as { tokens: unknown[] });
    stream.string = line;
  } else {
    stream.pos = end;
  }
  if (stream.pos === stream.start) stream.next();
  return style;
}

/** Classify a top-level line from its start. */
function startLine(stream: StringStream, state: JustState): string | null {
  const text = stream.string;
  state.body = false;
  state.shell = null;
  if (text.startsWith('[')) {
    state.line = 'attr';
    const script = /\bscript\b(?:\s*\(\s*['"]([^'"]*))?/.exec(text);
    if (script) state.recipeShell = script[1] === undefined ? state.scriptShell : isShell(script[1]);
    return null;
  }
  const keyword = KEYWORDS.exec(text);
  if (keyword && !/^\S+\s*:(?!=)/.test(text)) {
    stream.pos = keyword[0].length;
    state.line = keyword[1] === 'set' ? 'setting' : 'decl';
    const interpreter = /^set\s+(shell|script-interpreter)\s*:=\s*\[\s*['"]([^'"]*)/.exec(text);
    if (interpreter?.[1] === 'shell') state.setShell = isShell(interpreter[2]);
    if (interpreter?.[1] === 'script-interpreter') state.scriptShell = isShell(interpreter[2]);
    return 'keyword';
  }
  if (/^[A-Za-z_][\w-]*\s*:=/.test(text)) {
    state.line = 'assign';
    return null;
  }
  if (/^@?[A-Za-z_]/.test(text)) {
    state.line = 'header';
    state.header = true;
    state.bodyIndent = null;
    state.shell = (state.recipeShell ?? state.setShell) ? shellParser.startState!(0) : null;
    state.recipeShell = null;
    if (stream.eat('@')) return 'modifier';
    return null;
  }
  state.line = 'expr';
  return null;
}

export const justParser: StreamParser<JustState> = {
  name: 'just',
  startState: () => ({
    line: 'expr',
    stack: [],
    depth: 0,
    header: false,
    body: false,
    bodyStart: false,
    bodyIndent: null,
    recipeShell: null,
    setShell: true,
    scriptShell: true,
    shell: null,
    continued: false,
  }),
  copyState: (state) => ({ ...state, stack: state.stack.slice(), shell: copyShell(state.shell) }),
  token(stream, state) {
    const style = token(stream, state);
    if (state.body && stream.eol()) state.continued = /\\$/.test(stream.string);
    return style;
  },
  indent(state, _textAfter, cx) {
    if (state.stack.length || state.depth) return cx.unit;
    if (state.header || state.body) return state.bodyIndent ?? cx.unit;
    return 0;
  },
  languageData: {
    commentTokens: { line: '#' },
    closeBrackets: { brackets: ['(', '[', '{', "'", '"', '`'] },
  },
};

function token(stream: StringStream, state: JustState): string | null {
  const top = state.stack[state.stack.length - 1];
  if (top?.kind === 'string') return readString(stream, state, top);
  if (stream.sol() && !top && state.depth === 0) {
    const indented = /^[ \t]/.test(stream.string);
    if (indented && (state.body || state.header)) {
      if (state.header) {
        state.body = true;
        state.bodyStart = true;
        state.continued = false;
        state.line = 'expr';
      }
      state.header = false;
    } else if (indented || !stream.string.trim()) {
      if (stream.eatSpace()) return null;
    } else if (!stream.string.startsWith('#')) {
      state.header = false;
      const style = startLine(stream, state);
      if (stream.pos > stream.start) return style;
    } else {
      stream.skipToEnd();
      return stream.string.startsWith('#!') ? 'meta' : 'comment';
    }
  }
  if (state.body && !top) return body(stream, state);
  return expression(stream, state);
}

export const justLanguage = StreamLanguage.define(justParser);

export function just(): LanguageSupport {
  return new LanguageSupport(justLanguage);
}
