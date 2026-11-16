// With Tauri's drop handler off, WebKit loads what is dropped where the page
// doesn't take it; keep the page instead. Frames handle their own drops.
for (const type of ['dragover', 'drop']) {
  window.addEventListener(type, (e) => {
    if (e.defaultPrevented) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'none';
  });
}
