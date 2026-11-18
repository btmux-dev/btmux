// Where the cursor jumped from, across files, for going back and forward
// (vim's CTRL-O and CTRL-I, VS Code's Go Back and Go Forward).
import type { ChangeDesc } from '@codemirror/state';

export interface Jump {
  path: string;
  /** Offset in the file; clamped when it has shrunk since. */
  pos: number;
  line: number;
}

const MAX = 100;

const same = (a: Jump | undefined, b: Jump) => !!a && a.path === b.path && a.line === b.line;

export class JumpList {
  private entries: Jump[] = [];
  /** Where the cursor is in the list: `entries.length` unless back went back. */
  private at = 0;

  /** A jump away from `from`; drops what forward would go to, as VS Code does. */
  record(from: Jump) {
    this.entries.length = this.at;
    if (!same(this.entries[this.entries.length - 1], from)) this.entries.push(from);
    if (this.entries.length > MAX) this.entries.shift();
    this.at = this.entries.length;
  }

  /** Where back goes from `here`, or null. */
  back(here: Jump): Jump | null {
    if (this.at === this.entries.length) {
      // Keep where back left, for forward.
      if (same(this.entries[this.at - 1], here)) this.at--;
      else this.entries.push(here);
    } else this.entries[this.at] = here;
    if (this.at === 0) return null;
    return this.entries[--this.at];
  }

  forward(here: Jump): Jump | null {
    if (this.at >= this.entries.length - 1) return null;
    this.entries[this.at] = here;
    return this.entries[++this.at];
  }

  /** Follow edits to `path`. */
  map(path: string, changes: ChangeDesc) {
    for (const entry of this.entries) if (entry.path === path) entry.pos = changes.mapPos(entry.pos);
  }

  get size() {
    return this.entries.length;
  }
}
