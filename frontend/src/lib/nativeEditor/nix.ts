import { LanguageSupport } from '@codemirror/language';
import { ExternalTokenizer, type InputStream, type LRParser } from '@lezer/lr';
import { nix as replitNix, nixLanguage as replitLanguage } from '@replit/codemirror-lang-nix';

// Its string tokenizers miss Nix's escapes (`''$`, `'''`, `''\x`, `$$`), so
// e.g. `''${HOME}` ends an indented string. These follow Nix's lexer instead.
// Term ids are the package's generated ones (pinned at 6.0.1).
const STR_CONTENT = 63;
const STR_DOLLAR_BRACE = 64;
const STR_END = 65;
const IND_STR_CONTENT = 67;
const IND_STR_DOLLAR_BRACE = 68;
const IND_STR_END = 69;

const QUOTE = 34;
const DOLLAR = 36;
const APOSTROPHE = 39;
const BACKSLASH = 92;
const BRACE_L = 123;

/** Ends a run of string content, or (with none) takes `token` of `length`. */
function finish(input: InputStream, n: number, content: number, token: number, length: number) {
  if (n > 0) {
    input.acceptToken(content);
  } else {
    input.advance(length);
    input.acceptToken(token);
  }
}

const scanString = new ExternalTokenizer((input) => {
  let n = 0;
  for (;;) {
    const c = input.next;
    if (c < 0) break;
    if (c === QUOTE) return finish(input, n, STR_CONTENT, STR_END, 1);
    if (c === DOLLAR && input.peek(1) === BRACE_L) return finish(input, n, STR_CONTENT, STR_DOLLAR_BRACE, 2);
    // `\x`, and `$$` (so `$${` is literal).
    const length = c === BACKSLASH || (c === DOLLAR && input.peek(1) === DOLLAR) ? 2 : 1;
    input.advance(length);
    n += length;
  }
  if (n > 0) input.acceptToken(STR_CONTENT);
});

const scanIndString = new ExternalTokenizer((input) => {
  let n = 0;
  for (;;) {
    const c = input.next;
    if (c < 0) break;
    if (c === APOSTROPHE && input.peek(1) === APOSTROPHE) {
      const e = input.peek(2);
      // `'''`, `''$`, and `''\x` escape; `''` alone ends the string.
      const length = e === APOSTROPHE || e === DOLLAR ? 3 : e === BACKSLASH ? 4 : 0;
      if (!length) return finish(input, n, IND_STR_CONTENT, IND_STR_END, 2);
      input.advance(length);
      n += length;
      continue;
    }
    if (c === DOLLAR && input.peek(1) === BRACE_L) {
      return finish(input, n, IND_STR_CONTENT, IND_STR_DOLLAR_BRACE, 2);
    }
    // `$$`, so `$${` is literal; `\` is not an escape here.
    const length = c === DOLLAR && input.peek(1) === DOLLAR ? 2 : 1;
    input.advance(length);
    n += length;
  }
  if (n > 0) input.acceptToken(IND_STR_CONTENT);
});

// The package doesn't export its tokenizers; the parser lists them first.
const [stringFrom, indStringFrom] = (replitLanguage.parser as LRParser & { tokenizers: ExternalTokenizer[] })
  .tokenizers;

export const nixLanguage = replitLanguage.configure({
  tokenizers: [
    { from: stringFrom, to: scanString },
    { from: indStringFrom, to: scanIndString },
  ],
});

/** The package's support (snippets, language data) with fixed strings. */
export function nix(): LanguageSupport {
  return new LanguageSupport(nixLanguage, replitNix().support);
}
