/** Floating popups: `popup-opacity`, blurred by `popup-blur` (`useModalBackdrop`). */
export const POPUP_GLASS = {
  backgroundColor: 'color-mix(in srgb, var(--color-background-popover) var(--btm-popup-opacity, 85%), transparent)',
  backdropFilter: 'blur(var(--btm-popup-blur, 2px))',
} as const;
