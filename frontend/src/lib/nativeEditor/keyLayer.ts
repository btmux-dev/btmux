// Key sequences the native editor runs ahead of vim: `[native-editor.vim-keys]`
// and its defaults, and VS Code's `Mod-k` chords. Keys are codemirror-vim's
// names (`<C-w>`, `<A-x>`, `<M-k>` for Cmd, `<S-Tab>`, `<Space>`, `x`), as
// the server parses them from the config.
import { macOptionText, optionChordKey } from '../macOption';
import type { VimBind, VimGroup, VimMode } from '../../generated/protocol';

/** A vim mode, or `plain` without vim. */
export type KeyMode = VimMode | 'plain';

export interface KeyEntry {
  keys: string[];
  action: string | null;
  /** Keys vim gets instead of an action. */
  rhs: string[] | null;
  desc: string | null;
}

type KeyInit = Pick<KeyboardEvent, 'key' | 'code' | 'ctrlKey' | 'altKey' | 'shiftKey' | 'metaKey'>;

const SPECIAL: Record<string, string> = {
  Enter: 'CR',
  Backspace: 'BS',
  Delete: 'Del',
  Escape: 'Esc',
  Insert: 'Ins',
  ArrowLeft: 'Left',
  ArrowRight: 'Right',
  ArrowUp: 'Up',
  ArrowDown: 'Down',
  ' ': 'Space',
};

const MODIFIER_KEYS = new Set(['Shift', 'Alt', 'Control', 'Meta', 'CapsLock', 'AltGraph', 'Dead', 'Unidentified']);

/** The key's name, as codemirror-vim names it; null for modifiers alone. */
export function eventToken(e: KeyInit, mac: boolean): string | null {
  if (MODIFIER_KEYS.has(e.key)) return null;
  let key = SPECIAL[e.key] ?? e.key;
  let alt = e.altKey;
  // Option types text on macOS: a chord only when it isn't ASCII text here.
  if (mac && alt && !e.ctrlKey && !e.metaKey && key.length === 1) {
    const text = macOptionText(e);
    const chord = text === null ? optionChordKey(e) : null;
    if (chord !== null) key = e.shiftKey ? chord.toUpperCase() : chord;
    else {
      if (text !== null) key = text;
      alt = false;
    }
  }
  const letter = key.length === 1 && /[a-z]/i.test(key);
  let mods = '';
  if (e.ctrlKey) mods += 'C-';
  if (alt) mods += 'A-';
  if (e.metaKey) mods += 'M-';
  if ((mods || key.length > 1) && e.shiftKey) mods += 'S-';
  if (letter && mods) key = e.shiftKey ? key.toUpperCase() : key.toLowerCase();
  const name = mods + key;
  return name.length > 1 ? `<${name}>` : name;
}

const join = (keys: readonly string[]) => keys.join('\u0000');

/** Mappings in one mode. */
export class KeyTable {
  private full = new Map<string, KeyEntry>();
  /** Proper prefixes of mappings, with the mappings under each. */
  private prefixes = new Map<string, number>();

  constructor(entries: KeyEntry[]) {
    for (const entry of entries) {
      this.full.set(join(entry.keys), entry);
      for (let i = 1; i < entry.keys.length; i++) {
        const prefix = join(entry.keys.slice(0, i));
        this.prefixes.set(prefix, (this.prefixes.get(prefix) ?? 0) + 1);
      }
    }
  }

  get size() {
    return this.full.size;
  }

  lookup(keys: readonly string[]): { entry: KeyEntry | null; prefix: boolean } {
    const key = join(keys);
    return { entry: this.full.get(key) ?? null, prefix: this.prefixes.has(key) };
  }

  /** What can follow `keys`: a mapping, a prefix of more, or both. */
  next(keys: readonly string[]): { key: string; entry: KeyEntry | null; more: number }[] {
    const seen = new Map<string, { key: string; entry: KeyEntry | null; more: number }>();
    for (const entry of this.full.values()) {
      if (entry.keys.length <= keys.length || !keys.every((k, i) => entry.keys[i] === k)) continue;
      const key = entry.keys[keys.length];
      const row = seen.get(key) ?? { key, entry: null, more: 0 };
      if (entry.keys.length === keys.length + 1) row.entry = entry;
      else row.more++;
      seen.set(key, row);
    }
    return [...seen.values()];
  }
}

/** Tables per mode, from the config's vim mappings. */
export function keyTables(binds: readonly VimBind[]): Map<VimMode, KeyTable> {
  const byMode = new Map<VimMode, KeyEntry[]>();
  for (const b of binds) {
    const list = byMode.get(b.mode) ?? [];
    list.push({ keys: b.keys, action: b.action, rhs: b.rhs, desc: b.desc });
    byMode.set(b.mode, list);
  }
  return new Map([...byMode].map(([mode, entries]) => [mode, new KeyTable(entries)]));
}

export const groupNames = (groups: readonly VimGroup[]) => new Map(groups.map((g) => [join(g.keys), g.name]));
export const groupName = (names: Map<string, string>, keys: readonly string[]) => names.get(join(keys)) ?? null;

export type Step =
  /** Not a mapping's start: the key does what it does. */
  | { kind: 'pass' }
  /** Held for more keys; a mapping of its own too if `ambiguous`, run after the timeout. */
  | { kind: 'wait'; keys: string[]; ambiguous: boolean }
  | { kind: 'run'; entry: KeyEntry }
  /** Held keys that turned out not to be a mapping: give them to vim. */
  | { kind: 'replay'; keys: string[] };

/** What `key` does after the held keys `held`, as in Vim: the longest mapping wins. */
export function step(table: KeyTable, held: readonly string[], key: string): Step {
  const keys = [...held, key];
  const { entry, prefix } = table.lookup(keys);
  if (prefix) return { kind: 'wait', keys, ambiguous: entry !== null };
  if (entry) return { kind: 'run', entry };
  return held.length ? { kind: 'replay', keys } : { kind: 'pass' };
}

/** A key name for `KeyCap`: `ctrl+w`, `shift+tab`, `space`, `G`. */
export function keyCapKeys(token: string): string {
  const m = /^<((?:[CAMS]-)*)(.+)>$/.exec(token);
  if (!m) return token;
  const mods = m[1]
    .split('-')
    .filter(Boolean)
    .map((mod) => ({ C: 'ctrl', A: 'alt', M: 'mod', S: 'shift' })[mod]!);
  const names: Record<string, string> = {
    CR: 'enter',
    BS: 'backspace',
    Esc: 'esc',
    Del: 'delete',
    Space: 'space',
    Tab: 'tab',
    Left: 'left',
    Right: 'right',
    Up: 'up',
    Down: 'down',
    PageUp: 'pageup',
    PageDown: 'pagedown',
  };
  return [...mods, names[m[2]] ?? m[2]].join('+');
}

/** Keys as typed, for the status line: `<Space>hs` is `␣hs`. */
export function showKeys(keys: readonly string[]): string {
  return keys.map((k) => (k === '<Space>' ? '␣' : k)).join('');
}
