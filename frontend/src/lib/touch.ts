/** A touchscreen is the primary pointer, as wterm decides for its input. */
export const TOUCH_QUERY = '(pointer: coarse)';

export const isTouchPrimary = () => window.matchMedia?.(TOUCH_QUERY).matches ?? false;

/** Phone-width screens, as ConfigPage and SessionSwitcher lay out. */
export const NARROW_QUERY = '(max-width: 767px)';

/**
 * Touchscreen phones in either orientation: narrow, or too short for several panes.
 * Never as wide as the nav drawer's breakpoint (`2xl`), where the side nav shows instead.
 */
export const PHONE_QUERY = `${TOUCH_QUERY} and (max-width: 767px), ${TOUCH_QUERY} and (max-height: 500px) and (max-width: 1535px)`;

/** Stop iOS capitalizing and autocorrecting an input; TextInput doesn't forward these. */
export function plainTextInput(input: HTMLInputElement | null) {
  input?.setAttribute('autocapitalize', 'off');
  input?.setAttribute('autocorrect', 'off');
}

/** Dialog props that fill a phone's visual viewport; 100dvh ignores the iOS keyboard. */
export const NARROW_DIALOG = {
  variant: 'fullscreen',
  style: { height: 'var(--btm-viewport-height, 100dvh)' },
} as const;
