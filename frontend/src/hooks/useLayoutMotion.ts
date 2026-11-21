import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react';
import { collapsedRect, sameBounds, type Bounds } from '../state/layout';
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
  const [settling, setSettling] = useState(false);
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
        const from = prev.rects.get(rect.paneId) ?? collapsedRect(layout, rect.paneId, rect);
        return from && !sameBounds(from, rect) ? { ...rect, from } : rect;
      })
    : rects;
  const rectByPane = new Map(moving.map((r) => [r.paneId, r]));

  useLayoutEffect(() => {
    if (moved && prev) {
      const closed = [...prev.rects.values()].flatMap((rect) => {
        if (rectByPane.has(rect.paneId)) return [];
        const to = collapsedRect(prev.layout, rect.paneId, rect);
        return to ? [{ key: rect.paneId, windowId, from: rect, to }] : [];
      });
      if (closed.length) setGhosts((current) => [...current, ...closed]);
      // Panes' layout effects, which start their animations, ran first.
      const animations = (containerRef.current?.getAnimations({ subtree: true }) ?? []).filter(
        (animation) => animation.constructor === Animation,
      );
      const id = ++motion.current;
      setSettling(animations.length > 0);
      void Promise.all(animations.map((animation) => animation.finished.catch(() => {}))).then(() => {
        if (motion.current === id) setSettling(false);
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
    settling,
  };
}
