// Cmd+W on macOS: the menus' Close Window is ⌘⇧W, so pages get ⌘W first.
// One that takes it (btmux's editor closing a file) cancels it; otherwise it
// closes the window, on every page: btmux, the picker, other servers'.
// CodeMirror panels (search, rename, pickers) keep it from closing the window.
{
  if (/Mac/.test(navigator.platform))
    window.addEventListener(
      'keydown',
      (e) => {
        if (!e.metaKey || e.ctrlKey || e.altKey || e.shiftKey || e.key.toLowerCase() !== 'w') return;
        if (e.target instanceof Element && e.target.closest('.cm-panels')) return e.preventDefault();
        // After the page's own handlers.
        setTimeout(() => {
          if (!e.defaultPrevented) window.__TAURI_INTERNALS__?.invoke('close_window').catch(() => {});
        });
      },
      true,
    );
}
