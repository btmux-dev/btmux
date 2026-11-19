import { StateEffect, StateField } from '@codemirror/state';
import { EditorView, showPanel, type Panel } from '@codemirror/view';

/** A choice in the editor's picker. */
export interface PickItem {
  label: string;
  /** Shown dimmed after the label. */
  detail?: string;
  /** Matched characters (code points) of the label and detail, highlighted. */
  labelMatches?: number[];
  detailMatches?: number[];
  run(): void;
}

export interface PickerSpec {
  title: string;
  /** Fixed items, filtered by what is typed… */
  items?: PickItem[];
  /**
   * …or items asked for as the query changes (workspace symbols, files).
   * `refresh`, called before the query resolves, replaces them with newer
   * ones for the same text later; Enter waits for those too.
   */
  query?: (text: string, refresh: (items: Promise<PickItem[]>) => void) => Promise<PickItem[]>;
}

const setPicker = StateEffect.define<PickerSpec | null>();

/** Whether `query`'s characters appear in `text` in order, ignoring case. */
export function fuzzyMatch(text: string, query: string): boolean {
  const t = text.toLowerCase();
  let at = 0;
  for (const c of query.toLowerCase()) {
    at = t.indexOf(c, at);
    if (at < 0) return false;
    at++;
  }
  return true;
}

let nextId = 0;

/** `text` with the code points at `matches` highlighted. */
function highlighted(text: string, matches: number[] | undefined): (string | HTMLElement)[] {
  if (!matches?.length) return [text];
  const at = new Set(matches);
  const parts: (string | HTMLElement)[] = [];
  let run = '';
  let marked = false;
  const flush = () => {
    if (!run) return;
    if (marked) {
      const mark = document.createElement('span');
      mark.className = 'cm-btmux-picker-match';
      mark.textContent = run;
      parts.push(mark);
    } else parts.push(run);
    run = '';
  };
  Array.from(text).forEach((c, i) => {
    if (at.has(i) !== marked) {
      flush();
      marked = !marked;
    }
    run += c;
  });
  flush();
  return parts;
}

function pickerPanel(view: EditorView, spec: PickerSpec): Panel {
  const id = `cm-btmux-picker-${nextId++}`;
  const dom = document.createElement('div');
  dom.className = 'cm-btmux-picker';
  const input = document.createElement('input');
  input.placeholder = spec.title;
  input.setAttribute('aria-label', spec.title);
  input.setAttribute('role', 'combobox');
  input.setAttribute('aria-expanded', 'true');
  input.setAttribute('aria-controls', id);
  const list = document.createElement('ul');
  list.id = id;
  list.setAttribute('role', 'listbox');
  list.setAttribute('aria-label', spec.title);
  dom.append(input, list);

  let shown: PickItem[] = [];
  let selected = 0;
  let generation = 0;
  // Closed or replaced: pending results and picks are dropped.
  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    view.dispatch({ effects: setPicker.of(null) });
    view.focus();
  };
  const render = () => {
    list.replaceChildren(
      ...shown.slice(0, 200).map((item, i) => {
        const li = document.createElement('li');
        li.id = `${id}-${i}`;
        li.setAttribute('role', 'option');
        li.setAttribute('aria-selected', String(i === selected));
        li.append(...highlighted(item.label, item.labelMatches));
        if (item.detail) {
          const detail = document.createElement('span');
          detail.className = 'cm-btmux-picker-detail';
          detail.append(...highlighted(item.detail, item.detailMatches));
          li.append(detail);
        }
        li.addEventListener('mousedown', (e) => {
          e.preventDefault();
          pick(i);
        });
        return li;
      }),
    );
    input.setAttribute('aria-activedescendant', shown.length ? `${id}-${selected}` : '');
    list.children[selected]?.scrollIntoView({ block: 'nearest' });
  };
  // The last update and its refreshes, which Enter waits for.
  let settled = Promise.resolve();
  const update = () => {
    const text = input.value;
    const g = ++generation;
    const refreshes: Promise<void>[] = [];
    const refresh = (next: Promise<PickItem[]>) => {
      refreshes.push(
        next.then(
          (items) => {
            if (g !== generation || closed) return;
            // Keep the selected item if it is still there.
            const current = shown[selected];
            shown = items;
            selected = Math.max(
              0,
              items.findIndex((item) => item.label === current?.label && item.detail === current?.detail),
            );
            render();
          },
          () => {},
        ),
      );
    };
    const first = (async () => {
      const items = spec.query ? await spec.query(text, refresh).catch(() => []) : (spec.items ?? []);
      if (g !== generation || closed) return;
      shown = spec.query ? items : items.filter((item) => fuzzyMatch(item.label, text));
      selected = 0;
      render();
    })();
    settled = first.then(() => Promise.all(refreshes)).then(() => {});
    return settled;
  };
  const pick = (i: number) => {
    if (closed) return;
    const item = shown[i];
    close();
    item?.run();
  };
  let timer: ReturnType<typeof setTimeout> | undefined;
  input.addEventListener('input', () => {
    clearTimeout(timer);
    timer = setTimeout(
      () => {
        timer = undefined;
        void update();
      },
      spec.query ? 150 : 0,
    );
  });
  input.addEventListener('keydown', (e) => {
    // Ctrl-N and Ctrl-P as in vim's menus.
    const ctrl = e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey;
    const down = e.key === 'ArrowDown' || (ctrl && e.key === 'n');
    if (down || e.key === 'ArrowUp' || (ctrl && e.key === 'p')) {
      e.preventDefault();
      const n = Math.min(shown.length, 200);
      if (n) selected = (selected + (down ? 1 : n - 1)) % n;
      render();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      // Pick from results for what was typed.
      if (timer !== undefined) {
        clearTimeout(timer);
        timer = undefined;
        void update();
      }
      void settled.then(() => pick(selected));
    } else if (e.key === 'Escape') {
      e.preventDefault();
      close();
    }
  });
  input.addEventListener('blur', () =>
    setTimeout(() => dom.isConnected && !dom.contains(document.activeElement) && close()),
  );
  void update();
  return {
    dom,
    top: true,
    mount: () => input.focus(),
    destroy: () => {
      closed = true;
      clearTimeout(timer);
    },
  };
}

const pickerField = StateField.define<PickerSpec | null>({
  create: () => null,
  update: (value, tr) => tr.effects.reduce((v, e) => (e.is(setPicker) ? e.value : v), value),
  provide: (f) => showPanel.from(f, (spec) => (spec ? (view) => pickerPanel(view, spec) : null)),
});

export const pickerExtension = [
  pickerField,
  EditorView.theme({
    // Capped on the panels, as a percentage on the picker would resolve
    // against their flex-shrunk height and clip the list. Short editors
    // give it at least 10em, or all of them.
    '.cm-panels-top:has(.cm-btmux-picker)': {
      display: 'flex',
      flexDirection: 'column',
      maxHeight: 'max(45%, min(100%, 10em))',
    },
    '.cm-btmux-picker': { display: 'flex', flexDirection: 'column', minHeight: 0 },
    '.cm-btmux-picker input': {
      font: 'inherit',
      margin: '6px 8px',
      padding: '2px 6px',
      color: 'inherit',
      background: 'transparent',
      border: '1px solid var(--color-border)',
      borderRadius: '4px',
      outline: 'none',
    },
    '.cm-btmux-picker ul': {
      listStyle: 'none',
      margin: 0,
      padding: '0 0 4px',
      overflowX: 'hidden',
      overflowY: 'auto',
      maxHeight: '240px',
    },
    '.cm-btmux-picker li': {
      padding: '1px 12px',
      cursor: 'pointer',
      whiteSpace: 'pre',
      overflow: 'hidden',
      textOverflow: 'ellipsis',
    },
    '.cm-btmux-picker li[aria-selected=true]': { background: 'var(--btm-editor-selection)' },
    '.cm-btmux-picker-detail': { opacity: 0.6, marginLeft: '1em' },
    '.cm-btmux-picker-match': { color: 'var(--btm-editor-blue)', fontWeight: 'bold' },
  }),
];

/** Show the picker in `view`. */
export function openPicker(view: EditorView, spec: PickerSpec) {
  const effects: StateEffect<unknown>[] = [setPicker.of(spec)];
  if (!view.state.field(pickerField, false)) effects.push(StateEffect.appendConfig.of(pickerExtension));
  view.dispatch({ effects });
}
