// Git signs, after gitsigns.nvim: lines changed from the Git index marked in
// the gutter, hunk navigation, inline previews, staging and resetting hunks,
// and line blame. The server sends the index text (`git_base`); the diff
// runs here, updated incrementally as the buffer changes.
import { Chunk } from '@codemirror/merge';
import {
  RangeSet,
  RangeSetBuilder,
  StateEffect,
  StateField,
  type EditorState,
  type Extension,
  type Text,
} from '@codemirror/state';
import {
  Decoration,
  EditorView,
  GutterMarker,
  ViewPlugin,
  WidgetType,
  gutter,
  keymap,
  showTooltip,
  type DecorationSet,
  type Tooltip,
  type ViewUpdate,
} from '@codemirror/view';
import type { Blame } from '../../generated/protocol';
import { useStore } from '../../state/store';
import { editorConnection } from './connection';
import { viewDocs } from './doc';

export interface GitBase {
  /** Changes with each new index text; staging names the one it diffed against. */
  generation: number;
  text: Text;
  /** Not in the index: every line is added. */
  untracked: boolean;
}

export const setGitBase = StateEffect.define<GitBase | null>();

/** Big, very different files fall back to a coarser diff rather than stall typing. */
const DIFF = { scanLimit: 2000, timeout: 200 };

type SignKind = 'add' | 'change' | 'changedelete' | 'delete' | 'topdelete';

class Sign extends GutterMarker {
  constructor(readonly kind: SignKind) {
    super();
  }
  eq(other: Sign) {
    return other.kind === this.kind;
  }
  toDOM() {
    const el = document.createElement('div');
    el.className = `cm-git-sign cm-git-${this.kind}`;
    return el;
  }
}

const SIGNS = Object.fromEntries(
  (['add', 'change', 'changedelete', 'delete', 'topdelete'] as const).map((k) => [k, new Sign(k)]),
) as Record<SignKind, Sign>;

/** A chunk's lines in the buffer (B) and the index (A); `null` sides are empty. */
interface Hunk {
  chunk: Chunk;
  /** 1-based, inclusive. */
  lines: { first: number; last: number } | null;
  removed: number;
  /** The line its sign and jumps go to. */
  anchor: number;
  kind: SignKind;
}

function hunkOf(chunk: Chunk, base: Text, doc: Text): Hunk {
  const removed = chunk.toA > chunk.fromA ? base.lineAt(chunk.endA).number - base.lineAt(chunk.fromA).number + 1 : 0;
  if (chunk.toB > chunk.fromB) {
    const lines = { first: doc.lineAt(chunk.fromB).number, last: doc.lineAt(chunk.endB).number };
    const added = lines.last - lines.first + 1;
    const kind = removed === 0 ? 'add' : removed > added ? 'changedelete' : 'change';
    return { chunk, lines, removed, anchor: lines.first, kind };
  }
  // Removed lines sit between two lines: marked under the one above.
  const line = doc.lineAt(chunk.fromB);
  const atStart = chunk.fromB === line.from;
  if (atStart && line.number === 1) return { chunk, lines: null, removed, anchor: 1, kind: 'topdelete' };
  return { chunk, lines: null, removed, anchor: atStart ? line.number - 1 : line.number, kind: 'delete' };
}

interface GitState {
  base: GitBase | null;
  chunks: readonly Chunk[];
}

function compute(base: GitBase | null, doc: Text): GitState {
  return { base, chunks: base ? Chunk.build(base.text, doc, DIFF) : [] };
}

const gitField = StateField.define<GitState>({
  create: () => ({ base: null, chunks: [] }),
  update(value, tr) {
    for (const e of tr.effects) if (e.is(setGitBase)) return compute(e.value, tr.state.doc);
    if (!tr.docChanged || !value.base) return value;
    return {
      base: value.base,
      chunks: Chunk.updateB(value.chunks, value.base.text, tr.state.doc, tr.changes, DIFF),
    };
  },
});

/** The hunks of `state`, and its index text. */
function hunks(state: EditorState): { base: GitBase | null; hunks: Hunk[] } {
  const git = state.field(gitField, false);
  if (!git?.base) return { base: null, hunks: [] };
  const base = git.base.text;
  return { base: git.base, hunks: git.chunks.map((c) => hunkOf(c, base, state.doc)) };
}

const signsField = StateField.define<RangeSet<GutterMarker>>({
  create: (state) => buildSigns(state),
  update: (value, tr) => (tr.docChanged || tr.effects.some((e) => e.is(setGitBase)) ? buildSigns(tr.state) : value),
});

function buildSigns(state: EditorState): RangeSet<GutterMarker> {
  const byLine = new Map<number, SignKind>();
  for (const h of hunks(state).hunks) {
    if (h.lines) {
      for (let n = h.lines.first; n <= h.lines.last; n++) byLine.set(n, h.kind === 'changedelete' ? 'change' : h.kind);
      if (h.kind === 'changedelete') byLine.set(h.lines.last, 'changedelete');
    } else if (!byLine.has(h.anchor)) byLine.set(h.anchor, h.kind);
  }
  const builder = new RangeSetBuilder<GutterMarker>();
  for (const n of [...byLine.keys()].sort((a, b) => a - b))
    builder.add(state.doc.line(n).from, state.doc.line(n).from, SIGNS[byLine.get(n)!]);
  return builder.finish();
}

/** Lines added, changed, and removed, as gitsigns' status counts them; `null` outside Git. */
export function gitSummary(state: EditorState): { added: number; changed: number; removed: number } | null {
  const { base, hunks: list } = hunks(state);
  if (!base) return null;
  const sum = { added: 0, changed: 0, removed: 0 };
  for (const h of list) {
    const added = h.lines ? h.lines.last - h.lines.first + 1 : 0;
    const changed = Math.min(added, h.removed);
    sum.changed += changed;
    sum.added += added - changed;
    sum.removed += h.removed - changed;
  }
  return sum;
}

const covers = (h: Hunk, line: number) => (h.lines ? h.lines.first <= line && line <= h.lines.last : h.anchor === line);

/** The hunks on the selection's lines, or under the cursor. */
function selectedHunks(state: EditorState): Hunk[] {
  const { hunks: list } = hunks(state);
  const sel = state.selection.main;
  const first = state.doc.lineAt(sel.from).number;
  const last = state.doc.lineAt(sel.to).number;
  return list.filter((h) => {
    const top = h.lines?.first ?? h.anchor;
    const bottom = h.lines?.last ?? h.anchor;
    return top <= last && bottom >= first;
  });
}

const info = (message: string) => useStore.getState().showToast(message, 'info');

const inGit = (view: EditorView) => !!view.state.field(gitField, false)?.base;

function needsGit(run: (view: EditorView) => boolean) {
  return (view: EditorView) => {
    if (!inGit(view)) {
      info('This file is not in a Git repository');
      return true;
    }
    return run(view);
  };
}

function jump(view: EditorView, forward: boolean): boolean {
  const list = hunks(view.state).hunks;
  if (!list.length) {
    info('No changes from the Git index');
    return true;
  }
  const line = view.state.doc.lineAt(view.state.selection.main.head).number;
  const anchors = list.map((h) => h.anchor);
  const target = forward
    ? (anchors.find((a) => a > line) ?? anchors[0])
    : ([...anchors].reverse().find((a) => a < line) ?? anchors[anchors.length - 1]);
  const pos = view.state.doc.line(target).from;
  view.dispatch({ selection: { anchor: pos }, effects: EditorView.scrollIntoView(pos, { y: 'nearest', yMargin: 60 }) });
  return true;
}

/** Changes to `target` that give `list`'s hunks the text `source` has: side `a` stages, `b` resets. */
function replaceHunks(
  target: Text,
  source: Text,
  list: Hunk[],
  side: 'a' | 'b',
): { from: number; to: number; insert: string }[] {
  return list.map(({ chunk }) => {
    const [from, to, sFrom, sTo] =
      side === 'b'
        ? [chunk.fromB, chunk.toB, chunk.fromA, chunk.toA]
        : [chunk.fromA, chunk.toA, chunk.fromB, chunk.toB];
    return {
      from,
      to: Math.min(to, target.length),
      insert: source.sliceString(sFrom, Math.min(sTo, source.length)),
    };
  });
}

async function stageText(view: EditorView, text: (base: Text) => string) {
  const doc = viewDocs.get(view);
  const git = view.state.field(gitField, false)?.base;
  if (!doc?.buffer || !git) return;
  const result = await editorConnection.gitStage(doc.buffer, git.generation, text(git.text));
  if (result.error) useStore.getState().showToast('Cannot stage', 'error', { body: result.error });
}

function stageHunks(view: EditorView, list: Hunk[]) {
  if (!list.length) return info('No hunk here');
  const doc = view.state.doc;
  void stageText(view, (base) => applyTo(base, replaceHunks(base, doc, list, 'a')));
}

function resetHunks(view: EditorView, list: Hunk[]) {
  if (!list.length) return info('No hunk here');
  view.dispatch({
    changes: replaceHunks(view.state.doc, hunks(view.state).base!.text, list, 'b'),
    userEvent: 'delete.git-reset',
  });
}

/** Apply non-overlapping, ordered changes to a `Text`. */
function applyTo(text: Text, changes: readonly { from: number; to: number; insert: string }[]): string {
  let out = '';
  let pos = 0;
  for (const c of changes) {
    out += text.sliceString(pos, c.from) + c.insert;
    pos = c.to;
  }
  return out + text.sliceString(pos);
}

// --- Inline previews ---

const togglePreview = StateEffect.define<number | null>();

const hunkAt = (state: EditorState, pos: number) => {
  const line = state.doc.lineAt(Math.min(pos, state.doc.length)).number;
  return hunks(state).hunks.find((h) => covers(h, line));
};

/** The anchor position of the hunk previewed inline, if any; closed once the hunk is gone. */
const previewField = StateField.define<number | null>({
  create: () => null,
  update(value, tr) {
    let pos = value === null ? null : tr.changes.mapPos(value);
    for (const e of tr.effects) if (e.is(togglePreview)) pos = e.value;
    return pos !== null && hunkAt(tr.state, pos) ? pos : null;
  },
});

function previewedHunk(state: EditorState): { hunk: Hunk; index: number; count: number; base: Text } | null {
  const pos = state.field(previewField, false);
  if (pos === null || pos === undefined) return null;
  const { base, hunks: list } = hunks(state);
  const hunk = hunkAt(state, pos);
  const index = hunk ? list.findIndex((h) => h.chunk === hunk.chunk) : -1;
  return index < 0 || !base ? null : { hunk: list[index], index, count: list.length, base: base.text };
}

function preview(view: EditorView, hunk: Hunk | undefined) {
  const shown = previewedHunk(view.state);
  const same = shown && hunk && shown.hunk.chunk === hunk.chunk;
  if (!hunk && !shown) return info('No hunk here');
  view.dispatch({
    effects: togglePreview.of(same || !hunk ? null : view.state.doc.line(hunk.anchor).from),
  });
}

class RemovedLines extends WidgetType {
  constructor(
    readonly hunk: Hunk,
    readonly index: number,
    readonly count: number,
    readonly base: Text,
  ) {
    super();
  }

  eq(other: RemovedLines) {
    const a = this.hunk.chunk;
    const b = other.hunk.chunk;
    return (
      this.index === other.index &&
      this.count === other.count &&
      this.base === other.base &&
      a.fromA === b.fromA &&
      a.toA === b.toA &&
      a.toB - a.fromB === b.toB - b.fromB
    );
  }

  toDOM(view: EditorView) {
    const dom = document.createElement('div');
    dom.className = 'cm-git-preview';
    const bar = document.createElement('div');
    bar.className = 'cm-git-preview-bar';
    const added = this.hunk.lines ? this.hunk.lines.last - this.hunk.lines.first + 1 : 0;
    const label = document.createElement('span');
    label.textContent = `Hunk ${this.index + 1} of ${this.count}  +${added} −${this.hunk.removed}`;
    bar.append(label);
    const button = (text: string, title: string, run: () => void) => {
      const b = document.createElement('button');
      b.type = 'button';
      b.textContent = text;
      b.title = title;
      // Keep the cursor where it is.
      b.onmousedown = (e) => e.preventDefault();
      b.onclick = () => {
        run();
        view.focus();
      };
      bar.append(b);
    };
    const current = () => previewedHunk(view.state)?.hunk;
    button('Stage', 'Stage this hunk', () => {
      const h = current();
      if (h) stageHunks(view, [h]);
    });
    button('Reset', 'Reset this hunk to the index', () => {
      const h = current();
      if (h) resetHunks(view, [h]);
    });
    button('Close', 'Close the preview', () => view.dispatch({ effects: togglePreview.of(null) }));
    dom.append(bar);
    const { chunk } = this.hunk;
    if (chunk.toA > chunk.fromA) {
      const end = Math.min(chunk.endA, this.base.length);
      // Removed text within changed lines, as words gitsigns' word_diff marks.
      const marks = this.hunk.lines ? chunk.changes.map((c) => [chunk.fromA + c.fromA, chunk.fromA + c.toA]) : [];
      for (let pos = chunk.fromA; pos <= end;) {
        const line = this.base.lineAt(pos);
        const row = document.createElement('div');
        row.className = 'cm-git-removed-line';
        let at = line.from;
        for (const [from, to] of marks) {
          const s = Math.max(from, line.from);
          const e = Math.min(to, line.to);
          if (s >= e) continue;
          if (s > at) row.append(this.base.sliceString(at, s));
          const del = document.createElement('del');
          del.textContent = this.base.sliceString(s, e);
          row.append(del);
          at = e;
        }
        if (line.to > at) row.append(this.base.sliceString(at, line.to));
        // Keeps empty lines their height.
        if (!row.textContent) row.append('​');
        dom.append(row);
        pos = line.to + 1;
      }
    }
    return dom;
  }

  get estimatedHeight() {
    return -1;
  }

  ignoreEvent() {
    return true;
  }
}

const addedLine = Decoration.line({ class: 'cm-git-added-line' });
const addedText = Decoration.mark({ class: 'cm-git-added-text' });

const previewDecorations = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, tr) {
    if (!tr.docChanged && !tr.effects.some((e) => e.is(togglePreview) || e.is(setGitBase))) return value;
    const shown = previewedHunk(tr.state);
    if (!shown) return Decoration.none;
    const { hunk, base } = shown;
    const { chunk } = hunk;
    const doc = tr.state.doc;
    const ranges = [];
    const atLineStart = chunk.fromB < doc.length && doc.lineAt(chunk.fromB).from === chunk.fromB;
    // Above the hunk, or below the last line when lines were removed at the end.
    const widget = Decoration.widget({
      widget: new RemovedLines(hunk, shown.index, shown.count, base),
      block: true,
      side: atLineStart ? -1 : 1,
    });
    ranges.push(widget.range(atLineStart ? chunk.fromB : doc.length));
    if (hunk.lines) {
      for (let n = hunk.lines.first; n <= hunk.lines.last; n++) ranges.push(addedLine.range(doc.line(n).from));
      if (hunk.removed)
        for (const c of chunk.changes)
          if (c.toB > c.fromB) ranges.push(addedText.range(chunk.fromB + c.fromB, chunk.fromB + c.toB));
    }
    return Decoration.set(ranges, true);
  },
  provide: (f) => EditorView.decorations.from(f),
});

// --- Blame ---

const BLAME_KEY = 'btmux.editor.lineBlame';
const BLAME_DELAY_MS = 400;
/** Retries while the server hasn't applied this tab's edits yet. */
const BLAME_RETRIES = 6;

let lineBlame = (() => {
  try {
    return localStorage.getItem(BLAME_KEY) === '1';
  } catch {
    return false;
  }
})();
const blameViews = new Set<EditorView>();
const blameRedraw = StateEffect.define<null>();

function toggleLineBlame(): boolean {
  lineBlame = !lineBlame;
  try {
    localStorage.setItem(BLAME_KEY, lineBlame ? '1' : '0');
  } catch {
    // Remembered for this page only.
  }
  for (const view of blameViews) view.dispatch({ effects: blameRedraw.of(null) });
  info(lineBlame ? 'Line blame on' : 'Line blame off');
  return true;
}

function relativeTime(seconds: number): string {
  const diff = Date.now() / 1000 - seconds;
  const units: [number, string][] = [
    [60 * 60 * 24 * 365, 'year'],
    [60 * 60 * 24 * 30, 'month'],
    [60 * 60 * 24 * 7, 'week'],
    [60 * 60 * 24, 'day'],
    [60 * 60, 'hour'],
    [60, 'minute'],
  ];
  for (const [size, unit] of units) {
    const n = Math.floor(diff / size);
    if (n >= 1) return `${n} ${unit}${n === 1 ? '' : 's'} ago`;
  }
  return 'just now';
}

const blameText = (b: Blame) =>
  b.commit ? `${b.author}, ${relativeTime(b.time)} · ${b.summary}` : 'You · Uncommitted changes';

/** Blame `line` as the buffer has it, retrying while the server is behind this tab. */
async function requestBlame(view: EditorView, line: number, full: boolean): Promise<Blame | null> {
  for (let tries = 0; tries < BLAME_RETRIES; tries++) {
    const doc = viewDocs.get(view);
    const version = doc?.version();
    if (!doc?.buffer || version === null || version === undefined) return null;
    const text = view.state.doc;
    const result = await editorConnection.gitBlame(doc.buffer, version, line, full);
    if (view.state.doc !== text) return null;
    if (!result.stale) {
      if (result.error) throw new Error(result.error);
      return result.blame;
    }
    await new Promise((r) => setTimeout(r, 250));
  }
  return null;
}

class BlameWidget extends WidgetType {
  constructor(readonly text: string) {
    super();
  }
  eq(other: BlameWidget) {
    return other.text === this.text;
  }
  toDOM() {
    const el = document.createElement('span');
    el.className = 'cm-git-blame';
    el.textContent = this.text;
    return el;
  }
}

/** Shows the cursor line's blame at its end while line blame is on. */
const blamePlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet = Decoration.none;
    shown: { line: number; doc: Text; text: string } | null = null;
    timer: ReturnType<typeof setTimeout> | undefined;
    generation = 0;

    constructor(readonly view: EditorView) {
      blameViews.add(view);
      this.refresh();
    }

    update(u: ViewUpdate) {
      if (
        u.docChanged ||
        u.selectionSet ||
        u.transactions.some((tr) => tr.effects.some((e) => e.is(blameRedraw) || e.is(setGitBase)))
      )
        this.refresh();
    }

    cursorLine() {
      const state = this.view.state;
      return state.doc.lineAt(state.selection.main.head).number;
    }

    refresh() {
      clearTimeout(this.timer);
      const line = this.cursorLine();
      const current = lineBlame && this.shown?.doc === this.view.state.doc && this.shown.line === line;
      this.decorations = current
        ? Decoration.set(
            Decoration.widget({ widget: new BlameWidget(this.shown!.text), side: 1 }).range(
              this.view.state.doc.line(line).to,
            ),
          )
        : Decoration.none;
      if (!lineBlame || current || !inGit(this.view)) return;
      const generation = ++this.generation;
      this.timer = setTimeout(() => {
        const doc = this.view.state.doc;
        requestBlame(this.view, line, false)
          .then((blame) => {
            if (!blame || generation !== this.generation || this.view.state.doc !== doc) return;
            this.shown = { line, doc, text: blameText(blame) };
            this.view.dispatch({ effects: blameRedraw.of(null) });
          })
          .catch(() => {});
      }, BLAME_DELAY_MS);
    }

    destroy() {
      clearTimeout(this.timer);
      blameViews.delete(this.view);
    }
  },
  { decorations: (p) => p.decorations },
);

const setBlameTooltip = StateEffect.define<Tooltip | null>();
const blameTooltip = StateField.define<Tooltip | null>({
  create: () => null,
  update(value, tr) {
    for (const e of tr.effects) if (e.is(setBlameTooltip)) return e.value;
    return tr.docChanged || tr.selection ? null : value;
  },
  provide: (f) => showTooltip.from(f),
});

function blameLine(view: EditorView): boolean {
  const head = view.state.selection.main.head;
  const line = view.state.doc.lineAt(head);
  const doc = view.state.doc;
  requestBlame(view, line.number, true)
    .then((blame) => {
      if (view.state.doc !== doc || view.state.selection.main.head !== head) return;
      if (!blame) return info('Nothing to blame on this line');
      view.dispatch({
        effects: setBlameTooltip.of({
          pos: line.from,
          above: true,
          create() {
            const dom = document.createElement('div');
            dom.className = 'cm-git-blame-tooltip';
            const head = document.createElement('div');
            head.className = 'cm-git-blame-head';
            head.textContent = blame.commit
              ? `${blame.commit.slice(0, 8)} · ${blame.author} · ${new Date(blame.time * 1000).toLocaleString()} (${relativeTime(blame.time)})`
              : 'Uncommitted changes';
            dom.append(head);
            if (blame.message) {
              const body = document.createElement('div');
              body.className = 'cm-git-blame-message';
              body.textContent = blame.message;
              dom.append(body);
            }
            return { dom };
          },
        }),
      });
    })
    .catch((error) =>
      useStore.getState().showToast('Blame failed', 'error', { body: String(error?.message ?? error) }),
    );
  return true;
}

// --- Commands ---

const currentHunk = (view: EditorView) => {
  const line = view.state.doc.lineAt(view.state.selection.main.head).number;
  return hunks(view.state).hunks.find((h) => covers(h, line));
};

export const nextHunk = needsGit((view) => jump(view, true));
export const prevHunk = needsGit((view) => jump(view, false));
export const stageHunk = needsGit((view) => (stageHunks(view, selectedHunks(view.state)), true));
export const resetHunk = needsGit((view) => (resetHunks(view, selectedHunks(view.state)), true));
export const previewHunk = needsGit((view) => (preview(view, currentHunk(view)), true));
export const blameCurrentLine = needsGit(blameLine);
export const stageBuffer = needsGit((view) => {
  void stageText(view, () => view.state.doc.toString());
  return true;
});
export const resetBuffer = needsGit((view) => {
  const base = hunks(view.state).base!.text;
  view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: base }, userEvent: 'delete.git-reset' });
  return true;
});

/** The lines of the hunk at `line` (0-based), for vim's `ih`. */
export function hunkLines(view: EditorView, line: number): { first: number; last: number } | null {
  return hunks(view.state).hunks.find((h) => covers(h, line + 1))?.lines ?? null;
}

/** Commands for the btmux command palette, by action name. */
export const gitCommands: Record<string, (view: EditorView) => boolean> = {
  'editor-next-hunk': nextHunk,
  'editor-prev-hunk': prevHunk,
  'editor-preview-hunk': previewHunk,
  'editor-stage-hunk': stageHunk,
  'editor-reset-hunk': resetHunk,
  'editor-stage-buffer': stageBuffer,
  'editor-reset-buffer': resetBuffer,
  'editor-blame-line': blameCurrentLine,
  'editor-toggle-line-blame': toggleLineBlame,
};

const signGutter = gutter({
  class: 'cm-git-gutter',
  markers: (view) => view.state.field(signsField),
  initialSpacer: () => SIGNS.add,
  domEventHandlers: {
    mousedown(view, block) {
      const line = view.state.doc.lineAt(block.from).number;
      const hunk = hunks(view.state).hunks.find((h) => covers(h, line));
      if (!hunk) return false;
      preview(view, hunk);
      return true;
    },
  },
});

/** Git signs for a doc whose index text `initial` gives when a state is made. */
export function gitSigns(initial: () => GitBase | null): Extension {
  return [
    gitField.init((state) => compute(initial(), state.doc)),
    signsField,
    signGutter,
    previewField,
    previewDecorations,
    blamePlugin,
    blameTooltip,
    // VS Code's next and previous change.
    keymap.of([
      { key: 'Alt-F5', run: nextHunk, preventDefault: true },
      { key: 'Shift-Alt-F5', run: prevHunk, preventDefault: true },
    ]),
    EditorView.domEventHandlers({
      keydown(e, view) {
        const modifier = e.key === 'Shift' || e.key === 'Control' || e.key === 'Alt' || e.key === 'Meta';
        if (!modifier && !e.metaKey && view.state.field(blameTooltip))
          view.dispatch({ effects: setBlameTooltip.of(null) });
        return false;
      },
    }),
    EditorView.theme({
      '.cm-git-gutter .cm-gutterElement': { position: 'relative', width: '7px', padding: '0 0 0 2px' },
      '.cm-git-sign': { height: '100%', width: '3px', cursor: 'pointer' },
      '.cm-git-add': { backgroundColor: 'var(--btm-editor-green)' },
      '.cm-git-change': { backgroundColor: 'var(--btm-editor-blue)' },
      '.cm-git-changedelete': {
        backgroundColor: 'var(--btm-editor-blue)',
        borderBottom: '3px solid var(--btm-editor-red)',
        boxSizing: 'border-box',
      },
      '.cm-git-delete::after, .cm-git-topdelete::after': {
        content: '""',
        position: 'absolute',
        left: '2px',
        borderLeft: '6px solid var(--btm-editor-red)',
        borderTop: '4px solid transparent',
        borderBottom: '4px solid transparent',
        zIndex: '1',
      },
      '.cm-git-delete::after': { bottom: '-4px' },
      '.cm-git-topdelete::after': { top: '-4px' },
      '.cm-git-preview': {
        borderTop: '1px solid color-mix(in srgb, var(--btm-editor-blue) 50%, transparent)',
        borderBottom: '1px solid color-mix(in srgb, var(--btm-editor-blue) 50%, transparent)',
      },
      '.cm-git-preview-bar': {
        display: 'flex',
        gap: '8px',
        alignItems: 'center',
        padding: '1px 6px',
        fontSize: '85%',
        opacity: 0.85,
        backgroundColor: 'color-mix(in srgb, var(--btm-editor-blue) 10%, transparent)',
      },
      '.cm-git-preview-bar span': { marginRight: 'auto', whiteSpace: 'pre' },
      '.cm-git-preview-bar button': {
        font: 'inherit',
        color: 'inherit',
        background: 'none',
        border: 'none',
        padding: '0 2px',
        cursor: 'pointer',
      },
      '.cm-git-preview-bar button:hover': { textDecoration: 'underline' },
      '.cm-git-removed-line': {
        whiteSpace: 'pre',
        padding: '0 2px 0 6px',
        backgroundColor: 'color-mix(in srgb, var(--btm-editor-red) 14%, transparent)',
      },
      '.cm-git-removed-line del': {
        textDecoration: 'none',
        backgroundColor: 'color-mix(in srgb, var(--btm-editor-red) 30%, transparent)',
      },
      '.cm-git-added-line': { backgroundColor: 'color-mix(in srgb, var(--btm-editor-green) 12%, transparent)' },
      '.cm-git-added-text': { backgroundColor: 'color-mix(in srgb, var(--btm-editor-green) 28%, transparent)' },
      '.cm-git-blame': { opacity: 0.5, paddingLeft: '3em', fontStyle: 'italic', whiteSpace: 'pre' },
      '.cm-git-blame-tooltip': { padding: '4px 8px', maxWidth: '60ch' },
      '.cm-git-blame-head': { fontWeight: 'bold' },
      '.cm-git-blame-message': { whiteSpace: 'pre-wrap', marginTop: '4px' },
    }),
  ];
}
