import { language, LanguageSupport, StreamLanguage, type StreamParser, type StringStream } from '@codemirror/language';
import { shell } from '@codemirror/legacy-modes/mode/shell';
import { Prec } from '@codemirror/state';

// Justfiles: recipes, settings, and expressions, with recipe bodies
// highlighted as shell unless a shebang, `[script…]`, or a setting names
// another interpreter. `{{ … }}` interpolates in bodies and format strings.

/** Whether the file's interpreter settings name shells. */
export interface JustSettings {
  /** `set shell`, for recipes. */
  shell: boolean;
  /** `set script-interpreter`, for `[script]` recipes. */
  script: boolean;
  /** `set default-script`: recipes are scripts unless marked otherwise. */
  defaultScript: boolean;
}

const DEFAULTS: JustSettings = { shell: true, script: true, defaultScript: false };

type StringFrame = { kind: 'string'; close: string; escapes: boolean; format: boolean; style: string };
/** An interpolation, with the expression braces open inside it. */
type InterpFrame = { kind: 'interp'; braces: number };
type Frame = StringFrame | InterpFrame;

/** What the rest of a top-level line is. */
type Line = 'header' | 'params' | 'deps' | 'attr' | 'decl' | 'setting' | 'assign' | 'expr';

interface JustState {
  line: Line;
  /** Open strings and interpolations, innermost last. Frames are never mutated. */
  stack: Frame[];
  /** Open parens, brackets, and braces, which continue an expression's line. */
  depth: number;
  /** The last line was a recipe header, so indented lines are its body. */
  header: boolean;
  body: boolean;
  /** The body's first line hasn't been read (a shebang may follow). */
  bodyStart: boolean;
  /** The body's indentation, in columns and in characters. */
  bodyIndent: number | null;
  bodyStrip: number;
  /** Whether the next recipe's body is shell, from a `[script]` attribute. */
  recipeShell: boolean | null;
  /** The next string in an attribute line is `script`'s interpreter. */
  scriptArg: boolean;
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

/** The interpreter settings anywhere in a justfile, as they apply to all of it. */
export function justSettings(text: string): JustSettings {
  const settings = { ...DEFAULTS };
  const lines = text.split('\n');
  for (let i = 0; i < lines.length; i++) {
    const name = /^set[ \t]+(shell|script-interpreter|default-script)(?![\w-])/.exec(lines[i]);
    if (!name) continue;
    // Attributes above it: one for Windows only doesn't apply elsewhere.
    let windows = false;
    for (let j = i - 1; j >= 0 && lines[j].startsWith('['); j--) {
      if (/\bwindows\b/.test(lines[j])) windows = true;
    }
    if (windows) continue;
    const rest = lines.slice(i).join('\n').slice(name[0].length);
    if (name[1] === 'default-script') {
      settings.defaultScript = !/^[ \t]*:=[ \t]*false\b/.test(rest);
      continue;
    }
    // The array's first string, which may be on a later line.
    const program = /^[ \t]*:=[ \t]*\[(?:\s|#[^\n]*)*(['"])([^'"\n]*)\1/.exec(rest);
    if (!program) continue;
    if (name[1] === 'shell') settings.shell = isShell(program[2]);
    else settings.script = isShell(program[2]);
  }
  return settings;
}

function copyShell(state: unknown): unknown {
  return state && { tokens: (state as { tokens: unknown[] }).tokens.slice() };
}

/** Replace the innermost frame (frames are shared with copied states). */
function setTop(state: JustState, frame: Frame) {
  state.stack[state.stack.length - 1] = frame;
}

/** A string starting at the stream, as a frame, after reading its opening. */
function openString(stream: StringStream, prefix: string): StringFrame | null {
  for (const close of ["'''", '"""', '```', "'", '"', '`']) {
    if (stream.match(close)) {
      return {
        kind: 'string',
        close,
        escapes: close[0] === '"',
        format: prefix === 'f',
        style: close[0] === '`' ? 'string.special' : 'string',
      };
    }
  }
  return null;
}

/** String content; every kind of string may span lines. */
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
      state.stack.push({ kind: 'interp', braces: 0 });
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
  return frame.style;
}

/** One token of an expression, or of a top-level line's other parts. */
function expression(stream: StringStream, state: JustState, settings: JustSettings): string | null {
  if (stream.eatSpace()) return null;
  const ch = stream.peek()!;
  const top = state.stack[state.stack.length - 1] as InterpFrame | undefined;
  if (ch === '#' && !top) {
    stream.skipToEnd();
    return 'comment';
  }
  const prefixed = stream.match(/^[xf](?=['"`])/);
  const string = openString(stream, prefixed ? stream.current() : '');
  if (string) {
    if (state.scriptArg) {
      state.scriptArg = false;
      state.recipeShell = isShell(/^[^'"`]*/.exec(stream.string.slice(stream.pos))![0]);
    }
    state.stack.push(string);
    return string.style;
  }
  if (ch === '{' || ch === '}') {
    // `}}` ends an interpolation once its expression's braces are closed.
    if (ch === '}' && top && !top.braces && stream.match('}}')) {
      state.stack.pop();
      return 'brace.special';
    }
    stream.next();
    const step = ch === '{' ? 1 : -1;
    if (top) setTop(state, { kind: 'interp', braces: Math.max(0, top.braces + step) });
    else state.depth = Math.max(0, state.depth + step);
    return 'brace';
  }
  if (stream.match(/^(:=|==|!=|=~|&&|\|\||[+/=?!*$@-])/)) {
    // `&&` in a header starts subsequent dependencies.
    if (stream.current() === '&&' && state.line === 'params') state.line = 'deps';
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
    state.scriptArg = false;
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
        if (state.depth !== 1 || state.scriptArg) break;
        // `[script]` uses `script-interpreter`; `[script('…')]` or `[script: '…']` names one.
        if (word === 'script') {
          state.recipeShell = settings.script;
          state.scriptArg = true;
        }
        return 'attributeName';
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
    if (state.bodyIndent === null) {
      state.bodyIndent = stream.indentation();
      state.bodyStrip = /^[ \t]*/.exec(stream.string)![0].length;
    }
    if (stream.eatSpace()) return null;
  }
  if (state.bodyStart) {
    state.bodyStart = false;
    if (stream.match('#!')) {
      if (!isShell(stream.string.slice(stream.pos))) state.shell = null;
      else state.shell ??= shellParser.startState!(0);
      stream.skipToEnd();
      return 'meta';
    }
  }
  const lineStart = !stream.string.slice(0, stream.pos).trim();
  const inShellToken = !!(state.shell as { tokens: unknown[] } | null)?.tokens.length;
  if (lineStart && !state.continued && !inShellToken && stream.match(/^[@-]+/)) return 'modifier';
  if (stream.match('{{{{')) return 'escape';
  if (stream.match('{{')) {
    state.stack.push({ kind: 'interp', braces: 0 });
    return 'brace.special';
  }
  // Leave the next interpolation to this mode.
  const next = stream.string.indexOf('{{', stream.pos);
  const end = next < 0 ? stream.string.length : next;
  let style: string | null = null;
  if (state.shell) {
    // The shell sees the line without the recipe's indentation, as just runs
    // it, so a heredoc's delimiter starts its line.
    const line = stream.string;
    const offset = Math.min(state.bodyStrip, /^[ \t]*/.exec(line)![0].length);
    stream.string = line.slice(offset, end);
    stream.pos -= offset;
    stream.start -= offset;
    try {
      style = shellParser.token(stream, state.shell as { tokens: unknown[] });
    } finally {
      stream.string = line;
      stream.pos += offset;
      stream.start += offset;
    }
  } else {
    stream.pos = end;
  }
  if (stream.pos === stream.start) stream.next();
  return style;
}

/** Classify a top-level line from its start. */
function startLine(stream: StringStream, state: JustState, settings: JustSettings): string | null {
  const text = stream.string;
  state.body = false;
  state.shell = null;
  state.scriptArg = false;
  if (text.startsWith('[')) {
    state.line = 'attr';
    return null;
  }
  const keyword = KEYWORDS.exec(text);
  if (keyword && !/^\S+\s*:(?!=)/.test(text)) {
    stream.pos = keyword[0].length;
    state.line = keyword[1] === 'set' ? 'setting' : 'decl';
    state.recipeShell = null;
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
    const shell = state.recipeShell ?? (settings.defaultScript ? settings.script : settings.shell);
    state.shell = shell ? shellParser.startState!(0) : null;
    state.recipeShell = null;
    if (stream.eat('@')) return 'modifier';
    return null;
  }
  state.line = 'expr';
  return null;
}

function token(stream: StringStream, state: JustState, settings: JustSettings): string | null {
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
      const style = startLine(stream, state, settings);
      if (stream.pos > stream.start) return style;
    } else {
      stream.skipToEnd();
      return stream.string.startsWith('#!') ? 'meta' : 'comment';
    }
  }
  if (state.body && !top) return body(stream, state);
  return expression(stream, state, settings);
}

function parser(settings: JustSettings): StreamParser<JustState> {
  return {
    name: 'just',
    startState: () => ({
      line: 'expr',
      stack: [],
      depth: 0,
      header: false,
      body: false,
      bodyStart: false,
      bodyIndent: null,
      bodyStrip: 0,
      recipeShell: null,
      scriptArg: false,
      shell: null,
      continued: false,
    }),
    copyState: (state) => ({ ...state, stack: state.stack.slice(), shell: copyShell(state.shell) }),
    token(stream, state) {
      const style = token(stream, state, settings);
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
}

const variants = new Map<string, StreamLanguage<JustState>>();

/** The language for a file's settings: one per combination, so equal settings keep the parse. */
export function justLanguageFor(settings: JustSettings): StreamLanguage<JustState> {
  const key = `${+settings.shell}${+settings.script}${+settings.defaultScript}`;
  let found = variants.get(key);
  if (!found) variants.set(key, (found = StreamLanguage.define(parser(settings))));
  return found;
}

export const justLanguage = justLanguageFor(DEFAULTS);

/** Settings apply to the whole file, wherever they are, so a change to them reparses it. */
export function just(): LanguageSupport {
  return new LanguageSupport(
    justLanguage,
    Prec.high(language.compute(['doc'], (state) => justLanguageFor(justSettings(state.doc.toString())))),
  );
}
