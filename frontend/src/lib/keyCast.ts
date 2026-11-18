// The key overlay's model: pressed keys as entries, with what btmux made of them.
import type { KeyOverlayMode } from '../generated/protocol';
import { macOptionText } from './macOption';
import { IS_MAC } from './nvimKeys';
import { terminalInput, type InputSocket } from './terminalInput';

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
  /** Input to a terminal at a password prompt, or one btmux can't tell about. */
  | { id: number; at: number; kind: 'hidden'; reason: HiddenReason };

export type HiddenReason = 'password' | 'unknown';

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
}

/** How long an entry stays after its last key. */
export const ENTRY_MS = 2500;
/** Typing after this pause starts a new run. */
const RUN_GAP_MS = 1500;
const MAX_ENTRIES = 4;
const MAX_RUN = 32;
/** Terminal input waits this long for the server to say whether it was hidden. */
export const HOLD_MS = 2000;

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

/** The terminals keys go to: their `/ws/pane` sockets (`terminalInput`). */
export interface TerminalInputs {
  socket(id: string): InputSocket | null;
  subscribe(listener: () => void): () => void;
}

interface Pending {
  source: object | null;
  key: KeyInfo | null;
  text: string | null;
  /** The pane or popup the input goes to. */
  terminal: string | null;
  socket: InputSocket | null;
  /** The socket's input frames before this input. */
  before: number;
  note: KeyNote | null;
  synthetic: boolean;
  at: number;
}

interface Held extends Pending {
  /** The socket's input frames after this input. */
  sent: number;
  deadline: number;
}

/** A synthetic key this soon after a real one is its copy. */
const REPLACE_MS = 50;

export class KeyCaster {
  private entries: Entry[] = [];
  private pending: Pending | null = null;
  /** Committed in order, once each terminal input is known to be shown or hidden. */
  private held: Held[] = [];
  private timer: ReturnType<typeof setTimeout> | undefined;
  private nextId = 0;
  private listeners = new Set<() => void>();
  mode: KeyOverlayMode = 'all';

  constructor(
    private readonly mac: boolean,
    private readonly inputs: TerminalInputs,
    private readonly now: () => number = () => performance.now(),
  ) {
    inputs.subscribe(() => this.drain());
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** Entries still shown, oldest first. */
  snapshot(): readonly Entry[] {
    return this.entries;
  }

  /**
   * A keydown to `terminal` (a pane or popup id, or null), committed by
   * `flush` once `note` had its say. A synthetic key right after a real one
   * replaces it, as the mobile key bar's copies do.
   */
  key(source: object, info: KeyInfo, terminal: string | null, synthetic = false) {
    const last = this.pending;
    if (last?.key && synthetic && !last.synthetic && this.now() - last.at < REPLACE_MS) {
      this.pending = { ...this.input(terminal), source, key: info, note: last.note, synthetic };
      return;
    }
    this.flush();
    this.pending = { ...this.input(terminal), source, key: info, synthetic };
  }

  /** Text typed without a keydown (an IME composition, dictation), committed by `flush`. */
  text(text: string, terminal: string | null) {
    this.flush();
    if (text) this.pending = { ...this.input(terminal), text };
  }

  note(source: object, note: KeyNote) {
    if (this.pending && this.pending.source === source) this.pending.note = note;
  }

  /** The prefix timed out without a key. */
  cancelPrefix() {
    this.flush();
    const last = this.last();
    if (last?.kind !== 'binding' || !last.pending) return;
    last.pending = false;
    last.cancelled = true;
    this.touch(last);
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
    clearTimeout(this.timer);
    this.pending = null;
    this.held = [];
    this.entries = [];
    this.changed();
  }

  flush() {
    const pending = this.pending;
    if (!pending) return;
    this.pending = null;
    this.held.push({ ...pending, sent: pending.socket?.sent ?? 0, deadline: this.now() + HOLD_MS });
    this.drain();
  }

  /** Commit held input whose terminal said whether it was hidden, or that waited too long. */
  drain() {
    clearTimeout(this.timer);
    let changed = false;
    while (this.held.length > 0) {
      const verdict = this.verdict(this.held[0]);
      if (verdict === undefined) break;
      changed = this.commit(this.held.shift()!, verdict) || changed;
    }
    if (this.held.length > 0) {
      this.timer = setTimeout(() => this.drain(), Math.max(0, this.held[0].deadline - this.now()) + 1);
    }
    if (changed) this.changed();
  }

  private input(terminal: string | null): Pending {
    const socket = terminal === null ? null : this.inputs.socket(terminal);
    return {
      source: null,
      key: null,
      text: null,
      terminal,
      socket,
      before: socket?.sent ?? 0,
      note: null,
      synthetic: false,
      at: this.now(),
    };
  }

  /** Whether held input is hidden; undefined while its terminal hasn't said. */
  private verdict(held: Held): HiddenReason | false | undefined {
    // Keys btmux took never reach the terminal.
    if (held.terminal === null || held.note) return false;
    const socket = held.socket;
    if (!socket || socket.closed) return 'unknown';
    const late = this.now() >= held.deadline;
    // Reports covering this input's frames, or the mode now if it sent none.
    const covering =
      held.sent > held.before
        ? socket.reports.filter((report) => report.inputs > held.before)
        : socket.reports.slice(-1);
    const done = held.sent > held.before ? covering.some((report) => report.inputs >= held.sent) : covering.length > 0;
    if (!done) return late ? 'unknown' : undefined;
    if (covering.some((report) => report.hidden === true)) return 'password';
    if (covering.some((report) => report.hidden === null)) return 'unknown';
    return false;
  }

  private commit(input: Held, hidden: HiddenReason | false): boolean {
    if (input.text !== null) {
      if (hidden) this.addHidden(hidden);
      else if (this.mode === 'all') for (const char of input.text) this.addText(char, false);
      else return false;
      return true;
    }
    const info = input.key;
    if (!info || info.isComposing || IGNORED.has(info.key)) return false;
    const note = input.note;
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
    if (hidden) {
      this.addHidden(hidden);
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

  private addHidden(reason: HiddenReason) {
    const last = this.last();
    if (last?.kind === 'hidden') {
      // A password prompt says more than not knowing.
      if (reason === 'password') last.reason = reason;
      this.touch(last);
    } else {
      this.push({ kind: 'hidden', reason });
    }
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
export const keyCaster = new KeyCaster(IS_MAC, terminalInput);
