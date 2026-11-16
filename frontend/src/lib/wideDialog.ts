/** Dialog props for the file browser and popups: wide, or the whole visual viewport. */
export function wideDialogProps(fullscreen: boolean) {
  return fullscreen
    ? ({
        variant: 'fullscreen',
        style: {
          // Fullscreen's 100dvh ignores the iOS keyboard; useVisualViewport tracks it.
          height: 'var(--btm-viewport-height, 100dvh)',
          // padding={0} drops Dialog's safe-area padding.
          padding:
            'env(safe-area-inset-top, 0px) env(safe-area-inset-right, 0px) var(--btm-safe-bottom, env(safe-area-inset-bottom, 0px)) env(safe-area-inset-left, 0px)',
        },
      } as const)
    : ({ width: 'min(1440px, 94vw)', maxHeight: '90dvh', style: { height: '90dvh' } } as const);
}
