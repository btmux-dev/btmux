// The key overlay's model: pressed keys as entries, with what btmux made of them.
import type { KeyOverlayMode } from '../generated/protocol';
import { macOptionText } from './macOption';
import { IS_MAC } from './nvimKeys';

/** A key as shown: modifier names and the key's name. */
export interface Cap {
  mods: string[];
  key: string;
}

export interface Token {
  text: string;
  /** A named key (⏎, ⌫) rather than typed text. */
  named: boolean;
  count: number;
}

export type Entry =
  | { id: number; at: number; kind: 'text'; tokens: Token[]; truncated: boolean }
  | { id: number; at: number; kind: 'chord'; cap: Cap; count: number }
  | {
      id: number;
      at: number;
      kind: 'binding';
      /** The prefix chord, or null for a repeat without it. */
      prefix: Cap | null;
      keys: Cap[];
      /** What the keys did; null if nothing is bound to them. */
      label: string | null;
      pending: boolean;
      cancelled: boolean;
      count: number;
    }
  | { id: number; at: number; kind: 'hidden' };

/** What `useKeybindings` made of a key. */
export type KeyNote =
  | { kind: 'prefix' }
  /** The key after the prefix; `label` is null when nothing is bound to it. */
  | { kind: 'binding'; label: string | null }
  /** A repeatable binding run again without the prefix. */
  | { kind: 'repeat'; label: string };

export interface KeyInfo {
  key: string;
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
  /** AltGr, which types text with Ctrl and Alt held on Windows. */
  altGraph: boolean;
  isComposing: boolean;
  /** Typed into a program reading a password. */
  hidden: boolean;
}

/** How long an entry stays after its last key. */
export const ENTRY_MS = 2500;
/** Typing after this pause starts a new run. */
const RUN_GAP_MS = 1500;
const MAX_ENTRIES = 4;
const MAX_RUN = 32;

const IGNORED = new Set([
  'Shift',
  'Control',
  'Alt',
  'Meta',
  'AltGraph',
  'CapsLock',
  'NumLock',
  'ScrollLock',
  'Fn',
  'FnLock',
  'OS',
  'Super',
  'Hyper',
  'Dead',
  'Process',
  'Unidentified',
  'Compose',
]);

const NAMED: Record<string, string> = {
  Enter: '⏎',
  Tab: '⇥',
  Backspace: '⌫',
  Delete: '⌦',
  Escape: 'Esc',
  ArrowUp: '↑',
  ArrowDown: '↓',
  ArrowLeft: '←',
  ArrowRight: '→',
  PageUp: 'PgUp',
  PageDown: 'PgDn',
  Insert: 'Ins',
};

const MAC_MODS = { ctrl: '⌃', alt: '⌥', shift: '⇧', meta: '⌘' };
const PC_MODS = { ctrl: 'Ctrl', alt: 'Alt', shift: 'Shift', meta: 'Super' };

/** A key as typed text: one character, or null for a named key or chord. */
function typedText(e: KeyInfo, mac: boolean): string | null {
  if ([...e.key].length !== 1) return null;
  if (e.altGraph) return e.key;
  if (mac && macOptionText(e) !== null) return e.key;
  return e.ctrlKey || e.altKey || e.metaKey ? null : e.key;
}

const modified = (e: KeyInfo) => e.ctrlKey || e.altKey || e.metaKey;

/** The key's name: as typed alone (`c` and `C` bind apart), upper case in a chord. */
function keyName(e: KeyInfo, mac: boolean): string {
  if (e.key === ' ') return 'Space';
  const named = NAMED[e.key];
  if (named) return named;
  if ([...e.key].length !== 1 || !modified(e)) return e.key;
  // macOS Option composes characters (Option+x is "≈").
  const letter = /^Key([A-Z])$/.exec(e.code)?.[1];
  if (letter && e.altKey && (mac || !/^[a-z]$/i.test(e.key))) return letter;
  return e.key.toUpperCase();
}

/** The key as a cap; Shift shows only where the key doesn't imply it. */
export function capOf(e: KeyInfo, mac: boolean): Cap {
  const names = mac ? MAC_MODS : PC_MODS;
  const key = keyName(e, mac);
  const letter = /^[A-Z]$/.test(key) && modified(e);
  const shifted = e.shiftKey && (letter || e.key in NAMED || [...e.key].length > 1);
  const mods = [e.ctrlKey && names.ctrl, e.altKey && names.alt, shifted && names.shift, e.metaKey && names.meta].filter(
    (mod): mod is string => !!mod,
  );
  return { mods, key };
}

function sameCap(a: Cap, b: Cap): boolean {
  return a.key === b.key && a.mods.join() === b.mods.join();
}

interface Pending {
  source: object;
  info: KeyInfo;
  note: KeyNote | null;
  synthetic: boolean;
  at: number;
}

/** A synthetic key this soon after a real one is its copy. */
const REPLACE_MS = 50;

export class KeyCaster {
  private entries: Entry[] = [];
  private pending: Pending | null = null;
  private nextId = 0;
  private listeners = new Set<() => void>();
  mode: KeyOverlayMode = 'all';

  constructor(
    private readonly mac: boolean,
    private readonly now: () => number = () => performance.now(),
  ) {}

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** Entries still shown, oldest first. */
  snapshot(): readonly Entry[] {
    return this.entries;
  }

  /**
   * A keydown, committed by `flush` once `note` had its say. A synthetic key
   * right after a real one replaces it, as the mobile key bar's copies do.
   */
  key(source: object, info: KeyInfo, synthetic = false) {
    const now = this.now();
    const last = this.pending;
    if (last && synthetic && !last.synthetic && now - last.at < REPLACE_MS) {
      this.pending = { source, info, note: last.note, synthetic, at: now };
      return;
    }
    this.flush();
    this.pending = { source, info, note: null, synthetic, at: now };
  }

  note(source: object, note: KeyNote) {
    if (this.pending?.source === source) this.pending.note = note;
  }

  /** The prefix timed out without a key. */
  cancelPrefix() {
    this.flush();
    const last = this.last();
    if (last?.kind !== 'binding' || !last.pending) return;
    last.pending = false;
    last.cancelled = true;
    this.touch(last);
  }

  /** Committed text without key events, such as an IME composition. */
  text(text: string, hidden: boolean) {
    this.flush();
    if (!text) return;
    if (hidden) this.addHidden();
    else if (this.mode === 'all') for (const char of text) this.addText(char, false);
    this.changed();
  }

  /** Drop entries shown long enough; true if any went. */
  expire(): boolean {
    const now = this.now();
    const kept = this.entries.filter(
      (entry) => (entry.kind === 'binding' && entry.pending) || now - entry.at < ENTRY_MS,
    );
    if (kept.length === this.entries.length) return false;
    this.entries = kept;
    this.changed();
    return true;
  }

  clear() {
    this.pending = null;
    this.entries = [];
    this.changed();
  }

  flush() {
    const pending = this.pending;
    if (!pending) return;
    this.pending = null;
    if (this.commit(pending)) this.changed();
  }

  private commit({ info, note }: Pending): boolean {
    if (info.isComposing || IGNORED.has(info.key)) return false;
    const cap = capOf(info, this.mac);
    const last = this.last();
    if (note?.kind === 'prefix') {
      this.push({ kind: 'binding', prefix: cap, keys: [], label: null, pending: true, cancelled: false, count: 1 });
      return true;
    }
    if (note?.kind === 'binding') {
      if (last?.kind === 'binding' && last.pending) {
        last.keys.push(cap);
        last.label = note.label;
        last.pending = false;
        this.touch(last);
      } else {
        this.push({
          kind: 'binding',
          prefix: null,
          keys: [cap],
          label: note.label,
          pending: false,
          cancelled: false,
          count: 1,
        });
      }
      return true;
    }
    if (note?.kind === 'repeat') {
      if (last?.kind === 'binding' && !last.pending && last.label === note.label) {
        last.count += 1;
        this.touch(last);
      } else {
        this.push({
          kind: 'binding',
          prefix: null,
          keys: [cap],
          label: note.label,
          pending: false,
          cancelled: false,
          count: 1,
        });
      }
      return true;
    }
    if (info.hidden) {
      this.addHidden();
      return true;
    }
    const text = typedText(info, this.mac);
    const chord = text === null && (info.ctrlKey || info.altKey || info.metaKey || /^F\d+$/.test(info.key));
    if (chord) {
      if (last?.kind === 'chord' && sameCap(last.cap, cap) && this.now() - last.at < RUN_GAP_MS) {
        last.count += 1;
        this.touch(last);
      } else {
        this.push({ kind: 'chord', cap, count: 1 });
      }
      return true;
    }
    if (this.mode === 'chords') return false;
    if (text !== null) this.addText(text === ' ' ? '␣' : text, false);
    else this.addText([...cap.mods, cap.key].join(this.mac ? '' : '+'), true);
    return true;
  }

  private addText(text: string, named: boolean) {
    let last = this.last();
    if (last?.kind !== 'text' || this.now() - last.at >= RUN_GAP_MS) {
      this.push({ kind: 'text', tokens: [], truncated: false });
      last = this.last()!;
    }
    if (last.kind !== 'text') return;
    const tail = last.tokens[last.tokens.length - 1];
    if (named && tail?.named && tail.text === text) tail.count += 1;
    else last.tokens.push({ text, named, count: 1 });
    if (last.tokens.length > MAX_RUN) {
      last.tokens.splice(0, last.tokens.length - MAX_RUN);
      last.truncated = true;
    }
    this.touch(last);
  }

  private addHidden() {
    const last = this.last();
    if (last?.kind === 'hidden') this.touch(last);
    else this.push({ kind: 'hidden' });
  }

  private last(): Entry | undefined {
    return this.entries[this.entries.length - 1];
  }

  private push(entry: DistributiveOmit<Entry, 'id' | 'at'>) {
    const full = { ...entry, id: this.nextId++, at: this.now() } as Entry;
    this.entries = [...this.entries, full].slice(-MAX_ENTRIES);
  }

  /** Mark an entry updated, as a new object so views re-render it. */
  private touch(entry: Entry) {
    const updated = { ...entry, at: this.now() } as Entry;
    this.entries = this.entries.map((e) => (e === entry ? updated : e));
  }

  private changed() {
    for (const listener of this.listeners) listener();
  }
}

type DistributiveOmit<T, K extends keyof T> = T extends unknown ? Omit<T, K> : never;

/** The client's key overlay. */
export const keyCaster = new KeyCaster(IS_MAC);
