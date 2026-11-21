// VS Code's workbench and terminal keys, without the prefix. Cmd ones are
// macOS only: elsewhere plain Ctrl chords are the shell's.
import { IS_MAC } from './nvimKeys';

/** Not a keybinding action: the palette's "settings" command. */
export const OPEN_SETTINGS = 'open-settings';

export interface WorkbenchKey {
  /** A keybinding action, or `OPEN_SETTINGS`. */
  action: string;
  /** In CodeMirror's notation, for `cmKeyCap`. */
  key: string;
  /** Only while a pane's terminal has focus, as in VS Code. */
  terminal?: boolean;
}

const MAC_KEYS: WorkbenchKey[] = [
  { action: 'command-palette', key: 'Cmd-Shift-p' },
  { action: 'editor-quick-open', key: 'Cmd-p' },
  { action: 'file-browser', key: 'Cmd-Shift-e' },
  { action: 'git-view', key: 'Ctrl-Shift-g' },
  { action: 'new-window', key: 'Ctrl-Shift-`' },
  { action: OPEN_SETTINGS, key: 'Cmd-,' },
  { action: 'search-pane', key: 'Cmd-f', terminal: true },
  { action: 'split-vertical', key: 'Cmd-\\', terminal: true },
];

const OTHER_KEYS: WorkbenchKey[] = [
  { action: 'command-palette', key: 'Ctrl-Shift-p' },
  { action: 'file-browser', key: 'Ctrl-Shift-e' },
  { action: 'git-view', key: 'Ctrl-Shift-g' },
  { action: 'new-window', key: 'Ctrl-Shift-`' },
  { action: OPEN_SETTINGS, key: 'Ctrl-,' },
  { action: 'split-vertical', key: 'Ctrl-Shift-5', terminal: true },
];

export function workbenchKeys(mac = IS_MAC): WorkbenchKey[] {
  return mac ? MAC_KEYS : OTHER_KEYS;
}

/** Keys matched by position (`code`), since Shift and layouts change their `key`. */
const CODES: Record<string, string> = { '`': 'Backquote', '\\': 'Backslash', ',': 'Comma', '5': 'Digit5' };

type KeyInit = Pick<KeyboardEvent, 'key' | 'code' | 'metaKey' | 'ctrlKey' | 'altKey' | 'shiftKey'>;

function matches(spec: string, e: KeyInit): boolean {
  const parts = spec.split('-');
  const key = parts.pop()!;
  const mods = new Set(parts);
  if (e.metaKey !== mods.has('Cmd') || e.ctrlKey !== mods.has('Ctrl') || e.shiftKey !== mods.has('Shift') || e.altKey)
    return false;
  const code = CODES[key];
  return code ? e.code === code : e.key.toLowerCase() === key;
}

/** The key `e` presses, if any. */
export function workbenchKey(e: KeyInit, mac = IS_MAC): WorkbenchKey | undefined {
  return workbenchKeys(mac).find((key) => matches(key.key, e));
}

/** Its keys, for key help. */
export function workbenchKeysFor(action: string, mac = IS_MAC): string[] {
  return workbenchKeys(mac)
    .filter((key) => key.action === action)
    .map((key) => key.key);
}
