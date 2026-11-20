import { useEffect } from 'react';
import { getBackdropBlur, getBackdropDim, getPopupBlur, getPopupOpacity } from '../state/configDefaults';
import type { ClientConfig } from '../state/types';

/**
 * Publishes the `backdrop-blur` / `backdrop-dim` config as CSS variables for
 * the global `dialog::backdrop` rule in index.css, which every modal shares,
 * and `popup-opacity` / `popup-blur` for editor popups (`POPUP_GLASS`).
 * `config` is the effective one, so the settings page previews them live.
 */
export function useModalBackdrop(config: ClientConfig | null): void {
  const blur = getBackdropBlur(config);
  const dim = getBackdropDim(config);
  const popupOpacity = getPopupOpacity(config);
  const popupBlur = getPopupBlur(config);
  useEffect(() => {
    const style = document.documentElement.style;
    style.setProperty('--btm-backdrop-blur', `${blur}px`);
    style.setProperty('--btm-backdrop-dim', String(dim));
    style.setProperty('--btm-popup-opacity', `${popupOpacity * 100}%`);
    style.setProperty('--btm-popup-blur', `${popupBlur}px`);
  }, [blur, dim, popupOpacity, popupBlur]);
}
