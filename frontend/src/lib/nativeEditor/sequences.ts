// Runs the editor's key sequences ahead of vim, as Neovim runs mappings ahead
// of its built-in keys, and keeps what the which-key popup and the status
// line show of pending keys: held prefixes, and vim's own unfinished commands.
import type { VimBind, VimGroup, VimMode } from '../../generated/protocol';
import { IS_MAC } from '../nvimKeys';
import {
  consume,
  eventToken,
  groupNames,
  KeyTable,
  keyTables,
  showKeys,
  type KeyEntry,
  type KeyMode,
} from './keyLayer';
import { vimPresets, whichKeyRows, type WhichKeyRow } from './whichKey';

/** How long keys wait before the which-key popup shows what can follow. */
export const WHICH_KEY_DELAY_MS = 300;

const MOD = IS_MAC ? 'M' : 'C';
const chord = (...keys: string[]) => keys.map((k) => `<${MOD}-${k}>`);

/** VS Code's `Mod-k` chords, without vim. */
const PLAIN = new KeyTable(
  (
    [
      [chord('k', 'i'), 'editor-hover'],
      [chord('k', 'f'), 'editor-format'],
      [chord('k', '0'), 'editor-fold-all'],
      [chord('k', 'j'), 'editor-unfold-all'],
    ] as const
  ).map(([keys, action]) => ({ keys: [...keys], action, rhs: null, desc: null })),
);
const PLAIN_GROUPS = new Map([[chord('k').join('\u0000'), 'chord']]);

export interface SequenceHost {
  /** Where mappings apply now: null while vim is midway through a command. */
  mode(): KeyMode | null;
  /** Vim's unfinished command, if any. */
  vimPending(): { keys: string[]; operator: boolean } | null;
  registers(): { name: string; text: string }[];
  run(entry: KeyEntry, mode: KeyMode): void;
  /** Keys held for a mapping that turned out not to be one. */
  replay(keys: string[], mode: KeyMode): void;
  /** The popup or the pending keys changed. */
  changed(): void;
}

export interface PendingKeys {
  keys: string[];
  rows: WhichKeyRow[];
}

export class Sequences {
  private tables = new Map<VimMode, KeyTable>();
  private groups = new Map<string, string>();
  private timeout = 1000;
  private whichKey = true;
  /** Keys held for a mapping, and the mode they were typed in. */
  private held: string[] = [];
  private heldMode: KeyMode = 'normal';
  /** Keys vim has of an unfinished command, as typed. */
  private typed: string[] = [];
  private ambiguousTimer: ReturnType<typeof setTimeout> | undefined;
  private popupTimer: ReturnType<typeof setTimeout> | undefined;
  private refreshTimer: ReturnType<typeof setTimeout> | undefined;
  /** Counts keys vim got, so a popup scheduled for older ones doesn't show. */
  private generation = 0;
  /** What the which-key popup shows, once keys have waited. */
  popup: PendingKeys | null = null;
  private shown = { showcmd: '', popup: null as PendingKeys | null };

  constructor(private readonly host: SequenceHost) {}

  configure(binds: readonly VimBind[], groups: readonly VimGroup[], timeout: number, whichKey: boolean) {
    this.tables = keyTables(binds);
    this.groups = groupNames(groups);
    this.timeout = timeout;
    this.whichKey = whichKey;
    this.reset();
  }

  /** Keys pending, as typed, for the status line. */
  get showcmd(): string {
    return showKeys(this.held.length ? this.held : this.typed);
  }

  private table(mode: KeyMode | null): KeyTable | null {
    if (mode === null) return null;
    return mode === 'plain' ? PLAIN : (this.tables.get(mode) ?? null);
  }

  /** Handle a keydown ahead of vim and the keymaps; true if it was taken. */
  keydown(e: KeyboardEvent): boolean {
    const token = eventToken(e, IS_MAC);
    if (token === null) return false;
    const mode = this.held.length ? this.heldMode : this.host.mode();
    const table = this.table(mode);
    if (!table || mode === null) {
      this.noteTyped(token);
      return false;
    }
    const held = this.held;
    const { out, held: next, ambiguous } = consume(table, [...held, token]);
    // Not a mapping's start: the key does what it does.
    if (!held.length && !next.length && out.length === 1 && out[0].kind === 'keys') {
      this.noteTyped(token);
      return false;
    }
    // This key, going through as typed last: its own event does that.
    const last = out[out.length - 1];
    const through = !next.length && last?.kind === 'keys';
    if (through) last.keys = last.keys.slice(0, -1);
    this.clear();
    for (const o of out) {
      if (o.kind === 'run') this.host.run(o.entry, mode);
      else if (o.keys.length) {
        this.host.replay(o.keys, mode);
        for (const key of o.keys) this.noteTyped(key);
      }
    }
    if (next.length) this.hold(next, mode, ambiguous);
    if (!through) return true;
    this.noteTyped(token);
    return false;
  }

  /** A key that went to vim without coming here first (Esc). */
  passed(key: string) {
    if (!this.held.length) this.noteTyped(key);
  }

  /** Esc: drop held keys; true if there were any. */
  cancel(): boolean {
    if (!this.held.length) return false;
    this.reset();
    return true;
  }

  /** Drop everything pending (a file switch, hiding). */
  reset() {
    this.typed = [];
    clearTimeout(this.refreshTimer);
    this.clear();
  }

  private clear() {
    clearTimeout(this.ambiguousTimer);
    clearTimeout(this.popupTimer);
    this.held = [];
    this.popup = null;
    this.notify();
  }

  /** Tell the host only when what it shows changes. */
  private notify() {
    const showcmd = this.showcmd;
    if (showcmd === this.shown.showcmd && this.popup === this.shown.popup) return;
    this.shown = { showcmd, popup: this.popup };
    this.host.changed();
  }

  private hold(keys: string[], mode: KeyMode, ambiguous: boolean) {
    clearTimeout(this.ambiguousTimer);
    clearTimeout(this.popupTimer);
    this.held = keys;
    this.heldMode = mode;
    this.popup = null;
    // A mapping that is also a prefix runs once no more keys come.
    if (ambiguous)
      this.ambiguousTimer = setTimeout(() => {
        const entry = this.table(mode)?.lookup(keys).entry;
        this.clear();
        if (entry) this.host.run(entry, mode);
      }, this.timeout);
    if (this.whichKey)
      this.popupTimer = setTimeout(() => {
        const presets = mode === 'plain' ? [] : vimPresets(keys, false, () => this.host.registers());
        const groups = mode === 'plain' ? PLAIN_GROUPS : this.groups;
        const rows = whichKeyRows(this.table(mode), keys, groups, presets);
        this.popup = rows.length ? { keys, rows } : null;
        this.notify();
      }, WHICH_KEY_DELAY_MS);
    this.notify();
  }

  /** A key vim gets: once it has handled it, see what it still waits for. */
  private noteTyped(key: string) {
    // Without vim, or typing text, nothing is pending of a command.
    const mode = this.host.mode();
    if (!this.typed.length && (mode === 'plain' || mode === 'insert')) return;
    this.typed.push(key);
    this.generation++;
    clearTimeout(this.refreshTimer);
    this.refreshTimer = setTimeout(() => this.refresh(), 0);
  }

  private refresh() {
    // Held keys have their own popup; vim has nothing of them.
    if (this.held.length) return;
    const pending = this.host.vimPending();
    clearTimeout(this.popupTimer);
    this.popup = null;
    if (!pending) this.typed = [];
    this.notify();
    if (!pending || !this.whichKey) return;
    const typed = [...this.typed];
    const generation = this.generation;
    this.popupTimer = setTimeout(() => {
      if (this.generation !== generation || this.held.length) return;
      const rows = vimPresets(typed, pending.operator, () => this.host.registers());
      this.popup = rows.length ? { keys: typed, rows } : null;
      this.notify();
    }, WHICH_KEY_DELAY_MS);
  }
}
