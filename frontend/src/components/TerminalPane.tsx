import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { PaneTerminal, PaneTerminalOptions } from '../lib/paneTerminal';
import { usePaneTerminal } from '../hooks/usePaneTerminal';
import { editorShown, useStore } from '../state/store';
import { AgentStatus, LayoutRect, ClientConfig } from '../state/types';
import { ClientMessage } from '../protocol/messages';
import { STARTUP_THEME } from '../state/startupTheme';
import { paneRectStyle } from '../state/layout';
import { usePaneMotion } from '../lib/paneMotion';
import { PaneCorner, PaneTitleBar } from './PaneTitleBar';
import { mix, withAlpha } from '../lib/chrome-colors';
import { findPaneBorderStyle } from '../lib/paneSwitchBorder';
import { PaneBorderDraw } from './PaneBorderDraw';
import { useLatexScan, type PaneLatexMatch } from '../lib/latexScan';
import { LatexOverlay } from './LatexOverlay';
import { PaneWidget } from './PaneWidget';
import { PaneSearch } from './PaneSearch';
import { CONNECTION_STATE_LABEL } from '../lib/connectionState';
import { clsx } from 'clsx';
import { Button } from '@astryxdesign/core/Button';
import { desktopTransparency } from '../lib/desktopTransparency';
import { hasShaderWallpaper } from '../lib/wallpaperCatalog';
import {
  CONFIG_DEFAULTS,
  getAnimations,
  getPaneSwitchBorderSpeed,
  getPaneSwitchBorderStyle,
  getShowPaneTitles,
  getTerminalFontFamily,
  getTerminalFontSize,
} from '../state/configDefaults';

interface Props {
  sessionId: string;
  paneId: string;
  rect: LayoutRect;
  isActive: boolean;
  /** Pane title (OSC 0/2), shown in the title bar. */
  title?: string | null;
  /** Pane working directory (OSC 7), shown in the title bar. */
  cwd?: string | null;
  /** Server-authoritative semantic status for an agent in this pane. */
  agentStatus?: AgentStatus;
  /** `githubKey` of the pane's repo and branch, if on one. */
  githubKey?: string;
  /** 0-based pane index within its window (layout order), shown as the badge. */
  paneIndex?: number;
  /**
   * Whether this pane is shown right now — i.e. it's in the active window of the
   * active session. Panes of inactive windows (or of pooled-but-inactive
   * sessions) stay mounted (the keep-alive pool) but are hidden with
   * `display:none` and paused so switches don't tear down and rebuild
   * emulators. See SessionPane for the pool rationale.
   */
  visible: boolean;
  /** True when this pane is the sole, fullscreen-zoomed pane of its window. */
  isZoomed?: boolean;
  /** Draft config applied locally while the settings overlay is open. */
  previewConfig?: ClientConfig | null;
  /**
   * Shared paneId→Terminal map owned by SessionPane. The pane registers its
   * Terminal here so SessionPane can authoritatively focus the active pane after
   * a window switch (see the focus race note in SessionPane).
   */
  registry?: Map<string, PaneTerminal>;
  send: (msg: ClientMessage) => void;
}

const FONT_FALLBACK = 'Symbols Nerd Font Mono, Menlo, Monaco, monospace';

export function buildFontFamily(configured: string): string {
  return `${configured}, ${FONT_FALLBACK}`;
}

export function buildTerminalOptions(config: ClientConfig | null): PaneTerminalOptions | null {
  if (!config) return null;
  const t = config.terminal;
  return {
    fontSize: getTerminalFontSize(config),
    fontFamily: buildFontFamily(getTerminalFontFamily(config)),
    fontWeight: t?.fontWeight ?? CONFIG_DEFAULTS.terminal.fontWeight,
    theme: config?.theme ?? STARTUP_THEME,
    cursorBlink: t?.cursorBlink ?? CONFIG_DEFAULTS.terminal.cursorBlink,
    scrollback: t?.scrollback ?? CONFIG_DEFAULTS.terminal.scrollback,
    disableStdin: t?.disableStdin ?? undefined,
  };
}

export function terminalTransparency(config: ClientConfig | null): boolean {
  if (!config) return false;
  // Let wallpapers and the desktop backdrop show through the terminal.
  const hasWallpaper = config.wallpaper != null || hasShaderWallpaper(config.wallpaper_shaders);
  return config.terminal?.allowTransparency ?? (hasWallpaper || desktopTransparency);
}

// Only construction options belong in the emulator lifetime key.
export function useTerminalOptions(config: ClientConfig | null) {
  const options = buildTerminalOptions(config);
  const key = JSON.stringify(options ? { ...options, theme: undefined } : null);
  return useMemo(
    () => options,
    // The theme is applied in place; only the key rebuilds.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [key],
  );
}

export function TerminalPane({
  sessionId,
  paneId,
  rect,
  isActive,
  title,
  cwd,
  agentStatus,
  githubKey,
  paneIndex,
  visible,
  isZoomed = false,
  previewConfig = null,
  registry,
  send,
}: Props) {
  const outerRef = useRef<HTMLDivElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  // Layout changes animate the box; the terminal keeps its final size.
  usePaneMotion(outerRef, rect, containerRef);
  const visibleRef = useRef(visible);
  visibleRef.current = visible;

  const storeConfig = useStore((s) => s.config);
  const config = previewConfig ?? storeConfig;
  const overlay = useStore((s) => s.overlay);
  const windowGridOpen = useStore((s) => s.windowGridOpen);
  const agentGridOpen = useStore((s) => s.agentGridOpen);
  const switcherOpen = useStore((s) => s.switcherOpen);
  const settingsOpen = useStore((s) => s.settingsOpen);
  // A popup over this pane's window owns the keyboard.
  const popupShown = useStore((s) => s.popupShown !== null);
  // Whether an editor or browser surface covers this pane.
  const nvimOpen = useStore(
    (s) =>
      (s.nvimOpen && s.nvimPaneId === paneId) ||
      (s.vscodeOpen && s.vscodePaneId === paneId) ||
      (s.browserOpen && s.browserPaneId === paneId) ||
      editorShown(s, paneId),
  );
  const fileBrowserOpen = useStore((s) => s.fileBrowserOpen && s.fileBrowserPaneId === paneId);
  const termOptions = useTerminalOptions(config);
  // Set below; the socket handler calls it after each write to rescan for LaTeX.
  const latexPokeRef = useRef<() => void>(() => {});
  const { term, termRef, fitRef, claimRef, clipped, dims, connection, connectionError, initialReplayRendered } =
    usePaneTerminal({
      containerRef,
      paneId,
      termOptions,
      visibleRef,
      registry,
      onWriteRef: latexPokeRef,
    });

  // LaTeX detection (lib/latexDetect.ts).
  // A pane under the built-in Neovim keeps its geometry but needn't render.
  const rendering = visible && !nvimOpen;
  const { matches: latexMatches, poke: latexPoke } = useLatexScan(term, rendering);
  latexPokeRef.current = latexPoke;
  const latexOpen = useStore((s) => s.latexPanes.has(paneId));
  const github = useStore((s) => (githubKey ? s.github.get(githubKey) : undefined));
  const searchOpen = useStore((s) => s.searchPaneId === paneId);
  const widget = useStore((s) => s.widgets.find((widget) => widget.pane_id === paneId));
  const widgetId = widget?.id ?? null;
  const toggleLatex = () => useStore.getState().toggleLatex(paneId);
  const highlightLatex = (match: PaneLatexMatch | null) => {
    if (!term) return;
    if (!match) return term.clearDecorations();
    const background = withAlpha(config?.theme?.yellow ?? STARTUP_THEME.yellow, 0.35);
    term.setDecorations(match.cells.map((cell) => ({ ...cell, background })));
  };
  // Highlights hold absolute buffer lines, which go stale on any rescan.
  useEffect(() => {
    term?.clearDecorations();
  }, [term, latexOpen, latexMatches]);

  // Park / wake the terminal as it leaves / enters the active window. Hidden
  // panes stay mounted (their socket keeps streaming into the WASM terminal) but
  // pausing stops painting so they cost ~0 CPU. On the way back in we
  // resume and fit: the container regained its real size, and if the pane was
  // first created while hidden it may still be unconnected — fit() drives the
  // ResizeObserver, which then opens the socket at the correct cols/rows.
  useEffect(() => {
    if (!term) return;
    term.setPaused(!rendering);
    if (rendering) fitRef.current?.();
  }, [rendering, term]);

  const allowTransparency = terminalTransparency(config);
  useEffect(() => {
    term?.setTransparent(allowTransparency);
  }, [allowTransparency, term]);

  // One-shot border-draw effect on the pane you switch to (`pane-switch-border`,
  // style-selectable, `wipe` by default; null = disabled). Pure CSS, keyed
  // on this nonce so the element remounts and replays on every activation:
  // pane-to-pane nav, and a window/session switch (where the newly active pane
  // goes visible+active in one update). The nonce doubles as a mounted flag — bumped on activation,
  // cleared once the draw+fade has run so an idle pane carries no leftover node.
  const paneSwitchBorderStyleId = getPaneSwitchBorderStyle(config);
  const paneSwitchBorderStyle = paneSwitchBorderStyleId ? findPaneBorderStyle(paneSwitchBorderStyleId) : null;
  const paneSwitchBorderSpeed = getPaneSwitchBorderSpeed(config);
  const [borderTraceNonce, setBorderTraceNonce] = useState(0);
  const prevIsActiveForBorderTrace = useRef(isActive);
  // Layout effect so the ring is hidden before the first paint of an activation.
  useLayoutEffect(() => {
    const wasActive = prevIsActiveForBorderTrace.current;
    prevIsActiveForBorderTrace.current = isActive;
    if (wasActive || !isActive || !visible) return;
    if (!paneSwitchBorderStyle || !getAnimations(config)) return;
    setBorderTraceNonce((nonce) => nonce + 1);
    const done = setTimeout(() => setBorderTraceNonce(0), (paneSwitchBorderSpeed + 0.35) * 1000);
    return () => clearTimeout(done);
  }, [isActive, visible, config?.animations, paneSwitchBorderStyleId, paneSwitchBorderSpeed]);

  // Themes apply in place.
  useEffect(() => {
    term?.setTheme(config?.theme ?? STARTUP_THEME);
  }, [config?.theme, term]);
  useEffect(() => {
    term?.setCursorHidden(!isActive);
  }, [isActive, term]);

  // Focus when this pane becomes active (false→true transition, including mount),
  // when its initial replay finishes, or when an overlay closes while this pane
  // is active. Blur when it is inactive, not yet ready for keyboard input, or a
  // modal surface or pane search owns the keyboard.
  // Tracking the previous value prevents the old active pane from re-stealing
  // focus while a select_pane command is still in flight to the server.
  const prevIsActive = useRef(false);
  const prevOverlay = useRef(overlay);
  const prevFileBrowser = useRef(fileBrowserOpen);
  const prevPopupShown = useRef(popupShown);
  const prevNvimOpen = useRef(nvimOpen);
  const prevSearchOpen = useRef(searchOpen);
  const prevWidgetId = useRef(widgetId);
  const prevInitialReplayRendered = useRef(false);
  useEffect(() => {
    const wasActive = prevIsActive.current;
    const hadOverlay = prevOverlay.current;
    const hadFileBrowser = prevFileBrowser.current;
    const hadPopup = prevPopupShown.current;
    const hadNvimOpen = prevNvimOpen.current;
    const widgetChanged = prevWidgetId.current !== widgetId;
    const hadSearchOpen = prevSearchOpen.current;
    const hadRenderedInitialReplay = prevInitialReplayRendered.current;
    prevIsActive.current = isActive;
    prevOverlay.current = overlay;
    prevFileBrowser.current = fileBrowserOpen;
    prevPopupShown.current = popupShown;
    prevNvimOpen.current = nvimOpen;
    prevSearchOpen.current = searchOpen;
    prevWidgetId.current = widgetId;
    prevInitialReplayRendered.current = initialReplayRendered;

    const keyboardOwnedElsewhere =
      settingsOpen ||
      overlay ||
      windowGridOpen ||
      agentGridOpen ||
      switcherOpen ||
      fileBrowserOpen ||
      popupShown ||
      nvimOpen ||
      searchOpen;

    if (!isActive || keyboardOwnedElsewhere || !initialReplayRendered) {
      termRef.current?.blur();
      return;
    }

    const becameActive = !wasActive;
    const replayFinished = !hadRenderedInitialReplay;
    const overlayClosed =
      (!overlay && !!hadOverlay) ||
      (!fileBrowserOpen && hadFileBrowser) ||
      (!popupShown && hadPopup) ||
      (!nvimOpen && hadNvimOpen) ||
      (!searchOpen && hadSearchOpen);
    if (becameActive || replayFinished || overlayClosed || widgetChanged) {
      termRef.current?.focus();
    }
  }, [
    isActive,
    settingsOpen,
    overlay,
    windowGridOpen,
    agentGridOpen,
    switcherOpen,
    fileBrowserOpen,
    popupShown,
    nvimOpen,
    searchOpen,
    widgetId,
    initialReplayRendered,
  ]);

  // When the user clicks this pane, tell the backend to make it active so the
  // border and server-side state stay in sync with DOM focus.
  const onMouseDown = () => {
    if (!isActive) send({ type: 'select_pane', session_id: sessionId, pane_id: paneId });
  };

  const borderActive = config?.theme?.blue ?? STARTUP_THEME.blue;
  const borderInactive = config?.theme?.selectionBackground ?? STARTUP_THEME.selectionBackground;
  const borderZoomed = config?.theme?.magenta ?? STARTUP_THEME.magenta;
  const borderColor = isZoomed ? borderZoomed : isActive ? borderActive : borderInactive;
  // Near-white leading edge for the `sweep` style.
  const borderEdgeColor = mix(borderColor, '#ffffff', 0.7);
  const animations = getAnimations(config);
  const showTitle = getShowPaneTitles(config);
  const termFont = getTerminalFontSize(config);
  const notification = useStore(
    (s) =>
      s.allSessions
        .find((session) => session.id === sessionId)
        ?.windows.flatMap((win) => win.panes)
        .find((pane) => pane.id === paneId)?.unread?.level ?? null,
  );

  const accentGlow = withAlpha(borderColor, 0.2);
  const borderPlaying = borderTraceNonce > 0 && animations && paneSwitchBorderStyle !== null;

  return (
    <div
      ref={outerRef}
      data-pane-id={paneId}
      onMouseDown={onMouseDown}
      style={{
        // Panes of inactive windows stay mounted but hidden (the keep-alive
        // pool). display:none detaches them from layout so they don't paint or
        // intercept clicks; the pause effect stops their painting.
        display: visible ? 'flex' : 'none',
        // Preserve terminal geometry and sockets beneath the transparent editor.
        opacity: nvimOpen ? 0 : undefined,
        flexDirection: 'column',
        position: 'absolute',
        ...paneRectStyle(rect),
        border: `1px solid ${isActive || isZoomed ? 'transparent' : borderColor}`,
        borderRadius: 'var(--radius-container)',
        overflow: 'hidden',
        caretColor: 'transparent',
        // A zoomed pane fills the grid and must paint over the panes it covers
        // (which stay mounted). Above dividers' zIndex of 10.
        zIndex: isZoomed ? 20 : undefined,
        transition: animations ? 'border-color .15s ease, background .15s ease' : undefined,
      }}
    >
      {connection !== 'connected' && (
        <div
          role="status"
          className={clsx(
            'absolute inset-0 flex items-center justify-center text-sm text-secondary',
            animations && 'animate-pulse',
          )}
          style={{ zIndex: 30 }}
        >
          {connectionError ?? CONNECTION_STATE_LABEL[connection]}
        </div>
      )}
      {showTitle && (
        <PaneTitleBar
          index={paneIndex ?? 0}
          title={title}
          cwd={cwd}
          agentStatus={agentStatus}
          github={github}
          cols={dims?.cols ?? null}
          rows={dims?.rows ?? null}
          isActive={isActive}
          notification={notification}
          latexCount={latexMatches.length}
          latexOpen={latexOpen}
          onToggleLatex={toggleLatex}
        />
      )}
      {!showTitle && (
        <PaneCorner
          index={paneIndex ?? 0}
          status={agentStatus}
          github={github}
          isActive={isActive}
          latexCount={latexOpen ? 0 : latexMatches.length}
          onToggleLatex={toggleLatex}
        />
      )}
      {/* Focus ring — only rendered on the active/zoomed pane so mounting it
          replays btm-bloom on every focus change without needing a key trick. */}
      {(isActive || isZoomed) && (
        <div
          style={{
            position: 'absolute',
            inset: 0,
            border: `1.5px solid ${borderColor}`,
            // Concentric inside the pane's 1px border.
            borderRadius: 'max(0px, calc(var(--radius-container) - 1px))',
            boxShadow: `0 0 0 1px ${borderColor}, 0 0 26px ${accentGlow}`,
            pointerEvents: 'none',
            animation: animations ? 'btm-bloom .3s cubic-bezier(.2,.8,.2,1)' : undefined,
            visibility: borderPlaying ? 'hidden' : undefined,
            zIndex: 1,
          }}
        />
      )}
      {/* One-shot pane-switch border draw. Keyed on the nonce so React remounts
          it (restarting its CSS animation) on each activation. It draws the
          focus ring itself, which stays hidden until the draw unmounts. */}
      {borderPlaying && (isActive || isZoomed) && (
        <PaneBorderDraw
          key={borderTraceNonce}
          className={paneSwitchBorderStyle.className}
          color={borderColor}
          edge={borderEdgeColor}
          seconds={paneSwitchBorderSpeed}
        />
      )}
      {/* The terminal fills the space below the (optional) title bar. inset-style
          padding around it keeps a small gutter so glyphs don't touch the border. */}
      <div
        style={{
          position: 'relative',
          flex: 1,
          minHeight: 0,
          // Cover wallpaper/transparency only while the terminal canvas is
          // hidden; once revealed, restore the pane's normal transparency.
          background: initialReplayRendered ? 'transparent' : (config?.theme?.background ?? STARTUP_THEME.background),
        }}
      >
        <div
          ref={containerRef}
          className="absolute inset-3"
          style={{
            // `visibility` preserves layout, so the ResizeObserver can
            // establish the correct grid while replay remains off-screen.
            visibility: initialReplayRendered ? 'visible' : 'hidden',
          }}
        />
        {clipped && rendering && initialReplayRendered && (
          // Another viewer sized the PTY larger than this pane, so it's clipped.
          <div className="absolute bottom-3 left-1/2 -translate-x-1/2" style={{ zIndex: 25 }}>
            <Button
              label="Fit to this screen"
              size="sm"
              elevation="med"
              tooltip="Another viewer set this pane's size"
              onClick={() => claimRef.current?.()}
            />
          </div>
        )}
        {widget && term && initialReplayRendered && (
          // Under a popup it can't be used, so callers fall back unless it's shown later.
          <PaneWidget
            key={widget.id}
            id={widget.id}
            kind={widget.kind}
            term={term}
            shown={rendering && !popupShown}
            send={send}
          />
        )}
        {searchOpen && term && (
          <PaneSearch
            term={term}
            theme={config?.theme ?? null}
            onClose={() => useStore.getState().setSearchPaneId(null)}
          />
        )}
        {latexOpen && (
          <LatexOverlay
            theme={config?.theme ?? null}
            termFont={termFont}
            matches={latexMatches}
            onHover={highlightLatex}
            onClose={toggleLatex}
          />
        )}
      </div>
    </div>
  );
}
