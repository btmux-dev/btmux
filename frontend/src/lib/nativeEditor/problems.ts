// The problems list: every file's diagnostics in a bottom panel, most severe
// first, as Vim's quickfix list or VS Code's Problems view.
import type { Extension } from '@codemirror/state';
import { EditorView, showPanel, type Panel } from '@codemirror/view';
import { IS_MAC } from '../nvimKeys';
import { editorConnection } from './connection';
import { fileDiagnostics, onDiagnostics, type FileDiagnostic } from './lsp';
import type { Position } from './lspText';

export interface ProblemsHost {
  /** Servers whose diagnostics are listed. */
  servers(): number[];
  /** The folder paths are shown relative to. */
  root(): string | null;
  /** Show `path` at `at` and focus the editor. */
  jump(path: string, at: Position): void;
  /** Esc or the close button. */
  close(): void;
}

/** At most this many rows are drawn. */
const MAX_ROWS = 500;

const LETTERS: Record<number, [string, string]> = {
  1: ['E', 'error'],
  2: ['W', 'warning'],
  3: ['I', 'info'],
};

const keyOf = (d: FileDiagnostic) => `${d.path}\0${d.start.line}\0${d.start.character}\0${d.message}`;

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

/**
 * One pane editor's list. Each file's state gets its own panel while it is
 * open; the selection is kept here, so switching files keeps it.
 */
export class ProblemsList {
  readonly extension: Extension;
  private selected: string | null = null;
  /** Focus the list once the next panel mounts. */
  private focusNext = false;
  private current: { focus(): void } | null = null;

  constructor(private readonly host: ProblemsHost) {
    this.extension = [showPanel.of((view) => this.panel(view)), theme];
  }

  /** Focus the shown list, now or once it mounts. */
  focus() {
    if (this.current) this.current.focus();
    else this.focusNext = true;
  }

  /** Whether any problem would be listed. */
  any(): boolean {
    return sortedProblems(fileDiagnostics(this.host.servers())).length > 0;
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
    const counts = document.createElement('span');
    counts.className = 'cm-btmux-problems-counts';
    const close = document.createElement('button');
    close.name = 'close';
    close.type = 'button';
    close.textContent = '×';
    close.setAttribute('aria-label', 'Close problems');
    close.addEventListener('mousedown', (e) => e.preventDefault());
    close.addEventListener('click', () => this.host.close());
    header.append(title, counts, close);
    const list = document.createElement('ul');
    list.id = id;
    list.tabIndex = 0;
    list.setAttribute('role', 'listbox');
    list.setAttribute('aria-label', 'Problems');
    dom.append(header, list);

    let shown: FileDiagnostic[] = [];
    let index = 0;
    const select = (i: number) => {
      index = Math.max(0, Math.min(i, Math.min(shown.length, MAX_ROWS) - 1));
      this.selected = shown[index] ? keyOf(shown[index]) : null;
      for (const [n, li] of [...list.children].entries())
        if (li.getAttribute('role') === 'option') li.setAttribute('aria-selected', String(n === index));
      list.setAttribute('aria-activedescendant', shown.length ? `${id}-${index}` : '');
      list.children[index]?.scrollIntoView({ block: 'nearest' });
    };
    const open = (i: number) => {
      const d = shown[i];
      if (!d) return;
      select(i);
      this.host.jump(d.path, d.start);
    };

    const render = () => {
      const all = sortedProblems(fileDiagnostics(this.host.servers()));
      shown = all;
      const errors = all.filter((d) => d.severity === 1).length;
      const warnings = all.filter((d) => d.severity === 2).length;
      const infos = all.length - errors - warnings;
      counts.textContent = [
        plural(errors, 'error'),
        plural(warnings, 'warning'),
        ...(infos ? [`${infos} info`] : []),
      ].join(', ');
      const root = this.host.root();
      const prefix = root ? `${root.replace(/\/+$/, '')}/` : null;
      const rows: HTMLElement[] = all.slice(0, MAX_ROWS).map((d, i) => {
        const li = document.createElement('li');
        li.id = `${id}-${i}`;
        li.setAttribute('role', 'option');
        const [letter, severity] = LETTERS[d.severity];
        li.className = `cm-btmux-problem cm-btmux-problem-${severity}`;
        const mark = document.createElement('span');
        mark.className = 'cm-btmux-problem-severity';
        mark.textContent = letter;
        mark.title = severity;
        const path = prefix && d.path.startsWith(prefix) ? d.path.slice(prefix.length) : d.path;
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
        li.addEventListener('mousedown', (e) => {
          e.preventDefault();
          open(i);
        });
        return li;
      });
      if (!all.length) {
        const empty = document.createElement('li');
        empty.className = 'cm-btmux-problems-empty';
        empty.textContent = 'No problems';
        rows.push(empty);
      } else if (all.length > MAX_ROWS) {
        const more = document.createElement('li');
        more.className = 'cm-btmux-problems-empty';
        more.textContent = `${all.length - MAX_ROWS} more`;
        rows.push(more);
      }
      list.replaceChildren(...rows);
      const kept = this.selected === null ? -1 : all.findIndex((d) => keyOf(d) === this.selected);
      select(kept >= 0 ? kept : Math.min(index, all.length - 1));
    };

    // Servers publish file by file, often many at once.
    let frame = 0;
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(() => ((frame = 0), render()));
    };

    list.addEventListener('keydown', (e) => {
      // Mod-Shift-M toggles it from here too.
      if ((IS_MAC ? e.metaKey : e.ctrlKey) && e.shiftKey && !e.altKey && e.key.toLowerCase() === 'm') {
        e.preventDefault();
        this.host.close();
        return;
      }
      if (e.metaKey || e.altKey) return;
      // Ctrl-N/Ctrl-P as in vim's menus, Ctrl-J/Ctrl-K as in fzf.
      const ctrl = e.ctrlKey && !e.shiftKey ? e.key.toLowerCase() : '';
      const plain = !e.ctrlKey ? e.key : '';
      const n = Math.min(shown.length, MAX_ROWS);
      let handled = true;
      if (plain === 'ArrowDown' || plain === 'j' || ctrl === 'n' || ctrl === 'j') select(index + 1);
      else if (plain === 'ArrowUp' || plain === 'k' || ctrl === 'p' || ctrl === 'k') select(index - 1);
      else if (plain === 'Home' || plain === 'g') select(0);
      else if (plain === 'End' || plain === 'G') select(n - 1);
      else if (plain === 'PageDown') select(index + 10);
      else if (plain === 'PageUp') select(index - 10);
      else if (plain === 'Enter' || plain === 'o') open(index);
      else if (plain === 'Escape' || plain === 'q') this.host.close();
      else handled = false;
      if (handled) {
        e.preventDefault();
        e.stopPropagation();
      }
    });

    const handle = { focus: () => list.focus() };
    let stopDiagnostics = () => {};
    let stopStatus = () => {};
    return {
      dom,
      mount: () => {
        render();
        stopDiagnostics = onDiagnostics(schedule);
        stopStatus = editorConnection.onLspStatus(schedule);
        this.current = handle;
        if (this.focusNext) {
          this.focusNext = false;
          list.focus();
        }
      },
      destroy: () => {
        cancelAnimationFrame(frame);
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
    padding: '0 0 4px',
    overflowX: 'hidden',
    overflowY: 'auto',
    maxHeight: '12.5em',
    outline: 'none',
  },
  '.cm-btmux-problems li': {
    display: 'flex',
    gap: '1ch',
    padding: '1px 12px',
    cursor: 'pointer',
    whiteSpace: 'pre',
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
  '.cm-btmux-problems-empty': { opacity: 0.6, cursor: 'default' },
});
