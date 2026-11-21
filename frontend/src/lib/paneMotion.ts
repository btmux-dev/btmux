import { useLayoutEffect, type RefObject } from 'react';
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

/**
 * Animate `el`'s box from `from` to its rect, holding `content` at its final
 * size meanwhile (the box clips it). Returns a cancel function.
 */
function animateBox(el: HTMLElement, content: HTMLElement | null, from: Bounds, to: Bounds): () => void {
  const m = motion(el);
  if (!m) return noop;
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
  const animation = el.animate([paneRectStyle(from), paneRectStyle(to)], m);
  animation.onfinish = () => release();
  return () => {
    animation.cancel();
    release();
  };
}

/**
 * Reveal `el`, already at its final rect, through a clip that follows the box
 * from `from`, for surfaces over a pane, which would otherwise resize on every
 * frame (Neovim's grid, Chromium's viewport).
 */
function revealBox(el: HTMLElement, from: Bounds): () => void {
  const m = motion(el);
  if (!m) return noop;
  const to = el.getBoundingClientRect();
  const style = el.style;
  const saved = [style.top, style.left, style.width, style.height] as const;
  Object.assign(style, paneRectStyle(from));
  const start = el.getBoundingClientRect();
  [style.top, style.left, style.width, style.height] = saved;
  const radius = parseFloat(getComputedStyle(el).borderTopLeftRadius) || 0;
  const inset = (top: number, right: number, bottom: number, left: number) =>
    `inset(${top}px ${right}px ${bottom}px ${left}px round ${radius}px)`;
  const animation = el.animate(
    {
      clipPath: [
        inset(start.top - to.top, to.right - start.right, to.bottom - start.bottom, start.left - to.left),
        inset(0, 0, 0, 0),
      ],
    },
    m,
  );
  return () => animation.cancel();
}

/**
 * Animate a pane's box (`content` held at its final size) or, without
 * `content`, reveal a surface over it, whenever its rect changes with a
 * `from` (see `SessionPane`).
 */
export function usePaneMotion(
  ref: RefObject<HTMLElement | null>,
  rect: LayoutRect | null,
  content?: RefObject<HTMLElement | null>,
) {
  useLayoutEffect(() => {
    const el = ref.current;
    const from = rect?.from;
    if (!el || !rect || !from) return;
    return content ? animateBox(el, content.current, from, rect) : revealBox(el, from);
    // Keyed on the rect's values: rerenders with the same rect keep it going.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rect?.top, rect?.left, rect?.width, rect?.height]);
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
