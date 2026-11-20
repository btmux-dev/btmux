// Hover and diagnostics tooltips at the cursor, opened from the keyboard
// (`K`, `Mod-k Mod-i`, `<C-w>d`): lsp-client's hover only follows the pointer.
import { StateEffect, StateField, type Extension } from '@codemirror/state';
import { EditorView, ViewPlugin, showTooltip, type Tooltip } from '@codemirror/view';
import { forEachDiagnostic, type Diagnostic } from '@codemirror/lint';
import { ask } from './features';
import { gather, mayHave, pluginsReady } from './servers';

/** Keys that leave a tooltip up: modifiers alone, and Cmd chords such as copying from it. */
const keepsTooltip = (e: KeyboardEvent) =>
  e.key === 'Shift' || e.key === 'Control' || e.key === 'Alt' || e.key === 'Meta' || e.metaKey;

/** Counts keys, so a hover answered after one (Esc, `i`) isn't shown; a new state gets a new one. */
const requests = ViewPlugin.fromClass(
  class {
    generation = 0;
  },
  {
    eventObservers: {
      keydown(e, view) {
        if (keepsTooltip(e)) return;
        this.generation++;
        // Any key closes the tooltip, and still does what it does.
        if (view.state.field(tooltipField, false)) view.dispatch({ effects: setTooltip.of(null) });
      },
    },
  },
);

const setTooltip = StateEffect.define<Tooltip | null>();
const tooltipField = StateField.define<Tooltip | null>({
  create: () => null,
  update(value, tr) {
    for (const e of tr.effects) if (e.is(setTooltip)) return e.value;
    return tr.docChanged || tr.selection ? null : value;
  },
  provide: (f) => showTooltip.from(f),
});

type Markup = string | { language: string; value: string } | { kind: string; value: string };
type Hover = { contents: Markup | Markup[]; range?: { start: { line: number; character: number } } };

const markdown = (m: Markup) =>
  typeof m === 'string' ? m : 'language' in m ? '```' + m.language + '\n' + m.value + '\n```' : null;

/** Every server's hover for the symbol at the cursor, as a tooltip. */
export function showHover(view: EditorView): boolean {
  const counter = view.plugin(requests);
  if (!mayHave(view, 'hoverProvider') || !counter) return false;
  const generation = ++counter.generation;
  const pos = view.state.selection.main.head;
  void (async () => {
    const ps = await pluginsReady(view, 'hoverProvider');
    // A slow server's part is left out (`gather`).
    const answers = await gather(
      ps.map((p) => ask<Hover | null>(p, 'textDocument/hover', (p) => ({ position: p.toPosition(pos) }))),
    );
    const results = answers.map((result, i) => ({ p: ps[i], result: result ?? null }));
    const current = view.plugin(requests) === counter && counter.generation === generation;
    if (!current || view.state.selection.main.head !== pos) return;
    const html = results
      .map(({ p, result }) => {
        if (!result) return '';
        const parts = Array.isArray(result.contents) ? result.contents : [result.contents];
        return parts
          .map((part) => {
            const md = markdown(part);
            return md === null ? p.docToHTML(part as { kind: 'markdown'; value: string }) : p.docToHTML(md, 'markdown');
          })
          .filter(Boolean)
          .join('<br>');
      })
      .filter(Boolean)
      .join('<hr>');
    if (!html) return;
    const first = results.find((r) => r.result?.range);
    const at = first ? first.p.fromPosition(first.result!.range!.start) : pos;
    view.dispatch({
      effects: setTooltip.of({
        pos: Math.min(at, pos),
        above: true,
        create() {
          const dom = document.createElement('div');
          dom.className = 'cm-lsp-hover-tooltip cm-lsp-documentation';
          dom.innerHTML = html;
          return { dom };
        },
      }),
    });
  })();
  return true;
}

/** The diagnostics on the cursor's line, as lint's hover shows them. */
export function showDiagnostics(view: EditorView): boolean {
  const line = view.state.doc.lineAt(view.state.selection.main.head);
  const found: Diagnostic[] = [];
  forEachDiagnostic(view.state, (d, from, to) => {
    if (from <= line.to && to >= line.from) found.push({ ...d, from, to });
  });
  if (!found.length) return false;
  view.dispatch({
    effects: setTooltip.of({
      pos: Math.max(line.from, Math.min(found[0].from, line.to)),
      above: true,
      create() {
        const dom = document.createElement('ul');
        dom.className = 'cm-tooltip-lint';
        for (const d of found) {
          const item = document.createElement('li');
          item.className = `cm-diagnostic cm-diagnostic-${d.severity}`;
          const message = d.renderMessage ? d.renderMessage(view) : d.message;
          item.append(message);
          if (d.source) {
            const source = document.createElement('div');
            source.className = 'cm-diagnosticSource';
            source.textContent = d.source;
            item.append(source);
          }
          dom.append(item);
        }
        return { dom };
      },
    }),
  });
  return true;
}

export const keyboardTooltips: Extension = [tooltipField, requests];
