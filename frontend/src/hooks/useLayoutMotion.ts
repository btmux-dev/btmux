import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react';
import { collapsedRect, sameBounds, type Bounds } from '../state/layout';
import { PANE_MOTION } from '../lib/paneMotion';
import type { LayoutNode, LayoutRect } from '../state/types';

/** A closed pane's empty box, shrinking to the edge it grew from. */
export interface PaneGhost {
  key: string;
  windowId: string;
  from: Bounds;
  to: Bounds;
}

interface Options {
  /** The shown window; null while its session is hidden. */
  windowId: string | null;
  layout: LayoutNode | undefined;
  zoomed: boolean;
  rects: LayoutRect[];
  /** The pane area, whose boxes' animations decide when it has settled. */
  containerRef: RefObject<HTMLDivElement | null>;
}

/**
 * Layout motion for the shown window. When its layout changes (a split, a
 * close, a layout cycle), each rect gets the one it moves `from`, which
 * `usePaneMotion` animates; new panes grow from the outer edge of their split,
 * and closed ones leave a box shrinking back to it. Window switches, drags,
 * and zoom don't animate. `settling` holds while boxes move, when dividers
 * would sit ahead of the edges they move.
 */
export function useLayoutMotion({ windowId, layout, zoomed, rects, containerRef }: Options) {
  // The shown window's layout and rects as last committed, to animate from.
  const shownRef = useRef<{
    windowId: string;
    layoutKey: string;
    layout: LayoutNode;
    rects: Map<string, LayoutRect>;
  } | null>(null);
  const [ghosts, setGhosts] = useState<PaneGhost[]>([]);
  // The window whose boxes are still moving.
  const [settling, setSettling] = useState<string | null>(null);
  const motion = useRef(0);

  const layoutKey = layout ? JSON.stringify(layout) : '';
  const prev = shownRef.current;
  const moved =
    windowId !== null &&
    !!layout &&
    !zoomed &&
    prev !== null &&
    prev.windowId === windowId &&
    prev.layoutKey !== layoutKey;
  const moving = moved
    ? rects.map((rect) => {
        const was = prev.rects.get(rect.paneId);
        // Geometry only: a rect's own `from` would chain every earlier one.
        const from = was ? bounds(was) : collapsedRect(layout, rect.paneId, rect);
        return from && !sameBounds(from, rect) ? { ...rect, from } : rect;
      })
    : rects;
  const rectByPane = new Map(moving.map((r) => [r.paneId, r]));

  useLayoutEffect(() => {
    if (moved && prev) {
      const closed = [...prev.rects.values()].flatMap((rect) => {
        if (rectByPane.has(rect.paneId)) return [];
        const to = collapsedRect(prev.layout, rect.paneId, bounds(rect));
        return to ? [{ key: rect.paneId, windowId, from: bounds(rect), to }] : [];
      });
      if (closed.length) setGhosts((current) => [...current, ...closed]);
      // Panes' layout effects, which start their animations, ran first.
      const animations = (containerRef.current?.getAnimations({ subtree: true }) ?? []).filter(
        (animation) => animation.id === PANE_MOTION,
      );
      const id = ++motion.current;
      setSettling(animations.length > 0 ? windowId : null);
      void Promise.all(animations.map((animation) => animation.finished.catch(() => {}))).then(() => {
        if (motion.current === id) setSettling(null);
      });
    }
    shownRef.current =
      windowId !== null && layout && !zoomed ? { windowId, layoutKey, layout, rects: rectByPane } : null;
  });

  // Outlines belong to the window that closed them.
  useEffect(() => {
    setGhosts((current) => {
      const kept = current.filter((ghost) => ghost.windowId === windowId && !zoomed);
      return kept.length === current.length ? current : kept;
    });
  }, [windowId, zoomed]);
  const removeGhost = useCallback((ghost: PaneGhost) => setGhosts((current) => current.filter((g) => g !== ghost)), []);

  return {
    rects: moving,
    rectByPane,
    ghosts: ghosts.filter((ghost) => ghost.windowId === windowId && !zoomed),
    removeGhost,
    settling: settling !== null && settling === windowId,
  };
}

const bounds = ({ top, left, width, height }: Bounds): Bounds => ({ top, left, width, height });
