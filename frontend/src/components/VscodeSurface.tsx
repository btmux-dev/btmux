import { useEffect, useRef, useState } from 'react';
import { Button } from '@astryxdesign/core/Button';
import { Card } from '@astryxdesign/core/Card';
import { Center } from '@astryxdesign/core/Center';
import { Code } from '@astryxdesign/core/Code';
import { HStack, VStack } from '@astryxdesign/core/Layout';
import { Link } from '@astryxdesign/core/Link';
import { Text } from '@astryxdesign/core/Text';
import { Spinner } from './Spinner';
import type { VscodeStatus } from '../generated/protocol';
import { ClientConfig, LayoutRect } from '../state/types';
import { useStore } from '../state/store';
import { STARTUP_THEME } from '../state/startupTheme';
import { paneRectStyle } from '../state/layout';
import { usePaneMotion } from '../lib/paneMotion';
import { DOUBLE_ESC_MS } from './NvimSurface';
import { isDesktopApp } from '../lib/desktopApp';
import { IS_MAC } from '../lib/nvimKeys';
import type { ParsedKey } from '../lib/prefixKey';
import { prefixKeyMessages } from '../lib/browserInput';
import { findPane, findPaneSession, urlFolder, vscodeUrl, workspaceFolder } from '../lib/vscodeFolder';

/**
 * This page's `@font-face` rules with absolute URLs, so the same-origin iframe
 * can use btmux's bundled terminal fonts (the extension sets VS Code's font).
 */
function fontFaceCss(): string {
  const rules: string[] = [];
  for (const sheet of Array.from(document.styleSheets)) {
    let cssRules: CSSRuleList;
    try {
      cssRules = sheet.cssRules;
    } catch {
      continue; // Cross-origin stylesheet.
    }
    const base = sheet.href ?? document.baseURI;
    for (const rule of Array.from(cssRules)) {
      if (!(rule instanceof CSSFontFaceRule)) continue;
      rules.push(
        rule.cssText.replace(
          /url\((['"]?)([^'")]+)\1\)/g,
          (_, quote: string, url: string) => `url(${quote}${new URL(url, base).href}${quote})`,
        ),
      );
    }
  }
  return rules.join('\n');
}

let activePrefix: ((prefix: ParsedKey) => void) | null = null;

/** Press the prefix key in VS Code (`prefix prefix`), where it has focus. */
export function sendPrefixToVscode(prefix: ParsedKey) {
  activePrefix?.(prefix);
}

/** Sandboxed sessions already warned of, once per tab. */
const warnedSessions = new Set<string>();

// WKWebView rejects VS Code's clipboard read, so its terminal gets the Edit menu's paste,
// which runs only if nothing sees the key.
const isTerminalPaste = (e: KeyboardEvent) =>
  IS_MAC &&
  isDesktopApp() &&
  e.key === 'v' &&
  e.metaKey &&
  !e.ctrlKey &&
  !e.altKey &&
  !e.shiftKey &&
  !!(e.target as Element).classList?.contains('xterm-helper-textarea');

/** VS Code (`/vscode/`, proxied by the backend), pinned over a pane like the Neovim surface. */
export function VscodeSurface({
  config,
  open,
  paneId,
  rect,
  focused,
  zoomed,
  onActivate,
}: {
  config: ClientConfig | null;
  open: boolean;
  paneId: string | null;
  /** The pinned pane's rect, or null while it isn't shown. */
  rect: LayoutRect | null;
  focused: boolean;
  zoomed: boolean;
  onActivate: () => void;
}) {
  const visible = open && rect !== null;
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  usePaneMotion(rootRef, rect);
  const [loaded, setLoaded] = useState(false);
  const shown = useStore((s) => s.vscodeShown);
  // Changing the iframe's src (or moving it in the DOM) reloads VS Code, so
  // only switch folders when shown over a pane in another one: its worktree
  // (not each subfolder), never one on another host.
  const [folder, setFolder] = useState(() => {
    const { allSessions, vscodeFolder } = useStore.getState();
    return workspaceFolder(findPane(allSessions, paneId)) ?? vscodeFolder;
  });
  useEffect(() => {
    if (!open) return;
    const { allSessions, vscodeFolder, setVscodeFolder, showToast } = useStore.getState();
    const session = findPaneSession(allSessions, paneId);
    if (session?.runtime.type === 'multipass' && !warnedSessions.has(session.id)) {
      warnedSessions.add(session.id);
      showToast('VS Code runs on this host, not in the sandbox', 'attention', {
        body: 'Its terminals, tasks, and extensions run with your account on files the sandbox can change. Trust only what you reviewed.',
      });
    }
    const target = workspaceFolder(findPane(allSessions, paneId));
    if (!target || target === vscodeFolder) return;
    setLoaded(false);
    setVscodeFolder(target);
    if (target !== folder) setFolder(target);
    // Already its src: VS Code opened another folder since.
    else iframeRef.current?.contentWindow?.location.replace(vscodeUrl(target));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, paneId, shown]);
  const src = vscodeUrl(folder);
  const focusedRef = useRef(focused);
  focusedRef.current = focused;
  const onActivateRef = useRef(onActivate);
  onActivateRef.current = onActivate;
  const modalOpen = useStore(
    (s) => !!s.overlay || s.settingsOpen || s.fileBrowserOpen || s.switcherOpen || s.windowGridOpen || s.agentGridOpen,
  );
  const theme = config?.theme ?? STARTUP_THEME;

  // Ask the backend to start VS Code before loading it; it may need the user
  // to accept the VS Code Server license first, or not be installed.
  const [status, setStatus] = useState<VscodeStatus | null>(null);
  const [attempt, setAttempt] = useState(0);
  const ready = status?.state === 'ready';
  useEffect(() => {
    if (!open || ready) return;
    let cancelled = false;
    setStatus(null);
    fetch('/api/vscode')
      .then((r) => r.json() as Promise<VscodeStatus>)
      .catch((e: unknown) => ({ state: 'error', message: String(e) }) as const)
      .then((s) => !cancelled && setStatus(s));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, attempt]);
  const acceptLicense = () => {
    setStatus(null);
    fetch('/api/vscode/license', { method: 'POST' })
      .then((r) => r.json() as Promise<VscodeStatus>)
      .catch((e: unknown) => ({ state: 'error', message: String(e) }) as const)
      .then(setStatus);
  };
  const close = () => useStore.getState().setVscodeOpen(false);

  // Key events inside the iframe never reach this document. It is same-origin,
  // so replay each keydown to the global keybinding hook first, and swallow it
  // if a btmux binding handled it. As in Neovim, a quick second Esc hides the
  // surface; the first one still reaches VS Code.
  const attach = () => {
    const win = iframeRef.current?.contentWindow;
    if (!win) return;
    setLoaded(true);
    useStore.getState().setVscodeFolder(urlFolder(win.location.href));
    const fonts = win.document.createElement('style');
    fonts.textContent = fontFaceCss();
    win.document.head.append(fonts);
    // Keys btmux sends VS Code itself, which mustn't be replayed back.
    const forwarded = new WeakSet<Event>();
    activePrefix = (prefix) => {
      const [down] = prefixKeyMessages(prefix);
      if (down.type !== 'key') return;
      const target = win.document.activeElement ?? win.document.body;
      for (const type of ['keydown', 'keyup']) {
        const event = new (win as typeof window).KeyboardEvent(type, {
          key: down.key,
          code: down.code,
          ctrlKey: prefix.ctrl,
          altKey: prefix.alt,
          shiftKey: (down.modifiers & 8) !== 0,
          bubbles: true,
          cancelable: true,
          composed: true,
        });
        // VS Code and xterm.js read the legacy key codes, which the constructor can't set.
        for (const name of ['keyCode', 'which']) Object.defineProperty(event, name, { get: () => down.key_code });
        forwarded.add(event);
        target.dispatchEvent(event);
      }
    };
    let lastEsc = -Infinity;
    const onKeyDown = (e: KeyboardEvent) => {
      if (forwarded.has(e)) return;
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
      if (replay.defaultPrevented) {
        lastEsc = -Infinity;
        e.preventDefault();
        e.stopImmediatePropagation();
        return;
      }
      if (isTerminalPaste(e)) {
        lastEsc = -Infinity;
        e.stopImmediatePropagation();
        return;
      }
      if (e.key !== 'Escape' || e.repeat || e.ctrlKey || e.altKey || e.shiftKey || e.metaKey) {
        lastEsc = -Infinity;
      } else if (e.timeStamp - lastEsc < DOUBLE_ESC_MS) {
        lastEsc = -Infinity;
        e.preventDefault();
        e.stopImmediatePropagation();
        close();
      } else lastEsc = e.timeStamp;
    };
    const onMouseDown = () => {
      if (!focusedRef.current) onActivateRef.current();
    };
    win.addEventListener('keydown', onKeyDown, true);
    win.addEventListener(
      'keypress',
      (e) => {
        if (isTerminalPaste(e)) e.stopImmediatePropagation();
      },
      true,
    );
    win.addEventListener('mousedown', onMouseDown, true);
  };

  useEffect(
    () => () => {
      activePrefix = null;
    },
    [],
  );

  // Focus moving into VS Code, including its webviews (other origins, whose
  // events never reach this page or VS Code's), selects its pane.
  useEffect(() => {
    if (!visible) return;
    const onBlur = () => {
      if (document.activeElement === iframeRef.current && !focusedRef.current) onActivateRef.current();
    };
    window.addEventListener('blur', onBlur);
    return () => window.removeEventListener('blur', onBlur);
  }, [visible]);

  // Take focus when its pane is active, and back after a modal closes.
  useEffect(() => {
    if (!focused || modalOpen || !visible) return;
    const id = window.setTimeout(() => iframeRef.current?.focus(), 0);
    return () => window.clearTimeout(id);
  }, [focused, modalOpen, visible, loaded]);

  return (
    <div
      ref={rootRef}
      inert={!visible}
      aria-hidden={!visible}
      style={{
        position: 'absolute',
        ...(rect ? paneRectStyle(rect) : { inset: 0 }),
        // Mirrors TerminalPane's border.
        border: `1px solid ${zoomed ? theme.magenta : focused ? theme.blue : theme.selectionBackground}`,
        borderRadius: 'var(--radius-container)',
        zIndex: 30,
        overflow: 'hidden',
        visibility: visible ? undefined : 'hidden',
        background: theme.background,
      }}
    >
      {ready && (
        <iframe
          ref={iframeRef}
          src={src}
          title="VS Code"
          onLoad={attach}
          allow="clipboard-read; clipboard-write"
          style={{ display: 'block', width: '100%', height: '100%', border: 0 }}
        />
      )}
      {!loaded && (
        <Center className="absolute inset-0 bg-body/90">
          {status?.state === 'license' ? (
            <Card padding={4} elevation="high" maxWidth={480}>
              <VStack gap={3} hAlign="start">
                <VStack gap={1}>
                  <Text weight="semibold">Visual Studio Code Server license</Text>
                  <Text color="secondary">
                    btmux runs VS Code with <Code>code serve-web</Code>. By using it, you agree to the{' '}
                    <Link href="https://aka.ms/vscode-server-license" isExternalLink>
                      Visual Studio Code Server License Terms
                    </Link>{' '}
                    and the{' '}
                    <Link href="https://privacy.microsoft.com/en-US/privacystatement" isExternalLink>
                      Microsoft Privacy Statement
                    </Link>
                    .
                  </Text>
                </VStack>
                <HStack gap={2} vAlign="center">
                  <Button label="Accept" variant="primary" onClick={acceptLicense} />
                  <Button label="Cancel" onClick={close} />
                </HStack>
              </VStack>
            </Card>
          ) : status?.state === 'error' ? (
            <Card padding={4} elevation="high" maxWidth={560}>
              <VStack gap={3} hAlign="start">
                <VStack gap={1}>
                  <Text weight="semibold">VS Code didn't start</Text>
                  <Text color="secondary" className="whitespace-pre-wrap">
                    {status.message}
                  </Text>
                </VStack>
                <HStack gap={2} vAlign="center">
                  <Button label="Retry" variant="primary" onClick={() => setAttempt((a) => a + 1)} />
                  <Button label="Close" onClick={close} />
                </HStack>
              </VStack>
            </Card>
          ) : (
            <Spinner size="xl" label="Starting VS Code…" />
          )}
        </Center>
      )}
    </div>
  );
}
