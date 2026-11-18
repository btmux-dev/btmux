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
  // Option chords on macOS: by their physical key when Option doesn't type
  // ASCII text here, including dead keys (Option-E, Option-N on US).
  const option = mac && e.altKey && !e.ctrlKey && !e.metaKey;
  const dead = option && e.key === 'Dead';
  if (MODIFIER_KEYS.has(e.key) && !dead) return null;
  let key = SPECIAL[e.key] ?? e.key;
  let alt = e.altKey;
  if (option && (key.length === 1 || dead)) {
    const text = macOptionText(e);
    const chord = text === null ? optionChordKey(e) : null;
    if (chord !== null) key = e.shiftKey ? chord.toUpperCase() : chord;
    else if (dead) return null;
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

export type Output = { kind: 'run'; entry: KeyEntry } | { kind: 'keys'; keys: string[] };

/**
 * Keys as Vim maps them: the longest mapping wins. When no mapping goes on,
 * the longest one among the keys runs and the rest are mapped again; from a
 * key no mapping starts with, keys go through as typed. `held` waits for more
 * keys, and is a mapping of its own too if `ambiguous`, run after the timeout.
 */
export function consume(
  table: KeyTable,
  keys: readonly string[],
): { out: Output[]; held: string[]; ambiguous: boolean } {
  const out: Output[] = [];
  let rest = [...keys];
  while (rest.length) {
    const { entry, prefix } = table.lookup(rest);
    if (prefix) return { out, held: rest, ambiguous: entry !== null };
    if (entry) {
      out.push({ kind: 'run', entry });
      break;
    }
    let n = rest.length - 1;
    while (n > 0 && !table.lookup(rest.slice(0, n)).entry) n--;
    if (!n) {
      out.push({ kind: 'keys', keys: rest });
      break;
    }
    out.push({ kind: 'run', entry: table.lookup(rest.slice(0, n)).entry! });
    rest = rest.slice(n);
  }
  return { out, held: [], ambiguous: false };
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
