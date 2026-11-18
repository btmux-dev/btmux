// Hover and diagnostics tooltips at the cursor, opened from the keyboard
// (`K`, `Mod-k Mod-i`, `<C-w>d`): lsp-client's hover only follows the pointer.
import { StateEffect, StateField, type Extension } from '@codemirror/state';
import { EditorView, ViewPlugin, showTooltip, type Tooltip } from '@codemirror/view';
import { LSPPlugin } from '@codemirror/lsp-client';
import { forEachDiagnostic, type Diagnostic } from '@codemirror/lint';
import { ask, has } from './features';

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

/** The server's hover for the symbol at the cursor, as a tooltip. */
export function showHover(view: EditorView): boolean {
  const plugin = LSPPlugin.get(view);
  const counter = view.plugin(requests);
  if (!plugin || !counter || !has(plugin, 'hoverProvider')) return false;
  const generation = ++counter.generation;
  const pos = view.state.selection.main.head;
  void (async () => {
    const result = await ask<Hover | null>(view, 'textDocument/hover', (p) => ({ position: p.toPosition(pos) })).catch(
      () => null,
    );
    const current = view.plugin(requests) === counter && counter.generation === generation;
    if (!result || !current || view.state.selection.main.head !== pos) return;
    const parts = Array.isArray(result.contents) ? result.contents : [result.contents];
    const html = parts
      .map((part) => {
        const md = markdown(part);
        return md === null
          ? plugin.docToHTML(part as { kind: 'markdown'; value: string })
          : plugin.docToHTML(md, 'markdown');
      })
      .filter(Boolean)
      .join('<br>');
    if (!html) return;
    const at = result.range ? plugin.fromPosition(result.range.start) : pos;
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
