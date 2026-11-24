// Diagnostics' messages at the end of their lines, as Neovim's virtual_text.
import { forEachDiagnostic, setDiagnosticsEffect, type Diagnostic } from '@codemirror/lint';
import type { Range } from '@codemirror/state';
import { Decoration, EditorView, ViewPlugin, WidgetType, type DecorationSet, type ViewUpdate } from '@codemirror/view';
import { WRAP } from './wrap';

const RANK: Record<Diagnostic['severity'], number> = { error: 0, warning: 1, info: 2, hint: 3 };

class Message extends WidgetType {
  constructor(
    readonly severity: Diagnostic['severity'],
    readonly text: string,
    readonly more: number,
  ) {
    super();
  }

  eq(other: Message) {
    return other.severity === this.severity && other.text === this.text && other.more === this.more;
  }

  toDOM() {
    const el = document.createElement('span');
    el.className = `cm-btmux-virtualText cm-btmux-virtualText-${this.severity}`;
    el.setAttribute('aria-hidden', 'true');
    el.textContent = this.more > 0 ? `${this.text} (+${this.more})` : this.text;
    return el;
  }
}

/** Each visible line's most severe diagnostic, after the line's text. */
function build(view: EditorView): DecorationSet {
  const doc = view.state.doc;
  const visible = view.visibleRanges;
  const lines = new Map<number, { best: Diagnostic; count: number }>();
  forEachDiagnostic(view.state, (d, from) => {
    // Hints mostly fade unused code, which says it already.
    if (d.severity === 'hint' || !visible.some((r) => from >= r.from && from <= r.to)) return;
    const line = doc.lineAt(from).number;
    const seen = lines.get(line);
    if (!seen) lines.set(line, { best: d, count: 1 });
    else {
      seen.count++;
      if (RANK[d.severity] < RANK[seen.best.severity]) seen.best = d;
    }
  });
  const marks: Range<Decoration>[] = [];
  for (const [n, { best, count }] of lines) {
    const text = best.message.split('\n', 1)[0].trim();
    if (!text) continue;
    const widget = new Message(best.severity, text, count - 1);
    marks.push(Decoration.widget({ widget, side: 1 }).range(doc.line(n).to));
  }
  return Decoration.set(marks, true);
}

const plugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = build(view);
    }
    update(u: ViewUpdate) {
      const changed = u.transactions.some((tr) => tr.effects.some((e) => e.is(setDiagnosticsEffect)));
      if (changed || u.docChanged || u.viewportChanged) this.decorations = build(u.view);
    }
  },
  { decorations: (p) => p.decorations },
);

const theme = EditorView.baseTheme({
  '.cm-btmux-virtualText': {
    marginLeft: '3ch',
    padding: '0 0.5ch',
    borderRadius: '3px',
    whiteSpace: 'pre',
    fontStyle: 'italic',
    backgroundColor: 'color-mix(in srgb, currentColor 12%, transparent)',
  },
  // Wider than the content, it would stop every line wrapping.
  '.cm-lineWrapping .cm-btmux-virtualText': WRAP,
  '.cm-btmux-virtualText-error': { color: 'var(--btm-editor-red)' },
  '.cm-btmux-virtualText-warning': { color: 'var(--btm-editor-yellow)' },
  '.cm-btmux-virtualText-info': { color: 'var(--btm-editor-blue)' },
});

export const virtualText = [plugin, theme];
