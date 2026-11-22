import { useEffect, useRef, useSyncExternalStore, type ReactNode } from 'react';
import { HStack, VStack } from '@astryxdesign/core/Layout';
import { Lock } from 'lucide-react';
import { clsx } from 'clsx';
import type { KeyOverlayPosition } from '../generated/protocol';
import { ENTRY_MS, keyCaster, type Cap, type Entry, type KeyInfo } from '../lib/keyCast';
import { terminalInput } from '../lib/terminalInput';
import { getAnimations } from '../state/configDefaults';
import { useStore } from '../state/store';

const POSITIONS: Record<KeyOverlayPosition, string> = {
  bottom: 'bottom-16 left-1/2 -translate-x-1/2 items-center',
  'bottom-left': 'bottom-16 left-4 items-start',
  'bottom-right': 'bottom-16 right-4 items-end',
  top: 'top-4 left-1/2 -translate-x-1/2 items-center',
  'top-left': 'top-4 left-4 items-start',
  'top-right': 'top-4 right-4 items-end',
};

/** The pane or popup whose terminal input goes to; '' for one btmux doesn't know. */
function terminalOf(target: EventTarget | null): string | null {
  const element = target instanceof Element ? target.closest('[data-pane-terminal]') : null;
  if (!element) return null;
  for (const [id, term] of useStore.getState().terminals) if (term.element === element) return id;
  return '';
}

function keyInfo(e: KeyboardEvent): KeyInfo {
  return {
    key: e.key,
    code: e.code,
    ctrlKey: e.ctrlKey,
    altKey: e.altKey,
    shiftKey: e.shiftKey,
    metaKey: e.metaKey,
    altGraph: e.getModifierState?.('AltGraph') ?? false,
    isComposing: e.isComposing,
  };
}

/** Pressed keys, and what btmux did with them, over everything (for recordings). */
export function KeyOverlay() {
  const enabled = useStore((s) => s.keyOverlay);
  if (!enabled) return null;
  return <KeyOverlayView />;
}

function KeyOverlayView() {
  const mode = useStore((s) => s.config?.key_overlay_mode ?? 'all');
  const position = useStore((s) => s.config?.key_overlay_position ?? 'bottom');
  const animations = useStore((s) => getAnimations(s.config));
  const entries = useSyncExternalStore(
    (listener) => keyCaster.subscribe(listener),
    () => keyCaster.snapshot(),
  );
  const hostRef = useRef<HTMLElement>(null);

  useEffect(() => {
    keyCaster.mode = mode;
  }, [mode]);

  // Capture on window, ahead of every surface's handlers; `useKeybindings`
  // notes what it made of a key before the key is committed.
  useEffect(() => {
    let timer = 0;
    // Committed once the input reached the terminal's socket.
    const flushSoon = () => {
      clearTimeout(timer);
      timer = window.setTimeout(() => keyCaster.flush());
    };
    let lastKey: { key: string; at: number } | null = null;
    const onKeyDown = (e: KeyboardEvent) => {
      keyCaster.key(e, keyInfo(e), terminalOf(e.target), !e.isTrusted);
      lastKey = { key: e.key, at: e.timeStamp };
      flushSoon();
    };
    const onCompositionEnd = (e: CompositionEvent) => {
      keyCaster.text(e.data, terminalOf(e.target));
      flushSoon();
    };
    // Text without a keydown: soft keyboards, dictation, insertText.
    const onBeforeInput = (e: InputEvent) => {
      if (e.inputType !== 'insertText' || e.isComposing || !e.data) return;
      if (lastKey?.key === e.data && e.timeStamp - lastKey.at < 50) return;
      keyCaster.text(e.data, terminalOf(e.target));
      flushSoon();
    };
    terminalInput.setWatching(true);
    // A prefix ended without a key (timeout, the mobile key bar) shows as canceled.
    const unsubscribe = useStore.subscribe((state, prev) => {
      if (prev.prefixActive && !state.prefixActive) window.setTimeout(() => keyCaster.cancelPrefix());
    });
    window.addEventListener('keydown', onKeyDown, true);
    window.addEventListener('compositionend', onCompositionEnd, true);
    window.addEventListener('beforeinput', onBeforeInput, true);
    return () => {
      clearTimeout(timer);
      unsubscribe();
      window.removeEventListener('keydown', onKeyDown, true);
      window.removeEventListener('compositionend', onCompositionEnd, true);
      window.removeEventListener('beforeinput', onBeforeInput, true);
      terminalInput.setWatching(false);
      keyCaster.clear();
    };
  }, []);

  // Drop entries once shown long enough.
  useEffect(() => {
    // A pending prefix stays until its key, so it sets no deadline.
    const expiring = entries.filter((entry) => !(entry.kind === 'binding' && entry.pending));
    if (expiring.length === 0) return;
    const now = performance.now();
    const next = Math.min(...expiring.map((entry) => entry.at + ENTRY_MS - now));
    const timer = window.setTimeout(() => keyCaster.expire(), Math.max(next, 0) + 10);
    return () => clearTimeout(timer);
  }, [entries]);

  // Modal dialogs are in the top layer; showing the popover again puts it above them.
  useEffect(() => {
    const host = hostRef.current;
    if (!host || typeof host.showPopover !== 'function') return;
    host.setAttribute('popover', 'manual');
    const promote = () => {
      if (host.matches(':popover-open')) host.hidePopover();
      host.showPopover();
    };
    promote();
    const observer = new MutationObserver((records) => {
      const opened = records.some(
        ({ target }) => target instanceof HTMLDialogElement && target.open && target.matches(':modal'),
      );
      if (opened) promote();
    });
    observer.observe(document.body, { subtree: true, attributes: true, attributeFilter: ['open'] });
    return () => {
      observer.disconnect();
      if (host.matches(':popover-open')) host.hidePopover();
    };
  }, []);

  const top = position.startsWith('top');
  return (
    <VStack
      ref={hostRef}
      aria-hidden
      data-key-overlay
      gap={2}
      className={clsx(
        'pointer-events-none fixed inset-auto m-0 overflow-visible border-0 bg-transparent p-0',
        POSITIONS[position],
        top && 'flex-col-reverse',
      )}
    >
      {entries.map((entry) => (
        <EntryView key={`${entry.id}-${entry.at}`} entry={entry} fade={animations} />
      ))}
    </VStack>
  );
}

function EntryView({ entry, fade }: { entry: Entry; fade: boolean }) {
  const pending = entry.kind === 'binding' && entry.pending;
  return (
    <HStack
      gap={2}
      vAlign="center"
      data-key-overlay-entry={entry.kind}
      className={clsx(
        'max-w-[80vw] rounded-lg border border-border bg-popover px-3 py-2 text-xl text-primary shadow-lg',
        fade && !pending && 'btm-key-overlay-fade',
      )}
      style={{ animationDuration: `${ENTRY_MS}ms` }}
    >
      {renderEntry(entry)}
    </HStack>
  );
}

function renderEntry(entry: Entry): ReactNode {
  switch (entry.kind) {
    case 'text':
      return (
        <span className="btm-mono min-w-0 whitespace-pre-wrap break-all">
          {entry.truncated && <span className="text-secondary">…</span>}
          {entry.tokens.map((token, i) =>
            token.named ? (
              <span key={i} className="text-secondary">
                {token.text}
                {token.count > 1 && <Count count={token.count} />}
              </span>
            ) : (
              <span key={i}>{token.text}</span>
            ),
          )}
        </span>
      );
    case 'chord':
      return (
        <>
          <KeyCaps cap={entry.cap} />
          {entry.count > 1 && <Count count={entry.count} />}
        </>
      );
    case 'hidden':
      return (
        <>
          <Lock className="size-5 text-secondary" />
          <span className="text-base text-secondary">
            {entry.reason === 'password' ? 'Password input hidden' : 'Terminal input hidden'}
          </span>
        </>
      );
    case 'binding':
      return (
        <>
          {entry.prefix && <KeyCaps cap={entry.prefix} />}
          {entry.prefix && entry.keys.length === 0 && (
            <span className="text-base text-secondary">{entry.canceled ? 'prefix, no key' : 'prefix…'}</span>
          )}
          {entry.keys.map((cap, i) => (
            <KeyCaps key={i} cap={cap} />
          ))}
          {entry.count > 1 && <Count count={entry.count} />}
          {entry.keys.length > 0 && (
            <span className={clsx('text-base', entry.label ? 'text-primary' : 'text-secondary')}>
              {entry.label ?? 'not bound'}
            </span>
          )}
        </>
      );
  }
}

function KeyCaps({ cap }: { cap: Cap }) {
  return (
    <HStack as="span" gap={1} vAlign="center" className="flex-none">
      {[...cap.mods, cap.key].map((key, i) => (
        <kbd
          key={i}
          className="btm-mono inline-flex h-8 min-w-8 select-none items-center justify-center rounded-md border-b-2 border-b-border-strong bg-neutral px-2 text-lg font-medium text-primary"
        >
          {key}
        </kbd>
      ))}
    </HStack>
  );
}

function Count({ count }: { count: number }) {
  return <span className="text-base text-secondary">×{count}</span>;
}
