import type { BrowserClientMessage } from '../generated/protocol';
import type { ParsedKey } from './prefixKey';

const DEFAULT_SEARCH_URL = 'https://duckduckgo.com/?q=%s';

type Modifiers = Pick<KeyboardEvent, 'altKey' | 'ctrlKey' | 'metaKey' | 'shiftKey'>;

/** CDP's input modifier bits. */
export function modifierBits(e: Modifiers): number {
  return (e.altKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.metaKey ? 4 : 0) | (e.shiftKey ? 8 : 0);
}

/** The button CDP should report for a move with DOM `buttons` held. */
export function heldButton(buttons: number): 'left' | 'middle' | 'right' | 'none' {
  return buttons & 1 ? 'left' : buttons & 2 ? 'right' : buttons & 4 ? 'middle' : 'none';
}

/** A DOM mouse button as CDP names it. */
export function buttonName(button: number): 'left' | 'middle' | 'right' | 'none' {
  return button === 0 ? 'left' : button === 1 ? 'middle' : button === 2 ? 'right' : 'none';
}

/** The text a key types, which CDP needs to insert it. */
function keyText(e: Pick<KeyboardEvent, 'key' | 'ctrlKey' | 'metaKey'>): string | null {
  if (e.ctrlKey || e.metaKey) return null;
  if (e.key === 'Enter') return '\r';
  return [...e.key].length === 1 ? e.key : null;
}

/** A key whose text arrives through `input` once the browser handles it: dead keys, IMEs, phone keyboards, and AltGr. */
export function leftToComposition(
  e: Pick<KeyboardEvent, 'key' | 'isComposing'> & { getModifierState?: KeyboardEvent['getModifierState'] },
): boolean {
  if (e.isComposing || e.key === 'Process' || e.key === 'Unidentified' || e.key === 'Dead') return true;
  // AltGr reports Ctrl+Alt on Windows, but types a character.
  return !!e.getModifierState?.('AltGraph') && [...e.key].length === 1;
}

/** A chord the tab's browser turns into a `paste` event, which has the client's clipboard. */
export function isPasteChord(
  e: Pick<KeyboardEvent, 'key' | 'altKey' | 'ctrlKey' | 'metaKey' | 'shiftKey'>,
  isMac: boolean,
): boolean {
  if (e.altKey) return false;
  if (e.shiftKey && !e.ctrlKey && !e.metaKey && e.key === 'Insert') return true;
  return e.key.toLowerCase() === 'v' && (isMac ? e.metaKey : e.ctrlKey);
}

/** The Windows virtual key code, which Chromium's editing needs, for a key from its name. */
function keyCodeFor(key: string): number {
  return usKey(key)?.keyCode ?? 0;
}

interface PhysicalKey {
  code: string;
  keyCode: number;
  shift: boolean;
}

/** The physical key that types `key` on a US layout, its Windows virtual key code, and whether it takes Shift. */
function usKey(key: string): PhysicalKey | null {
  if (/^[a-z]$/i.test(key)) {
    const upper = key.toUpperCase();
    return { code: `Key${upper}`, keyCode: upper.charCodeAt(0), shift: key === upper };
  }
  if (/^\d$/.test(key)) return { code: `Digit${key}`, keyCode: key.charCodeAt(0), shift: false };
  const fkey = key.match(/^F([1-9]|1[0-2])$/);
  if (fkey) return { code: key, keyCode: 111 + Number(fkey[1]), shift: false };
  if (key in NAMED_KEY_CODES) return { code: key === ' ' ? 'Space' : key, keyCode: NAMED_KEY_CODES[key], shift: false };
  const index = US_SYMBOLS.findIndex(([plain, shifted]) => key === plain || key === shifted);
  if (index < 0) return null;
  const [plain, , code, keyCode] = US_SYMBOLS[index];
  return { code, keyCode, shift: key !== plain };
}

const NAMED_KEY_CODES: Record<string, number> = {
  ' ': 32,
  Enter: 13,
  Tab: 9,
  Escape: 27,
  Backspace: 8,
  Delete: 46,
  Insert: 45,
  Home: 36,
  End: 35,
  PageUp: 33,
  PageDown: 34,
  ArrowUp: 38,
  ArrowDown: 40,
  ArrowLeft: 37,
  ArrowRight: 39,
};

/** US layout symbols: unshifted, shifted, `code`, and key code. */
const US_SYMBOLS: [string, string, string, number][] = [
  ['`', '~', 'Backquote', 192],
  ['1', '!', 'Digit1', 49],
  ['2', '@', 'Digit2', 50],
  ['3', '#', 'Digit3', 51],
  ['4', '$', 'Digit4', 52],
  ['5', '%', 'Digit5', 53],
  ['6', '^', 'Digit6', 54],
  ['7', '&', 'Digit7', 55],
  ['8', '*', 'Digit8', 56],
  ['9', '(', 'Digit9', 57],
  ['0', ')', 'Digit0', 48],
  ['-', '_', 'Minus', 189],
  ['=', '+', 'Equal', 187],
  ['[', '{', 'BracketLeft', 219],
  [']', '}', 'BracketRight', 221],
  ['\\', '|', 'Backslash', 220],
  [';', ':', 'Semicolon', 186],
  ["'", '"', 'Quote', 222],
  [',', '<', 'Comma', 188],
  ['.', '>', 'Period', 190],
  ['/', '?', 'Slash', 191],
];

/** Pressing and releasing the prefix key (`prefix prefix`), as a US keyboard would. */
export function prefixKeyMessages(p: ParsedKey): BrowserClientMessage[] {
  const physical = usKey(p.key);
  const event = {
    key: p.key,
    code: physical?.code ?? '',
    keyCode: physical?.keyCode ?? 0,
    repeat: false,
    altKey: p.alt,
    ctrlKey: p.ctrl,
    metaKey: false,
    // Ctrl+letter is the same either case.
    shiftKey: !!physical?.shift && !(p.ctrl && /^[a-z]$/i.test(p.key)),
  };
  return [keyMessage(event, true), keyMessage(event, false)];
}

export function keyMessage(
  e: Pick<KeyboardEvent, 'key' | 'code' | 'keyCode' | 'repeat'> & Modifiers,
  down: boolean,
): BrowserClientMessage {
  return {
    type: 'key',
    down,
    key: e.key,
    code: e.code,
    // Synthetic keys, as from the mobile key bar, have none.
    key_code: e.keyCode || keyCodeFor(e.key),
    text: down ? keyText(e) : null,
    modifiers: modifierBits(e),
    repeat: e.repeat,
  };
}

/** Where a frame sits in a canvas box under `object-fit: contain`. */
export function containRect(boxWidth: number, boxHeight: number, frameWidth: number, frameHeight: number) {
  if (frameWidth <= 0 || frameHeight <= 0) return { left: 0, top: 0, width: boxWidth, height: boxHeight };
  const scale = Math.min(boxWidth / frameWidth, boxHeight / frameHeight);
  const width = frameWidth * scale;
  const height = frameHeight * scale;
  return { left: (boxWidth - width) / 2, top: (boxHeight - height) / 2, width, height };
}

/** What an address bar makes of typed text: a URL, or else a search. */
export function addressToUrl(input: string, searchUrl = DEFAULT_SEARCH_URL): string {
  const text = input.trim();
  const search = () => searchUrl.split('%s').join(encodeURIComponent(text));
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(text) || /^(about|data|file|chrome):/i.test(text)) return text;
  if (/\s/.test(text)) return search();
  const host = text.split(/[/?#]/)[0];
  if (/^(localhost|\[[0-9a-f:]+\]|\d+\.\d+\.\d+\.\d+)(:\d+)?$/i.test(host) || /^[^.]+\.local(:\d+)?$/i.test(host)) {
    return `http://${text}`;
  }
  if (/:\d+$/.test(host)) return `http://${text}`;
  return host.includes('.') ? `https://${text}` : search();
}
