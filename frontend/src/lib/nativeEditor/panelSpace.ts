// Panels float over the text (`theme.ts`), so scroll margins alone can't move
// the first or last line clear of them: the scroller pads its edges by their
// height, and a panel opening scrolls the cursor clear if it was in view.
import { EditorView, ViewPlugin, showPanel, type ViewUpdate } from '@codemirror/view';

const SIDES = [
  ['cm-panels-top', 'paddingTop'],
  ['cm-panels-bottom', 'paddingBottom'],
] as const;

export const panelSpace = ViewPlugin.fromClass(
  class {
    private observer = new ResizeObserver(() => this.view.requestMeasure(this.measure));
    private observed = new Set<Element>();
    private padding = [0, 0];
    private destroyed = false;
    private measure = {
      read: () =>
        SIDES.map(([cls]) => {
          const el = this.view.dom.querySelector(`:scope > .${cls}`);
          if (el && !this.observed.has(el)) {
            this.observed.add(el);
            this.observer.observe(el);
          }
          return el instanceof HTMLElement ? el.offsetHeight : 0;
        }),
      write: (heights: number[]) => {
        if (heights.every((h, i) => h === this.padding[i])) return;
        const grew = heights.some((h, i) => h > this.padding[i]);
        this.padding = heights;
        SIDES.forEach(([, side], i) => (this.view.scrollDOM.style[side] = heights[i] ? `${heights[i]}px` : ''));
        const head = this.view.state.selection.main.head;
        if (grew && this.view.visibleRanges.some((r) => head >= r.from && head <= r.to))
          // Not while measuring, when the view can't update.
          queueMicrotask(() => {
            if (!this.destroyed)
              this.view.dispatch({ effects: EditorView.scrollIntoView(this.view.state.selection.main.head) });
          });
      },
    };

    constructor(private readonly view: EditorView) {
      view.requestMeasure(this.measure);
    }

    update(update: ViewUpdate) {
      if (update.startState.facet(showPanel) !== update.state.facet(showPanel)) {
        for (const el of this.observed)
          if (!el.isConnected) {
            this.observed.delete(el);
            this.observer.unobserve(el);
          }
        update.view.requestMeasure(this.measure);
      }
    }

    destroy() {
      this.destroyed = true;
      this.observer.disconnect();
      for (const [, side] of SIDES) this.view.scrollDOM.style[side] = '';
    }
  },
);
