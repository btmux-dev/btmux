import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { PaneTerminal as Terminal } from '../lib/paneTerminal';
import { Dialog } from '@astryxdesign/core/Dialog';
import { TerminalPane } from './TerminalPane';
import { NativeEditor } from './NativeEditor';
import { FileBrowserOverlay } from './FileBrowserOverlay';
import { PopupDialog } from './PopupDialog';
import { wideDialogProps } from '../lib/wideDialog';
import { ProcessOverlay } from './ProcessOverlay';
import { ClientConfig, LayoutNode, LayoutRect } from '../state/types';
import {
  computeRectsAndDividers,
  dividerStyle,
  layoutPercent,
  PANE_GAP,
  paneIdsInOrder,
  paneRectStyle,
  splitRatio,
  collapsedRect,
  sameBounds,
  type Bounds,
  Divider,
} from '../state/layout';
import { animateGhost } from '../lib/paneMotion';
import { ClientMessage } from '../protocol/messages';
import { githubKey, useStore } from '../state/store';
import { useFileStore } from '../state/fileStore';
import { STARTUP_THEME } from '../state/startupTheme';
import { recordWindowMruVisit } from '../state/windowMru';
import { useWindowAttended } from '../lib/windowAttention';
import { NARROW_QUERY } from '../lib/touch';
import { DIVIDER_TOUCH_TARGET, trackPointerDrag } from '../hooks/useSidebarResize';
import { useMediaQuery } from '@astryxdesign/core/hooks';

interface Props {
  sessionId: string;
  /**
   * Whether this session is the one currently shown. SessionPane renders a
   * TerminalPane for *every* pane across *all* windows of its session (the
   * keep-alive pool) and shows them only while this is the active session *and*
   * the pane is in its active window; everything else stays mounted-but-hidden
   * so switches don't tear down and rebuild emulators.
   *
   * SessionPool mounts one SessionPane per pooled session (keyed by sessionId)
   * and keeps it mounted across session switches, so the keep-alive pool now
   * spans the last-N sessions, not just one. Reading the session's own state
   * from the store (rather than via props) lets a pooled-but-inactive session
   * keep re-rendering as the server pushes window/pane changes.
   */
  isActiveSession: boolean;
  /** Draft config applied locally to this active session while settings are open. */
  previewConfig: ClientConfig | null;
  send: (msg: ClientMessage) => void;
}

export function SessionPane({ sessionId, isActiveSession, previewConfig, send }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [ratioOverrides, setRatioOverrides] = useState<Map<string, number>>(new Map());
  // Ratios sent with `resize_split`, kept as overrides until the server's
  // layout has them, so a released divider doesn't jump back for a round trip.
  const committedRef = useRef(new Map<string, number>());
  // The shown window's layout and rects as last committed, to animate from.
  const shownRef = useRef<{
    windowId: string;
    layoutKey: string;
    layout: LayoutNode;
    rects: Map<string, LayoutRect>;
  } | null>(null);
  const [ghosts, setGhosts] = useState<{ key: string; from: Bounds; to: Bounds }[]>([]);
  const dropOverride = useCallback((id: string) => {
    committedRef.current.delete(id);
    setRatioOverrides((prev) => {
      if (!prev.has(id)) return prev;
      const next = new Map(prev);
      next.delete(id);
      return next;
    });
  }, []);
  const overlay = useStore((s) => s.overlay);
  const windowGridOpen = useStore((s) => s.windowGridOpen);
  const agentGridOpen = useStore((s) => s.agentGridOpen);
  const switcherOpen = useStore((s) => s.switcherOpen);
  const settingsOpen = useStore((s) => s.settingsOpen);
  const paneNumbersVisible = useStore((s) => s.paneNumbersVisible);
  const fileBrowserOpen = useStore((s) => s.fileBrowserOpen);
  const nvimPaneId = useStore((s) => (s.nvimOpen ? s.nvimPaneId : null));
  const vscodePaneId = useStore((s) => (s.vscodeOpen ? s.vscodePaneId : null));
  const browserPaneId = useStore((s) => (s.browserOpen ? s.browserPaneId : null));
  const fileBrowserPaneId = useStore((s) => s.fileBrowserPaneId);
  const fileBrowserCwd = useStore((s) => s.fileBrowserCwd);
  const fileBrowserMode = useStore((s) => s.fileBrowserInitialMode);
  const storeConfig = useStore((s) => s.config);
  const config = previewConfig ?? storeConfig;

  // Read this session's own state from the store. Subscribing to allSessions
  // (rather than receiving windows via props) means a pooled-but-inactive
  // session keeps re-rendering as the server pushes window/pane changes, so its
  // pool stays current and a switch-back shows up-to-date layout immediately.
  const session = useStore((s) => s.allSessions.find((sess) => sess.id === sessionId));
  const windows = session?.windows ?? [];
  const activeWindow = session ? windows[session.active_window] : undefined;
  const activePaneId = activeWindow?.panes[activeWindow.active_pane]?.id ?? null;
  // Narrow screens show only the active pane, as if zoomed (locally).
  const narrow = useMediaQuery(NARROW_QUERY);
  const previewFullscreen = useFileStore((s) => s.previewFullscreen);
  const zoomedPaneId =
    activeWindow?.zoomed_pane ?? (narrow && (activeWindow?.panes.length ?? 0) > 1 ? activePaneId : null);
  const focusedPane = activeWindow?.panes.find((pane) => pane.id === (zoomedPaneId ?? activePaneId));
  // A popup over the active pane, else the newest over another pane of the window.
  const popups = useStore((s) => s.popups);
  const shownPopup = isActiveSession
    ? (popups.find((popup) => popup.pane_id === activePaneId) ??
      [...popups].reverse().find((popup) => activeWindow?.panes.some((pane) => pane.id === popup.pane_id)))
    : undefined;
  const popupOpen = shownPopup !== undefined;
  const focusedAgentState = focusedPane?.agent_status.state ?? null;
  const focusedUnread = focusedPane?.unread != null;
  const attended = useWindowAttended();

  // Lazy-but-sticky pool membership. We don't mount *every* window's panes the
  // instant a session is entered — that would just move the mount-cost burst to
  // session entry. Instead a window joins the pool the first time it becomes
  // active and then stays (its panes keep streaming, paused, off-screen).
  // So the first switch to a never-seen window costs the same as today; every
  // switch back is instant. Keyed by window id (stable across reorders).
  const [mountedWindowIds, setMountedWindowIds] = useState<Set<string>>(
    () => new Set(activeWindow ? [activeWindow.id] : []),
  );
  useEffect(() => {
    if (!activeWindow || mountedWindowIds.has(activeWindow.id)) return;
    setMountedWindowIds((prev) => new Set(prev).add(activeWindow.id));
  }, [activeWindow?.id]);

  // Record window-level MRU as the active window changes, but only for the shown
  // session — a pooled background session keeps re-rendering on server pushes and
  // must not reorder the grid. This is the one place "the active window of the
  // active session" is observed, so it's the natural recording point.
  useEffect(() => {
    if (isActiveSession && activeWindow) recordWindowMruVisit(activeWindow.id);
  }, [isActiveSession, activeWindow?.id]);

  // Shared map of every mounted pane's Terminal. Each TerminalPane registers
  // itself here on mount and removes itself on unmount.
  const registryRef = useRef<Map<string, Terminal>>(new Map());

  // Authoritatively focus the active pane after the active pane / layout
  // changes, after all child TerminalPane effects (splits, pane navigation,
  // window switches). A pane that is still opening focuses itself once its
  // replay is shown (TerminalPane's focus effect).
  // While a pane is zoomed it is the one shown and must hold focus, even though
  // the covered panes stay mounted — focus it instead of the active pane.
  // Gated on isActiveSession: with the pool now spanning several sessions, N
  // SessionPanes coexist and only the visible one may grab keyboard focus —
  // otherwise a background session's panes would steal it on every server push.
  // Switching sessions flips isActiveSession→true here, re-running this effect
  // so the newly-shown session re-asserts focus.
  // A config update also rebuilds every Terminal (see TerminalPane's termOptions
  // effect), so re-run for it even when activePaneId is unchanged.
  // While a modal is open it owns the keyboard, so don't focus a pane. The
  // window grid, agent grid, and switcher dialogs stay mounted and close in
  // their own effect, which keeps the pane inert here and then restores focus
  // to <body> (TerminalPane blurred before they opened), so focus after it.
  const dialogOpen = windowGridOpen || agentGridOpen || switcherOpen;
  const prevDialogOpen = useRef(dialogOpen);
  useEffect(() => {
    const dialogClosed = prevDialogOpen.current && !dialogOpen;
    prevDialogOpen.current = dialogOpen;
    const focusId = zoomedPaneId ?? activePaneId;
    // Don't steal focus into the terminal while the file browser occupies that pane.
    const browserOwnsActivePane = fileBrowserOpen && fileBrowserPaneId === focusId;
    // Neovim takes focus itself when it covers the active pane.
    const nvimOwnsActivePane =
      nvimPaneId === focusId ||
      vscodePaneId === focusId ||
      browserPaneId === focusId ||
      !!activeWindow?.panes.find((p) => p.id === focusId)?.editor.shown;
    if (
      settingsOpen ||
      overlay ||
      windowGridOpen ||
      agentGridOpen ||
      switcherOpen ||
      browserOwnsActivePane ||
      nvimOwnsActivePane ||
      popupOpen ||
      !focusId ||
      !isActiveSession
    ) {
      return;
    }
    // A hidden or unfocused tab must not mark the pane as seen.
    if (attended && (focusedUnread || focusedAgentState === 'done')) {
      send({ type: 'acknowledge_pane', pane_id: focusId });
    }
    registryRef.current.get(focusId)?.focus();
    if (dialogClosed) {
      const id = window.setTimeout(() => registryRef.current.get(focusId)?.focus(), 0);
      return () => window.clearTimeout(id);
    }
  }, [
    activePaneId,
    zoomedPaneId,
    overlay,
    windowGridOpen,
    agentGridOpen,
    switcherOpen,
    fileBrowserOpen,
    fileBrowserPaneId,
    nvimPaneId,
    vscodePaneId,
    browserPaneId,
    focusedPane?.editor.shown,
    isActiveSession,
    settingsOpen,
    previewConfig,
    focusedAgentState,
    focusedUnread,
    attended,
    send,
    config,
    popupOpen,
  ]);

  const handleDividerPointerDown = useCallback(
    (divider: Divider, e: React.PointerEvent) => {
      e.stopPropagation();

      const container = containerRef.current;
      if (!container) return;

      const containerRect = container.getBoundingClientRect();
      const isVertical = divider.orientation === 'vertical';
      // The root's `column-gap` resolves PANE_GAP to px.
      const gap = parseFloat(getComputedStyle(container).columnGap) || 0;

      const computeRatio = (clientX: number, clientY: number): number => {
        const containerSize = isVertical ? containerRect.width : containerRect.height;
        const mousePos = isVertical ? clientX - containerRect.left : clientY - containerRect.top;
        const mousePct = layoutPercent(mousePos, containerSize, gap);
        const ratio = (mousePct - divider.boundsStart) / divider.boundsSize;
        return Math.max(0.05, Math.min(0.95, ratio));
      };

      const onMove = (moveEvent: PointerEvent) => {
        const ratio = computeRatio(moveEvent.clientX, moveEvent.clientY);
        setRatioOverrides((prev) => new Map(prev).set(divider.id, ratio));
      };

      committedRef.current.delete(divider.id);
      useStore.getState().setDividerDragging(true);
      trackPointerDrag(e, onMove, (upEvent) => {
        // A cancelled touch (a system gesture) keeps the old layout.
        if (upEvent.type === 'pointerup') {
          const ratio = computeRatio(upEvent.clientX, upEvent.clientY);
          committedRef.current.set(divider.id, ratio);
          setRatioOverrides((prev) => new Map(prev).set(divider.id, ratio));
          send({ type: 'resize_split', session_id: sessionId, split_id: divider.id, ratio });
          // Without the server's reply, fall back to its layout.
          window.setTimeout(() => {
            if (committedRef.current.get(divider.id) === ratio) dropOverride(divider.id);
          }, 2000);
        } else dropOverride(divider.id);
        // Terminals fit once, to the released layout.
        useStore.getState().setDividerDragging(false);
      });
    },
    [sessionId, send, dropOverride],
  );

  const layout = activeWindow?.layout;
  useEffect(() => {
    for (const [id, ratio] of committedRef.current) {
      const stored = layout ? splitRatio(layout, id) : null;
      // f32 on the server.
      if (stored === null || Math.abs(stored - ratio) < 1e-4) dropOverride(id);
    }
  }, [layout, dropOverride]);

  // Lay out only the active window. Panes of other windows are kept mounted but
  // hidden, so they need no rect of their own.
  const layoutRects = activeWindow
    ? computeRectsAndDividers(activeWindow.layout, { top: 0, left: 0, width: 100, height: 100 }, ratioOverrides)
    : { rects: [], dividers: [] };
  const { dividers } = layoutRects;

  // When the shown window's layout changes (a split, a close, a layout cycle),
  // each pane's box animates from where it was; new panes grow from the outer
  // edge of their split. Window switches, drags, and zoom don't animate.
  const layoutKey = activeWindow ? JSON.stringify(activeWindow.layout) : '';
  const prevShown = shownRef.current;
  const moved =
    isActiveSession &&
    !!activeWindow &&
    !zoomedPaneId &&
    prevShown !== null &&
    prevShown.windowId === activeWindow.id &&
    prevShown.layoutKey !== layoutKey;
  const rects: LayoutRect[] = moved
    ? layoutRects.rects.map((rect) => {
        const from = prevShown.rects.get(rect.paneId) ?? collapsedRect(activeWindow.layout, rect.paneId, rect);
        return from && !sameBounds(from, rect) ? { ...rect, from } : rect;
      })
    : layoutRects.rects;
  const rectByPane = new Map(rects.map((r) => [r.paneId, r]));
  useLayoutEffect(() => {
    if (moved) {
      // Closed panes' empty boxes shrink to the edge they came from.
      const closed = [...prevShown.rects.values()].flatMap((rect) => {
        if (rectByPane.has(rect.paneId)) return [];
        const to = collapsedRect(prevShown.layout, rect.paneId, rect);
        return to ? [{ key: rect.paneId, from: rect, to }] : [];
      });
      if (closed.length) setGhosts((prev) => [...prev, ...closed]);
    }
    shownRef.current =
      isActiveSession && activeWindow && !zoomedPaneId
        ? { windowId: activeWindow.id, layoutKey, layout: activeWindow.layout, rects: rectByPane }
        : null;
  });
  const HIDDEN: LayoutRect = { paneId: '', top: 0, left: 0, width: 100, height: 100 };

  // display-panes (prefix + q): map each pane id to its layout-order index, the
  // number shown over it and the digit that selects it (see useKeybindings).
  const paneNumberById = new Map(
    activeWindow ? paneIdsInOrder(activeWindow.layout).map((id, i) => [id, i] as const) : [],
  );
  const showPaneNumbers = isActiveSession && paneNumbersVisible;
  // A zoomed pane (or the one narrow screens show) fills the window.
  const numberRects = zoomedPaneId ? [{ ...HIDDEN, paneId: zoomedPaneId }] : rects;

  // Place the Neovim and VS Code surfaces over their panes while this session is shown.
  const pinnedRect = (paneId: string | null): LayoutRect | null =>
    paneId === null
      ? null
      : zoomedPaneId
        ? zoomedPaneId === paneId
          ? { ...HIDDEN, paneId }
          : null
        : (rectByPane.get(paneId) ?? null);
  const nvimRect = pinnedRect(nvimPaneId);
  useLayoutEffect(() => {
    if (isActiveSession) useStore.getState().setNvimRect(nvimRect);
  }, [isActiveSession, nvimRect?.paneId, nvimRect?.top, nvimRect?.left, nvimRect?.width, nvimRect?.height]);
  const vscodeRect = pinnedRect(vscodePaneId);
  useLayoutEffect(() => {
    if (isActiveSession) useStore.getState().setVscodeRect(vscodeRect);
  }, [isActiveSession, vscodeRect?.paneId, vscodeRect?.top, vscodeRect?.left, vscodeRect?.width, vscodeRect?.height]);
  const browserRect = pinnedRect(browserPaneId);
  useLayoutEffect(() => {
    if (isActiveSession) useStore.getState().setBrowserRect(browserRect);
  }, [
    isActiveSession,
    browserRect?.paneId,
    browserRect?.top,
    browserRect?.left,
    browserRect?.width,
    browserRect?.height,
  ]);

  // The keep-alive pool: one TerminalPane per pane across *every* window of the
  // session, keyed by paneId so the instance survives window switches (same key
  // → React reuses it, no unmount/remount). Only the active window's panes are
  // visible; the rest are display:none + paused by TerminalPane. Switching
  // windows is then just pause/resume + reposition — no emulator teardown,
  // no pane-socket reconnect, no scrollback replay.

  // Zoom is rendered by overlaying the zoomed pane at full size on top of the
  // others rather than swapping the tree to a single pane. Keeping every pane
  // mounted makes zoom toggle instant: no Terminal teardown, no pane-socket
  // reconnect, no scrollback replay. The zoomed pane's container grows to fill
  // the grid, so its ResizeObserver resizes the PTY exactly as a
  // mounted-from-scratch zoomed pane would. Dividers are dropped while zoomed —
  // they'd sit under the overlay and resizing a hidden split makes no sense.

  return (
    <div
      ref={containerRef}
      style={{
        // Every pooled SessionPane overlays the same region (absolute inset:0);
        // only the active session's is shown. display:none on the whole root
        // hides an inactive session's panes and dividers at once, while its
        // TerminalPanes stay mounted + paused (see the per-pane visible flag).
        display: isActiveSession ? undefined : 'none',
        position: 'absolute',
        inset: 0,
        columnGap: PANE_GAP,
      }}
    >
      {windows
        .filter((win) => mountedWindowIds.has(win.id) || win.id === activeWindow?.id)
        .flatMap((win) =>
          win.panes.map((pane) => {
            const rect = rectByPane.get(pane.id);
            // Visible only when this is the shown session AND the pane is in its
            // active window. A background session keeps all panes hidden +
            // paused (sockets still streaming), so switch-back is instant.
            const inActiveWindow = rect !== undefined;
            const isZoomed = isActiveSession && inActiveWindow && pane.id === zoomedPaneId;
            // When a pane is zoomed, hide all other active-window panes so their
            // borders don't show behind the zoomed overlay.
            const visible = isActiveSession && inActiveWindow && (!zoomedPaneId || isZoomed);
            const paneRect = isZoomed ? { ...HIDDEN, paneId: pane.id } : (rect ?? { ...HIDDEN, paneId: pane.id });
            const isActive = visible && (zoomedPaneId ? isZoomed : pane.id === activePaneId);
            // This tab's Neovim, VS Code, or browser over the pane hides its editor here.
            const coveredLocally = pane.id === nvimPaneId || pane.id === vscodePaneId || pane.id === browserPaneId;
            const editor = (pane.editor.shown || pane.editor.files.length > 0) && (
              <NativeEditor
                key={`editor-${pane.id}`}
                paneId={pane.id}
                state={pane.editor}
                rect={paneRect}
                visible={visible && pane.editor.shown && !coveredLocally}
                focused={isActive}
                zoomed={isZoomed}
                config={config}
                send={send}
                onActivate={() => send({ type: 'select_pane', session_id: sessionId, pane_id: pane.id })}
              />
            );
            return [
              <TerminalPane
                key={pane.id}
                sessionId={sessionId}
                paneId={pane.id}
                rect={paneRect}
                isActive={isActive}
                title={pane.title}
                cwd={pane.cwd}
                agentStatus={pane.agent_status}
                githubKey={pane.git?.branch ? githubKey(pane.git.repo_id, pane.git.branch) : undefined}
                paneIndex={paneNumberById.get(pane.id) ?? 0}
                visible={visible}
                isZoomed={isZoomed}
                previewConfig={previewConfig}
                registry={registryRef.current}
                send={send}
              />,
              editor,
            ];
          }),
        )}
      {ghosts.map((ghost) => (
        <PaneGhost
          key={ghost.key}
          from={ghost.from}
          to={ghost.to}
          color={config?.theme?.selectionBackground ?? STARTUP_THEME.selectionBackground}
          onDone={() => setGhosts((prev) => prev.filter((g) => g !== ghost))}
        />
      ))}
      {!zoomedPaneId &&
        dividers.map((divider) => (
          <div
            key={divider.id}
            onPointerDown={(e) => handleDividerPointerDown(divider, e)}
            className={DIVIDER_TOUCH_TARGET}
            style={
              divider.orientation === 'vertical'
                ? {
                    position: 'absolute',
                    ...dividerStyle(divider),
                    transform: 'translateX(-50%)',
                    width: '8px',
                    cursor: 'col-resize',
                    zIndex: 10,
                  }
                : {
                    position: 'absolute',
                    ...dividerStyle(divider),
                    transform: 'translateY(-50%)',
                    height: '8px',
                    cursor: 'row-resize',
                    zIndex: 10,
                  }
            }
          />
        ))}
      {fileBrowserOpen && isActiveSession && fileBrowserPaneId && (
        <Dialog
          key="file-browser"
          isOpen
          onOpenChange={(open) => {
            if (!open) useStore.getState().setFileBrowserOpen(false);
          }}
          aria-label={fileBrowserMode === 'process' ? 'Processes' : fileBrowserMode === 'git' ? 'Git' : 'Files'}
          padding={0}
          {...wideDialogProps(narrow || previewFullscreen)}
        >
          {fileBrowserMode === 'process' ? (
            <ProcessOverlay
              sessionId={sessionId}
              paneId={fileBrowserPaneId}
              send={send}
              onClose={() => useStore.getState().setFileBrowserOpen(false)}
            />
          ) : (
            <FileBrowserOverlay
              cwd={fileBrowserCwd}
              sessionId={sessionId}
              paneId={fileBrowserPaneId}
              send={send}
              onClose={() => useStore.getState().setFileBrowserOpen(false)}
            />
          )}
        </Dialog>
      )}
      {shownPopup && <PopupDialog key={shownPopup.id} popup={shownPopup} sessionId={sessionId} send={send} />}
      {showPaneNumbers &&
        numberRects.map((rect) => {
          const n = paneNumberById.get(rect.paneId);
          if (n === undefined) return null;
          const accent = config?.theme?.yellow ?? STARTUP_THEME.yellow;
          const bg = config?.theme?.background ?? STARTUP_THEME.background;
          return (
            <div
              key={`num-${rect.paneId}`}
              style={{
                position: 'absolute',
                ...paneRectStyle(rect),
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                pointerEvents: 'none',
                // Above the Neovim surface.
                zIndex: 35,
              }}
            >
              <span
                style={{
                  color: accent,
                  background: bg,
                  border: `2px solid ${accent}`,
                  borderRadius: '8px',
                  padding: '0.1em 0.4em',
                  fontFamily: 'var(--btmux-font)',
                  fontWeight: 'bold',
                  // Scale the digit with the pane, clamped so tiny panes stay legible.
                  fontSize: 'clamp(1.5rem, 8vmin, 6rem)',
                  lineHeight: 1,
                  opacity: 0.92,
                }}
              >
                {n}
              </span>
            </div>
          );
        })}
    </div>
  );
}

/** A closed pane's empty box, shrinking to the edge it grew from. */
function PaneGhost({ from, to, color, onDone }: { from: Bounds; to: Bounds; color: string; onDone: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const done = useRef(onDone);
  done.current = onDone;
  useLayoutEffect(() => animateGhost(ref.current!, from, to, () => done.current()), [from, to]);
  return (
    <div
      ref={ref}
      aria-hidden
      data-pane-ghost
      style={{
        position: 'absolute',
        ...paneRectStyle(to),
        border: `1px solid ${color}`,
        borderRadius: 'var(--radius-container)',
        pointerEvents: 'none',
      }}
    />
  );
}
