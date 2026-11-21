import { useLayoutEffect, useRef, type RefObject } from 'react';
import { paneRectStyle, type Bounds } from '../state/layout';
import type { LayoutRect } from '../state/types';

/**
 * Layout changes (splits, closes, layout cycling) animate each pane's box from
 * its old rect to its new one. Contents take their final size at once, so a
 * terminal or surface resizes once rather than on every frame, which would
 * reflow the grid and have the shell redraw its prompt each time.
 */

interface Motion {
  duration: number;
  easing: string;
}

const noop = () => {};

/** The theme's layout motion at `el`, or `null` when motion is off. */
function motion(el: Element): Motion | null {
  if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return null;
  const style = getComputedStyle(el);
  // Astryx's medium band is for transitions that rearrange the layout;
  // `animations = false` sets it to 0.
  const duration = parseFloat(style.getPropertyValue('--duration-medium-min'));
  if (!(duration > 0)) return null;
  return { duration, easing: style.getPropertyValue('--ease-standard').trim() || 'ease-out' };
}

/** A box in viewport pixels, as `getBoundingClientRect` gives it. */
interface Box {
  top: number;
  left: number;
  width: number;
  height: number;
}

/** Where a motion starts: layout bounds, or where an interrupted one had got to. */
type Start = { bounds: Bounds } | { box: Box };

interface Running {
  cancel(): void;
  /** Where the box shows now. */
  box(): Box;
}

/** `el`'s box were it at `bounds`, measured without a frame showing it. */
function boxAt(el: HTMLElement, bounds: Bounds): Box {
  const style = el.style;
  const saved = [style.top, style.left, style.width, style.height] as const;
  Object.assign(style, paneRectStyle(bounds));
  const box = el.getBoundingClientRect();
  [style.top, style.left, style.width, style.height] = saved;
  return box;
}

/**
 * Animate `el`'s box from `start` to its rect, holding `content` at its final
 * size meanwhile (the box clips it).
 */
function animateBox(el: HTMLElement, content: HTMLElement | null, start: Start, to: Bounds): Running | null {
  const m = motion(el);
  if (!m) return null;
  let release = noop;
  if (content) {
    const { width, height } = content.getBoundingClientRect();
    const style = content.style;
    const saved = [style.width, style.height, style.right, style.bottom] as const;
    Object.assign(style, { width: `${width}px`, height: `${height}px`, right: 'auto', bottom: 'auto' });
    release = () => {
      [style.width, style.height, style.right, style.bottom] = saved;
      release = noop;
    };
  }
  let from: Keyframe;
  if ('bounds' in start) from = paneRectStyle(start.bounds);
  else {
    const parent = (el.offsetParent ?? document.documentElement).getBoundingClientRect();
    const { top, left, width, height } = start.box;
    from = {
      top: `${top - parent.top}px`,
      left: `${left - parent.left}px`,
      width: `${width}px`,
      height: `${height}px`,
    };
  }
  const animation = el.animate([from, paneRectStyle(to)], m);
  animation.onfinish = () => release();
  return {
    cancel: () => {
      animation.cancel();
      release();
    },
    box: () => el.getBoundingClientRect(),
  };
}

/**
 * Reveal `el`, already at its final rect, through a clip that follows the box
 * from `start`, for surfaces over a pane, which would otherwise resize on every
 * frame (Neovim's grid, Chromium's viewport).
 */
function revealBox(el: HTMLElement, start: Start): Running | null {
  const m = motion(el);
  if (!m) return null;
  const to = el.getBoundingClientRect();
  const from = 'box' in start ? start.box : boxAt(el, start.bounds);
  const radius = parseFloat(getComputedStyle(el).borderTopLeftRadius) || 0;
  const inset = (top: number, right: number, bottom: number, left: number) =>
    `inset(${top}px ${right}px ${bottom}px ${left}px round ${radius}px)`;
  const animation = el.animate(
    {
      clipPath: [
        inset(
          from.top - to.top,
          to.right - (from.left + from.width),
          to.bottom - (from.top + from.height),
          from.left - to.left,
        ),
        inset(0, 0, 0, 0),
      ],
    },
    m,
  );
  const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
  return {
    cancel: () => animation.cancel(),
    // `progress` is eased, as the clip is.
    box: () => {
      const t = animation.effect?.getComputedTiming().progress ?? 1;
      return {
        top: lerp(from.top, to.top, t),
        left: lerp(from.left, to.left, t),
        width: lerp(from.width, to.width, t),
        height: lerp(from.height, to.height, t),
      };
    },
  };
}

/**
 * Animate a pane's box (`content` held at its final size) or, without
 * `content`, reveal a surface over it, whenever its rect changes with a
 * `from` (see `useLayoutMotion`). A change while one runs starts from where
 * the box shows, not from the last target.
 */
export function usePaneMotion(
  ref: RefObject<HTMLElement | null>,
  rect: LayoutRect | null,
  content?: RefObject<HTMLElement | null>,
) {
  const running = useRef<Running | null>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    const interrupted = running.current;
    running.current = null;
    // Sampled before canceling, while the box still shows mid-motion.
    const box = interrupted?.box();
    interrupted?.cancel();
    const from = rect?.from;
    if (!el || !rect || !from) return;
    const start: Start = box ? { box } : { bounds: from };
    running.current = content ? animateBox(el, content.current, start, rect) : revealBox(el, start);
    // Keyed on the rect's values: rerenders with the same rect keep it going.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rect?.top, rect?.left, rect?.width, rect?.height]);
  useLayoutEffect(() => () => running.current?.cancel(), []);
}

/** Animate a closed pane's empty box to `to`, then call `done`. */
export function animateGhost(el: HTMLElement, from: Bounds, to: Bounds, done: () => void): () => void {
  const m = motion(el);
  if (!m) {
    done();
    return noop;
  }
  const animation = el.animate([paneRectStyle(from), paneRectStyle(to)], { ...m, fill: 'forwards' });
  animation.onfinish = done;
  return () => animation.cancel();
}
