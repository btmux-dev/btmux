// macOS Option: text the layout types with it, or Meta.

type KeyInit = Pick<KeyboardEvent, 'key' | 'code' | 'ctrlKey' | 'altKey' | 'shiftKey' | 'metaKey'>;

/** The letter or digit printed on the physical key, or null. */
export function physicalKey(e: KeyInit): string | null {
  const m = e.code.match(/^(?:Key([A-Z])|Digit(\d))$/);
  if (!m) return null;
  return m[1] ? (e.shiftKey ? m[1] : m[1].toLowerCase()) : m[2];
}

/**
 * ASCII text that Option types on this layout (`@`, `[`, `|` on German or
 * Swiss), or null when Option should act as Meta. Pass only macOS events.
 */
export function macOptionText(e: KeyInit): string | null {
  if (!e.altKey || e.ctrlKey || e.metaKey || !/^[!-~]$/.test(e.key)) return null;
  return e.key.toLowerCase() === physicalKey(e)?.toLowerCase() ? null : e.key;
}

// Punctuation keys Option chords are bound with, by `code`.
const PUNCTUATION: Record<string, string> = {
  BracketLeft: '[',
  BracketRight: ']',
  Backslash: '\\',
  Minus: '-',
  Equal: '=',
  Semicolon: ';',
  Quote: "'",
  Comma: ',',
  Period: '.',
  Slash: '/',
  Backquote: '`',
};

/**
 * The unshifted key an Option chord is bound with (`f` for Shift-Option-F),
 * or null if this isn't one or Option types ASCII text on this layout. Pass
 * only macOS events: CodeMirror's keymaps take these chords as typed text
 * there, so they never match `Alt-…` bindings.
 */
export function optionChordKey(e: KeyInit): string | null {
  if (!e.altKey || e.ctrlKey || e.metaKey || macOptionText(e) !== null) return null;
  // Not a spread: an event's properties are getters on its prototype.
  return (
    physicalKey({ key: e.key, code: e.code, ctrlKey: false, altKey: true, shiftKey: false, metaKey: false }) ??
    PUNCTUATION[e.code] ??
    null
  );
}
