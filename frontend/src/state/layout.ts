import { LayoutNode, LayoutRect } from './types';

export interface Bounds {
  top: number;
  left: number;
  width: number;
  height: number;
}

export interface Divider {
  id: string;
  orientation: 'vertical' | 'horizontal';
  // Position of the divider line as a percentage along the split region's axis
  position: number; // percentage along the container's axis where the split sits
  // Cross-axis span (percentage-based start and size)
  crossStart: number;
  crossSize: number;
  // The split region bounds (for ratio calculation during drag)
  boundsStart: number;
  boundsSize: number;
}

/**
 * Flatten a layout tree to its pane ids in depth-first order (left/top child
 * first), mirroring the backend's `Layout::pane_ids`. This is the same order
 * `computeRectsAndDividers` emits its rects in, so a rect's array index lines up
 * with this list — used to number panes for display-panes.
 */
export function paneIdsInOrder(layout: LayoutNode): string[] {
  switch (layout.type) {
    case 'leaf':
      return layout.pane_id ? [layout.pane_id] : [];
    case 'v_split':
      return [...paneIdsInOrder(layout.left!), ...paneIdsInOrder(layout.right!)];
    case 'h_split':
      return [...paneIdsInOrder(layout.top!), ...paneIdsInOrder(layout.bottom!)];
    default:
      return [];
  }
}

/** The stored ratio of the split with this id, or `null` if there is none. */
export function splitRatio(layout: LayoutNode, id: string): number | null {
  if (layout.type === 'leaf') return null;
  if (layout.id === id) return layout.ratio ?? 0.5;
  const [a, b] = layout.type === 'v_split' ? [layout.left!, layout.right!] : [layout.top!, layout.bottom!];
  return splitRatio(a, id) ?? splitRatio(b, id);
}

export function sameBounds(a: Bounds, b: Bounds): boolean {
  return a.top === b.top && a.left === b.left && a.width === b.width && a.height === b.height;
}

/**
 * `rect` collapsed to the outer edge of the split that directly holds the pane,
 * where a new pane grows from and a closed one shrinks to; `null` for a pane
 * not directly in a split.
 */
export function collapsedRect(layout: LayoutNode, paneId: string, rect: Bounds): Bounds | null {
  if (layout.type === 'leaf') return null;
  const vertical = layout.type === 'v_split';
  const [a, b] = vertical ? [layout.left!, layout.right!] : [layout.top!, layout.bottom!];
  const isPane = (node: LayoutNode) => node.type === 'leaf' && node.pane_id === paneId;
  if (isPane(a)) return vertical ? { ...rect, width: 0 } : { ...rect, height: 0 };
  if (isPane(b))
    return vertical
      ? { ...rect, left: rect.left + rect.width, width: 0 }
      : { ...rect, top: rect.top + rect.height, height: 0 };
  return collapsedRect(a, paneId, rect) ?? collapsedRect(b, paneId, rect);
}

/**
 * Decode a recursive `LayoutNode` tree into percentage-based pane rects plus the
 * divider lines between splits. Shared by SessionPane (the live grid, which uses
 * both rects and dividers) and WindowGrid (thumbnails, which use rects only).
 *
 * `ratioOverrides` maps a split node id to a live drag ratio; pass an empty map
 * to use each split's stored `ratio` (or 0.5 default).
 */
export function computeRectsAndDividers(
  layout: LayoutNode,
  bounds: Bounds,
  ratioOverrides: Map<string, number>,
): { rects: LayoutRect[]; dividers: Divider[] } {
  switch (layout.type) {
    case 'leaf':
      return { rects: [{ paneId: layout.pane_id!, ...bounds }], dividers: [] };

    case 'v_split': {
      const ratio = ratioOverrides.get(layout.id!) ?? layout.ratio ?? 0.5;
      const leftWidth = bounds.width * ratio;
      const rightWidth = bounds.width * (1 - ratio);

      const { rects: lr, dividers: ld } = computeRectsAndDividers(
        layout.left!,
        { ...bounds, width: leftWidth },
        ratioOverrides,
      );
      const { rects: rr, dividers: rd } = computeRectsAndDividers(
        layout.right!,
        { top: bounds.top, left: bounds.left + leftWidth, width: rightWidth, height: bounds.height },
        ratioOverrides,
      );

      const divider: Divider = {
        id: layout.id!,
        orientation: 'vertical',
        position: bounds.left + leftWidth, // % along horizontal axis
        crossStart: bounds.top,
        crossSize: bounds.height,
        boundsStart: bounds.left,
        boundsSize: bounds.width,
      };

      return { rects: [...lr, ...rr], dividers: [...ld, ...rd, divider] };
    }

    case 'h_split': {
      const ratio = ratioOverrides.get(layout.id!) ?? layout.ratio ?? 0.5;
      const topHeight = bounds.height * ratio;
      const bottomHeight = bounds.height * (1 - ratio);

      const { rects: tr, dividers: td } = computeRectsAndDividers(
        layout.top!,
        { ...bounds, height: topHeight },
        ratioOverrides,
      );
      const { rects: br, dividers: bd } = computeRectsAndDividers(
        layout.bottom!,
        { top: bounds.top + topHeight, left: bounds.left, width: bounds.width, height: bottomHeight },
        ratioOverrides,
      );

      const divider: Divider = {
        id: layout.id!,
        orientation: 'horizontal',
        position: bounds.top + topHeight, // % along vertical axis
        crossStart: bounds.left,
        crossSize: bounds.width,
        boundsStart: bounds.top,
        boundsSize: bounds.height,
      };

      return { rects: [...tr, ...br], dividers: [...td, ...bd, divider] };
    }

    default:
      return { rects: [], dividers: [] };
  }
}

/**
 * Space between panes and around the pane area. Layout percentages map onto
 * the area inset by half of it; each pane then gives up half on every side.
 */
export const PANE_GAP = 'var(--spacing-1)';

/** Where a layout percentage's pane edge starts along one axis. */
const paneStart = (pct: number) => `calc(${pct / 100} * (100% - ${PANE_GAP}) + ${PANE_GAP})`;
/** How long a span of layout percentage is along one axis, less the gap. */
const paneSize = (pct: number) => `calc(${pct / 100} * (100% - ${PANE_GAP}) - ${PANE_GAP})`;

/** Absolute position for a pane, or a surface over it, in the pane area. */
export function paneRectStyle(rect: Bounds) {
  return {
    top: paneStart(rect.top),
    left: paneStart(rect.left),
    width: paneSize(rect.width),
    height: paneSize(rect.height),
  };
}

/** Absolute position for a divider's touch target, centered in its gap. */
export function dividerStyle(divider: Divider) {
  const along = `calc(${divider.position / 100} * (100% - ${PANE_GAP}) + ${PANE_GAP} / 2)`;
  const crossStart = paneStart(divider.crossStart);
  const crossSize = paneSize(divider.crossSize);
  return divider.orientation === 'vertical'
    ? { top: crossStart, left: along, height: crossSize }
    : { top: along, left: crossStart, width: crossSize };
}

/**
 * The layout percentage at a pointer offset (px) along an axis of the pane
 * area, the inverse of `dividerStyle`. `gap` is `PANE_GAP` in px.
 */
export function layoutPercent(offset: number, size: number, gap: number): number {
  return ((offset - gap / 2) / (size - gap)) * 100;
}
