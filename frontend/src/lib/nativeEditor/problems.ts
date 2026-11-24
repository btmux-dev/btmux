// The problems list: every file's diagnostics in a bottom panel, most severe
// first, as Vim's quickfix list or VS Code's Problems view, or only the shown
// file's, as Vim's location list or VS Code's "Show Active File Only".
import type { EditorState, Extension } from '@codemirror/state';
import { EditorView, showPanel, type Panel } from '@codemirror/view';
import { IS_MAC } from '../nvimKeys';
import { editorConnection } from './connection';
import { fileDiagnostics, onDiagnostics, type FileDiagnostic } from './lsp';
import type { Position } from './lspText';

export interface ProblemsHost {
  /** Servers whose diagnostics are listed. */
  servers(): number[];
  /** The shown file's state, whose diagnostics follow its edits. */
  state(path: string): EditorState | null;
  /** The shown file. */
  active(): string | null;
  /** The folder paths are shown relative to. */
  root(): string | null;
  /** Show `path` at `at` and focus the editor. */
  jump(path: string, at: Position): void;
  /** Esc or the close button. */
  close(): void;
  /** Mod-Shift-M: as from the text, hide the workspace's list, else show it. */
  toggle(): void;
  /** Run the editor's chords that don't need the text (quick open); true if one ran. */
  key(event: KeyboardEvent): boolean;
}

const slashes = (path: string) => path.replace(/\\/g, '/');

/** Rows drawn beyond each edge of the visible ones. */
const OVERSCAN = 10;
/** Row height until one is measured. */
const FALLBACK_ROW_PX = 18;

const LETTERS: Record<number, [string, string]> = {
  1: ['E', 'error'],
  2: ['W', 'warning'],
  3: ['I', 'info'],
};

// Not by position, which edits move.
const keyOf = (d: FileDiagnostic) => `${d.path}\0${d.severity}\0${d.message}`;

/** Errors, warnings, and infos (hints mostly fade unused code), most severe first, then by file and position. */
export function sortedProblems(list: FileDiagnostic[]): FileDiagnostic[] {
  return list
    .filter((d) => d.severity <= 3)
    .sort(
      (a, b) =>
        a.severity - b.severity ||
        (a.path < b.path ? -1 : a.path > b.path ? 1 : 0) ||
        a.start.line - b.start.line ||
        a.start.character - b.start.character,
    );
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

let nextId = 0;

/** Every open file's problems, or the shown file's. */
export type ProblemsScope = 'workspace' | 'file';

interface Shown {
  focus(): void;
  refresh(): void;
}

/**
 * One pane editor's list. Each file's state gets its own panel while it is
 * open; the selection is kept here, so switching files keeps it.
 */
export class ProblemsList {
  readonly extension: Extension;
  private selected: string | null = null;
  /** Focus the list once the next panel mounts. */
  private focusNext = false;
  private current: Shown | null = null;
  scope: ProblemsScope = 'workspace';

  constructor(private readonly host: ProblemsHost) {
    this.extension = [showPanel.of((view) => this.panel(view)), theme];
  }

  /** Focus the shown list, now or once it mounts. */
  focus() {
    if (this.current) this.current.focus();
    else this.focusNext = true;
  }

  /** Closed: a list that never mounted doesn't take focus later. */
  hidden() {
    this.focusNext = false;
  }

  /** List the workspace's problems, or the shown file's. */
  setScope(scope: ProblemsScope) {
    if (scope === this.scope) return;
    this.scope = scope;
    this.current?.refresh();
  }

  /** List again: the open files, and so the servers, changed. */
  refresh() {
    this.current?.refresh();
  }

  private problems(scope = this.scope) {
    const all = sortedProblems(fileDiagnostics(this.host.servers(), (path) => this.host.state(path)));
    if (scope === 'workspace') return all;
    const active = this.host.active();
    return all.filter((d) => d.path === active);
  }

  /** Whether `scope` has any problem to list. */
  any(scope = this.scope): boolean {
    return this.problems(scope).length > 0;
  }

  private panel(view: EditorView): Panel {
    const id = `cm-btmux-problems-${nextId++}`;
    const dom = document.createElement('div');
    dom.className = 'cm-btmux-problems';
    const header = document.createElement('div');
    header.className = 'cm-btmux-problems-header';
    const title = document.createElement('span');
    title.className = 'cm-btmux-problems-title';
    title.textContent = 'Problems';
    // Which problems: a segmented toggle, also `f` in the list.
    const scopes = document.createElement('span');
    scopes.className = 'cm-btmux-problems-scopes';
    scopes.setAttribute('role', 'group');
    scopes.setAttribute('aria-label', 'Show problems of');
    const scopeButton = (scope: ProblemsScope, label: string) => {
      const button = document.createElement('button');
      button.type = 'button';
      button.textContent = label;
      button.title = 'f switches';
      button.addEventListener('mousedown', (e) => e.preventDefault());
      button.addEventListener('click', () => this.setScope(scope));
      return button;
    };
    const workspace = scopeButton('workspace', 'Workspace');
    const file = scopeButton('file', 'This file');
    scopes.append(workspace, file);
    // Set before the list mounts and takes focus, not on the first render.
    const pressed = () => {
      workspace.setAttribute('aria-pressed', String(this.scope === 'workspace'));
      file.setAttribute('aria-pressed', String(this.scope === 'file'));
    };
    pressed();
    const counts = document.createElement('span');
    counts.className = 'cm-btmux-problems-counts';
    const close = document.createElement('button');
    close.name = 'close';
    close.type = 'button';
    close.textContent = '×';
    close.setAttribute('aria-label', 'Close problems');
    close.addEventListener('mousedown', (e) => e.preventDefault());
    close.addEventListener('click', () => this.host.close());
    header.append(title, scopes, counts, close);
    const list = document.createElement('ul');
    list.id = id;
    list.tabIndex = 0;
    list.setAttribute('role', 'listbox');
    list.setAttribute('aria-label', 'Problems');
    // Inline, as the theme's styles may mount after the first draw, which reads the height.
    list.style.maxHeight = '12.5em';
    list.style.overflowY = 'auto';
    // Editor input, so the keybinding hook lets Cmd chords (Mod-Shift-M) through.
    list.dataset.nativeEditor = '';
    dom.append(header, list);

    let shown: FileDiagnostic[] = [];
    let index = 0;
    let prefix: string | null = null;
    let rowPx = 0;

    const row = (i: number) => {
      const d = shown[i];
      const li = document.createElement('li');
      li.id = `${id}-${i}`;
      li.setAttribute('role', 'option');
      li.setAttribute('aria-selected', String(i === index));
      li.setAttribute('aria-setsize', String(shown.length));
      li.setAttribute('aria-posinset', String(i + 1));
      const [letter, severity] = LETTERS[d.severity];
      li.className = `cm-btmux-problem cm-btmux-problem-${severity}`;
      const mark = document.createElement('span');
      mark.className = 'cm-btmux-problem-severity';
      mark.textContent = letter;
      mark.title = severity;
      const full = slashes(d.path);
      const path = prefix && full.startsWith(prefix) ? full.slice(prefix.length) : d.path;
      const location = document.createElement('span');
      location.className = 'cm-btmux-problem-location';
      location.textContent = `${path}:${d.start.line + 1}:${d.start.character + 1}`;
      const message = document.createElement('span');
      message.className = 'cm-btmux-problem-message';
      message.textContent = d.message.split('\n', 1)[0].trim();
      li.title = d.message;
      li.append(mark, location, message);
      const code = d.code !== undefined && d.code !== '' ? `(${d.code})` : '';
      if (d.source || code) {
        const source = document.createElement('span');
        source.className = 'cm-btmux-problem-source';
        source.textContent = `${d.source ?? ''}${code}`;
        li.append(source);
      }
      // Keeps focus where it is; a primary click picks it.
      li.addEventListener('mousedown', (e) => e.preventDefault());
      li.addEventListener('click', () => open(i));
      return li;
    };

    const height = () => rowPx || FALLBACK_ROW_PX;
    // Stand in for the rows above and below the drawn ones.
    const spacer = (px: number) => {
      const li = document.createElement('li');
      li.setAttribute('role', 'presentation');
      li.className = 'cm-btmux-problems-spacer';
      li.style.height = `${px}px`;
      return li;
    };
    const pageRows = () => Math.max(1, Math.floor(list.clientHeight / height()));

    /** Only the rows in view (and a few more) are in the DOM; scrolled to `scroll` if given. */
    const draw = (scroll?: number) => {
      const n = shown.length;
      if (!n) {
        const empty = document.createElement('li');
        empty.className = 'cm-btmux-problems-empty';
        empty.textContent = this.scope === 'file' ? 'No problems in this file' : 'No problems';
        list.replaceChildren(empty);
        list.setAttribute('aria-activedescendant', '');
        return;
      }
      if (!rowPx) {
        const probe = row(0);
        list.replaceChildren(probe);
        // Zero while hidden: measured again next time.
        rowPx = probe.offsetHeight;
      }
      const px = height();
      const visible = Math.min(list.clientHeight || px * 10, window.innerHeight);
      const top = Math.max(0, Math.min(scroll ?? list.scrollTop, n * px - visible));
      const start = Math.max(0, Math.floor(top / px) - OVERSCAN);
      const end = Math.min(n, Math.ceil((top + visible) / px) + OVERSCAN);
      const rows: HTMLElement[] = [];
      for (let i = start; i < end; i++) rows.push(row(i));
      list.replaceChildren(spacer(start * px), ...rows, spacer((n - end) * px));
      // Set once there is room for it.
      if (scroll !== undefined) list.scrollTop = top;
      list.setAttribute('aria-activedescendant', `${id}-${index}`);
      const measured = rows[0].offsetHeight;
      if (measured) rowPx = measured;
    };

    const select = (i: number) => {
      index = Math.max(0, Math.min(i, shown.length - 1));
      this.selected = shown[index] ? keyOf(shown[index]) : null;
      const px = height();
      const top = index * px;
      const scroll = list.scrollTop;
      draw(top < scroll ? top : top + px > scroll + list.clientHeight ? top + px - list.clientHeight : scroll);
    };

    let scope = this.scope;
    const render = () => {
      pressed();
      // Another list: from its top.
      if (scope !== this.scope) {
        scope = this.scope;
        this.selected = null;
        index = 0;
        list.scrollTop = 0;
      }
      const all = this.problems();
      const errors = all.filter((d) => d.severity === 1).length;
      const warnings = all.filter((d) => d.severity === 2).length;
      const infos = all.length - errors - warnings;
      counts.textContent = [
        plural(errors, 'error'),
        plural(warnings, 'warning'),
        ...(infos ? [`${infos} info`] : []),
      ].join(', ');
      const root = this.host.root();
      prefix = root ? `${slashes(root).replace(/\/+$/, '')}/` : null;
      // The same problem again, the one nearest where it was if there are several.
      let kept = -1;
      if (this.selected !== null)
        all.forEach((d, i) => {
          if (keyOf(d) === this.selected && (kept < 0 || Math.abs(i - index) < Math.abs(kept - index))) kept = i;
        });
      shown = all;
      select(kept >= 0 ? kept : Math.min(index, all.length - 1));
    };

    // Servers publish file by file, often many at once.
    let frame = 0;
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(() => ((frame = 0), render()));
    };
    let scrollFrame = 0;
    list.addEventListener('scroll', () => {
      if (!scrollFrame) scrollFrame = requestAnimationFrame(() => ((scrollFrame = 0), draw()));
    });

    const open = (i: number) => {
      // Positions as they are now, not as last drawn.
      if (frame) {
        cancelAnimationFrame(frame);
        frame = 0;
        select(i);
        render();
        i = index;
      }
      const d = shown[i];
      if (!d) return;
      select(i);
      this.host.jump(d.path, d.start);
    };

    list.addEventListener('keydown', (e) => {
      // Mod-Shift-M does here what it does in the text.
      if ((IS_MAC ? e.metaKey : e.ctrlKey) && e.shiftKey && !e.altKey && e.key.toLowerCase() === 'm') {
        e.preventDefault();
        e.stopPropagation();
        this.host.toggle();
        return;
      }
      if ((e.metaKey || e.ctrlKey || e.altKey) && this.host.key(e)) {
        e.preventDefault();
        e.stopPropagation();
        return;
      }
      if (e.metaKey || e.altKey) return;
      // Ctrl-N/Ctrl-P as in vim's menus, Ctrl-J/Ctrl-K as in fzf.
      const ctrl = e.ctrlKey && !e.shiftKey ? e.key.toLowerCase() : '';
      const plain = !e.ctrlKey ? e.key : '';
      let handled = true;
      if (plain === 'ArrowDown' || plain === 'j' || ctrl === 'n' || ctrl === 'j') select(index + 1);
      else if (plain === 'ArrowUp' || plain === 'k' || ctrl === 'p' || ctrl === 'k') select(index - 1);
      else if (plain === 'Home' || plain === 'g') select(0);
      else if (plain === 'End' || plain === 'G') select(shown.length - 1);
      else if (plain === 'PageDown') select(index + pageRows());
      else if (plain === 'PageUp') select(index - pageRows());
      else if (plain === 'Enter' || plain === 'o') open(index);
      else if (plain === 'Escape' || plain === 'q') this.host.close();
      else if (plain === 'f') this.setScope(this.scope === 'file' ? 'workspace' : 'file');
      else handled = false;
      if (handled) {
        e.preventDefault();
        e.stopPropagation();
      }
    });

    const handle: Shown = { focus: () => list.focus(), refresh: schedule };
    let stopDiagnostics = () => {};
    let stopStatus = () => {};
    return {
      dom,
      mount: () => {
        schedule();
        stopDiagnostics = onDiagnostics(schedule);
        stopStatus = editorConnection.onLspStatus(schedule);
        this.current = handle;
        if (this.focusNext) {
          this.focusNext = false;
          list.focus();
        }
      },
      // Edits move this file's problems.
      update: (update) => {
        if (update.docChanged) schedule();
      },
      destroy: () => {
        cancelAnimationFrame(frame);
        cancelAnimationFrame(scrollFrame);
        stopDiagnostics();
        stopStatus();
        if (this.current === handle) this.current = null;
        // Focus left with the panel: back to the text.
        if (dom.contains(document.activeElement)) view.focus();
      },
    };
  }
}

const theme = EditorView.theme({
  '.cm-panels-bottom:has(.cm-btmux-problems)': { maxHeight: 'max(45%, min(100%, 8em))' },
  '.cm-btmux-problems': { display: 'flex', flexDirection: 'column', minHeight: 0 },
  '.cm-btmux-problems-header': { display: 'flex', alignItems: 'center', gap: '1em', padding: '2px 4px 2px 12px' },
  '.cm-btmux-problems-title': { fontWeight: 'bold' },
  '.cm-btmux-problems-counts': { opacity: 0.6, flex: 1 },
  '.cm-btmux-problems-scopes': { display: 'flex', gap: '2px' },
  '.cm-btmux-problems-header .cm-btmux-problems-scopes button': {
    fontSize: 'inherit',
    lineHeight: 'inherit',
    padding: '0 6px',
    borderRadius: '4px',
    background: 'transparent',
    opacity: 0.6,
  },
  '.cm-btmux-problems-header .cm-btmux-problems-scopes button[aria-pressed=true]': {
    opacity: 1,
    background: 'var(--btm-editor-selection)',
  },
  '.cm-btmux-problems-header button': {
    border: 'none',
    color: 'inherit',
    cursor: 'pointer',
    fontSize: '1.2em',
    lineHeight: 1,
    padding: '0 6px',
  },
  '.cm-btmux-problems ul': {
    listStyle: 'none',
    margin: 0,
    padding: 0,
    overflowX: 'hidden',
    outline: 'none',
  },
  // One line each, so rows are one height (`draw`).
  '.cm-btmux-problems li': {
    display: 'flex',
    gap: '1ch',
    padding: '1px 12px',
    cursor: 'pointer',
    whiteSpace: 'pre',
    overflow: 'hidden',
  },
  '.cm-btmux-problems ul:focus li[aria-selected=true]': { background: 'var(--btm-editor-selection)' },
  '.cm-btmux-problems ul:not(:focus) li[aria-selected=true]': {
    background: 'color-mix(in srgb, var(--btm-editor-selection) 50%, transparent)',
  },
  '.cm-btmux-problem-severity': { fontWeight: 'bold', flex: 'none' },
  '.cm-btmux-problem-error .cm-btmux-problem-severity': { color: 'var(--btm-editor-red)' },
  '.cm-btmux-problem-warning .cm-btmux-problem-severity': { color: 'var(--btm-editor-yellow)' },
  '.cm-btmux-problem-info .cm-btmux-problem-severity': { color: 'var(--btm-editor-blue)' },
  '.cm-btmux-problem-location': {
    opacity: 0.6,
    flex: '0 1 auto',
    maxWidth: '45%',
    overflow: 'hidden',
    textOverflow: 'ellipsis',
  },
  '.cm-btmux-problem-message': { flex: '1 1 0', minWidth: '10ch', overflow: 'hidden', textOverflow: 'ellipsis' },
  '.cm-btmux-problem-source': { opacity: 0.6, flex: 'none' },
  '.cm-btmux-problems li.cm-btmux-problems-spacer': { padding: 0, cursor: 'default' },
  '.cm-btmux-problems-empty': { opacity: 0.6, cursor: 'default', padding: '1px 12px' },
});
