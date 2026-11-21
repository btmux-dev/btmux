import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react';
import type { ClientMessage } from '../protocol/messages';
import { useStore } from '../state/store';
import { layoutPercent, splitRatio, type Divider } from '../state/layout';
import type { LayoutNode } from '../state/types';
import { trackPointerDrag } from './useSidebarResize';

interface Options {
  sessionId: string;
  /** The shown window, whose layout and dividers these are; null while hidden. */
  windowId: string | null;
  layout: LayoutNode | undefined;
  /** Zoomed windows show no dividers. */
  zoomed: boolean;
  /** The pane area, whose `column-gap` resolves `PANE_GAP` to px. */
  containerRef: RefObject<HTMLDivElement | null>;
  request: (msg: ClientMessage) => Promise<string | null>;
}

/** A released ratio, kept until the server's layout has it. */
interface Release {
  ratio: number;
}

/**
 * Dragging pane dividers. The drag shows a local ratio for its split while
 * terminals keep their grids (`dividerDragging`), so the shell doesn't redraw
 * its prompt at every step. A release keeps its ratio until the server's
 * layout has it, so the divider doesn't jump back for a round trip. Fitting
 * resumes once the released geometry is committed, so terminals resize once.
 */
export function useDividerDrag({ sessionId, windowId, layout, zoomed, containerRef, request }: Options) {
  const [overrides, setOverrides] = useState<Map<string, number>>(new Map());
  const releases = useRef(new Map<string, Release>());
  const drag = useRef<{ windowId: string; dividerId: string; cancel: () => void } | null>(null);
  // Set when a drag ends with a pointer event; the next commit resumes fitting.
  const resumeFit = useRef(false);

  const withoutOverride = (id: string) => (prev: Map<string, number>) => {
    const next = new Map(prev);
    next.delete(id);
    return next;
  };
  /** Drop a released ratio, if it is still `release`. */
  const drop = useCallback((id: string, release: Release) => {
    if (releases.current.get(id) !== release) return;
    releases.current.delete(id);
    setOverrides((prev) => (prev.has(id) ? withoutOverride(id)(prev) : prev));
  }, []);

  const onDividerPointerDown = useCallback(
    (divider: Divider, e: React.PointerEvent) => {
      e.stopPropagation();
      const container = containerRef.current;
      if (!container || windowId === null) return;

      const containerRect = container.getBoundingClientRect();
      const isVertical = divider.orientation === 'vertical';
      const gap = parseFloat(getComputedStyle(container).columnGap) || 0;
      const computeRatio = (event: PointerEvent): number => {
        const containerSize = isVertical ? containerRect.width : containerRect.height;
        const mousePos = isVertical ? event.clientX - containerRect.left : event.clientY - containerRect.top;
        const ratio = (layoutPercent(mousePos, containerSize, gap) - divider.boundsStart) / divider.boundsSize;
        return Math.max(0.05, Math.min(0.95, ratio));
      };
      const onMove = (event: PointerEvent) => {
        const ratio = computeRatio(event);
        setOverrides((prev) => new Map(prev).set(divider.id, ratio));
      };

      drag.current?.cancel();
      releases.current.delete(divider.id);
      useStore.getState().setDividerDragging(true);
      const cancel = trackPointerDrag(e, onMove, (event) => {
        drag.current = null;
        if (event === null) {
          // Canceled because the divider is going away: nothing to commit.
          setOverrides(withoutOverride(divider.id));
          useStore.getState().setDividerDragging(false);
          return;
        }
        resumeFit.current = true;
        if (event.type !== 'pointerup') {
          // A canceled touch (a system gesture) or lost capture keeps the old layout.
          setOverrides(withoutOverride(divider.id));
          return;
        }
        const ratio = computeRatio(event);
        const release: Release = { ratio };
        releases.current.set(divider.id, release);
        setOverrides((prev) => new Map(prev).set(divider.id, ratio));
        void request({ type: 'resize_split', session_id: sessionId, split_id: divider.id, ratio }).then((error) => {
          // Rejected, or the connection dropped: show the server's layout.
          if (error) drop(divider.id, release);
        });
        // Should no layout with it arrive, fall back to the server's.
        window.setTimeout(() => drop(divider.id, release), 2000);
      });
      drag.current = { windowId, dividerId: divider.id, cancel };
    },
    [sessionId, windowId, containerRef, request, drop],
  );

  // After the commit that applied a drag's end, terminals fit once, to it.
  useLayoutEffect(() => {
    if (!resumeFit.current) return;
    resumeFit.current = false;
    useStore.getState().setDividerDragging(false);
  });

  // A drag whose window or divider stops being shown (another client switched
  // windows, closed a pane, or zoomed) ends there.
  useEffect(() => {
    const current = drag.current;
    const shown =
      current?.windowId === windowId && !zoomed && !!layout && splitRatio(layout, current.dividerId) !== null;
    if (current && !shown) current.cancel();
  });
  useEffect(() => () => drag.current?.cancel(), []);

  useEffect(() => {
    for (const [id, release] of releases.current) {
      const stored = layout ? splitRatio(layout, id) : null;
      // f32 on the server.
      if (stored === null || Math.abs(stored - release.ratio) < 1e-4) drop(id, release);
    }
  }, [layout, drop]);

  return { overrides, onDividerPointerDown };
}
