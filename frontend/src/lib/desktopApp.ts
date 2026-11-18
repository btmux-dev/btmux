import type { ColorMode } from '../generated/protocol';

type TauriWindow = Window & {
  __TAURI_INTERNALS__?: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> };
};

/** Runs a desktop app command, or returns null outside the Tauri shell. */
export function invokeDesktop(command: string, args?: Record<string, unknown>): Promise<unknown> | null {
  return (window as TauriWindow).__TAURI_INTERNALS__?.invoke(command, args) ?? null;
}

let version: Promise<string | null> | null = null;

/** The desktop app's version, or null outside the Tauri shell. */
export function getDesktopVersion(): Promise<string | null> {
  version ??= (async () => {
    const tauri = (window as TauriWindow).__TAURI_INTERNALS__;
    if (!tauri) return null;
    try {
      const result = await tauri.invoke('desktop_info');
      return typeof result === 'string' ? result : null;
    } catch {
      return null;
    }
  })();
  return version;
}

export function desktopVersionMismatch(desktop: string | null, server: string | undefined): boolean {
  return desktop !== null && server !== undefined && desktop !== server;
}

/** Whether btmux runs in the desktop app. */
export function isDesktopApp(): boolean {
  return !!(window as TauriWindow).__TAURI_INTERNALS__;
}

/** Returns the desktop app to its server picker. */
export function switchDesktopServer(): void {
  (window as TauriWindow).__TAURI_INTERNALS__?.invoke('switch_server').catch(() => {});
}

/** Tells the desktop app the shown scheme, which windows opened from this one match. */
export function setDesktopColorScheme(scheme: ColorMode): void {
  // Older apps lack the command.
  invokeDesktop('set_color_scheme', { scheme })?.catch(() => {});
}

/** Opens another desktop app window on the server picker. */
export function openDesktopWindow(): void {
  (window as TauriWindow).__TAURI_INTERNALS__?.invoke('new_window').catch(() => {});
}

/** Closes this desktop app window. */
export function closeDesktopWindow(): void {
  invokeDesktop('close_window')?.catch(() => {});
}

/**
 * Cmd+W closes the window on macOS, as the menu's Close Window did before
 * it moved to Cmd+Shift+W, unless the native editor has focus: it closes a
 * file with it, as VS Code does.
 */
export function installDesktopCloseKey(): void {
  if (!isDesktopApp() || !/Mac/.test(navigator.platform)) return;
  window.addEventListener(
    'keydown',
    (e) => {
      if (!e.metaKey || e.ctrlKey || e.altKey || e.shiftKey || e.key.toLowerCase() !== 'w') return;
      if (e.target instanceof Element && e.target.closest('[data-native-editor]')) return;
      e.preventDefault();
      closeDesktopWindow();
    },
    true,
  );
}
