import { useLayoutEffect, useRef } from 'react';
import { previewUrl } from '@/lib/previewUrl';
import { useFileStore } from '@/state/fileStore';

const HTML_EXTENSIONS = new Set(['html', 'htm', 'xhtml']);

export function isHtmlFile(path: string | null): boolean {
  return HTML_EXTENSIONS.has(path?.split('.').pop()?.toLowerCase() ?? '');
}

export function HtmlPreview() {
  const selectedFile = useFileStore((s) => s.selectedFile);
  const frameRef = useRef<HTMLIFrameElement>(null);
  // Documents whose keys reach the file browser.
  const forwarded = useRef(new WeakSet<Document>());

  const returnFocus = () => {
    const frame = frameRef.current;
    if (document.activeElement !== frame) return;
    window.focus();
    frame?.closest<HTMLElement>('[tabindex="-1"]')?.focus();
  };

  // Keys never leave a focused iframe, so hand them to the file browser. Where
  // that can't work, the frame gives up focus instead: WebKit runs no listeners
  // on a sandboxed document, and a link may have left this origin.
  const forwardKeys = (): boolean => {
    const doc = frameRef.current?.contentDocument;
    const win = doc?.defaultView;
    if (!doc || !win) return false;
    if (forwarded.current.has(doc)) return true;
    let probed = false;
    win.addEventListener('btmux-probe', () => (probed = true), { once: true });
    win.dispatchEvent(new Event('btmux-probe'));
    if (!probed) return false;
    forwarded.current.add(doc);
    win.addEventListener('blur', onBlur);
    // A link's document would take keys before its load forwards them.
    doc.addEventListener('click', (e) => {
      if ((e.target as Element).closest?.('a[href]')) returnFocus();
    });
    doc.addEventListener(
      'keydown',
      (e) => {
        const replay = new KeyboardEvent('keydown', {
          key: e.key,
          code: e.code,
          ctrlKey: e.ctrlKey,
          altKey: e.altKey,
          shiftKey: e.shiftKey,
          metaKey: e.metaKey,
          repeat: e.repeat,
          bubbles: true,
          cancelable: true,
        });
        document.dispatchEvent(replay);
        if (replay.defaultPrevented) e.preventDefault();
      },
      true,
    );
    return true;
  };

  const onBlur = () => {
    // Now rather than on load, which a click into the frame can precede.
    const forwarding = forwardKeys();
    window.setTimeout(() => {
      // Keys in a nested frame, which may be cross-origin, never reach the listener above.
      const nested = frameRef.current?.contentDocument?.activeElement?.matches('iframe, frame');
      if (!forwarding || nested) returnFocus();
    });
  };

  useLayoutEffect(() => {
    window.addEventListener('blur', onBlur);
    return () => {
      window.removeEventListener('blur', onBlur);
      // Removing a focused frame would drop focus to <body>, where the file browser ignores keys.
      returnFocus();
    };
  }, []);

  if (!selectedFile) return null;

  return (
    <div className="flex-1 flex flex-col min-h-0">
      {/* Same origin so assets get the auth cookie; without allow-scripts nothing in it can use that. */}
      <iframe
        ref={frameRef}
        src={previewUrl(selectedFile)}
        sandbox="allow-same-origin"
        data-html-preview
        onLoad={() => {
          if (!forwardKeys()) returnFocus();
        }}
        className="flex-1 w-full border-0 bg-white"
        title={selectedFile.split('/').pop() ?? 'HTML'}
      />
    </div>
  );
}
