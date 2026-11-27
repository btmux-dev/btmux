import { useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { IconButton } from '@astryxdesign/core/IconButton';
import { useMediaQuery } from '@astryxdesign/core/hooks';
import { Activity, Bot, Command, FolderOpen, GitBranch, Keyboard, KeyboardOff, Menu, Search } from 'lucide-react';
import { useStore, type FileBrowserMode } from '../state/store';
import { agentGroup } from '../lib/agentGroups';
import { NARROW_QUERY } from '../lib/touch';
import { openFileBrowserFiles } from '../lib/openFileBrowserFiles';
import { computeRectsAndDividers, paneIdsInOrder } from '../state/layout';
import { STARTUP_THEME } from '../state/startupTheme';
import { useNavigate } from 'react-router-dom';
import { runAction } from '../hooks/useKeybindings';
import { editorButton } from './EditorIcon';
import { TailscaleButton } from './TailscaleButton';
import { MultipassButton } from './MultipassButton';
import { MobileKeyBar, hideKeyboard } from './MobileKeyBar';
import { PaneMinimap } from './PaneMinimap';
import type { Theme } from '../generated/protocol';
import type { ClientMessage } from '../protocol/messages';

const KEYS_STORAGE = 'btmux-phone-keys';
/** A button with the gap after it, the toolbar's padding and border, and the minimap's least width with its gap. */
const BUTTON = 48;
const TOOLBAR_FRAME = 10;
const MINIMAP_LEAST = 64 + 10;
const NO_OVERRIDES = new Map<string, number>();

/** A card floating in the dock, over the wallpaper. */
const card = (theme: Theme): CSSProperties => ({
  borderRadius: 'var(--radius-element)',
  border: `1px solid ${theme.selectionBackground}`,
  boxShadow: '0 2px 8px rgb(0 0 0 / 0.35)',
});

/**
 * Phones have no key bar or status bar: large buttons open what they held, the
 * keys open in a panel above them, and a minimap of the window's panes switches
 * panes. They float below the panes, so they never cover terminal output.
 */
export function PhoneDock({ sessionId, send }: { sessionId: string; send: (message: ClientMessage) => void }) {
  // Only below this width is there a drawer; wider, the side nav shows.
  const narrow = useMediaQuery(NARROW_QUERY);
  const navigate = useNavigate();
  const editor = editorButton(useStore((state) => state.config?.editor) ?? 'native');
  const theme = useStore((state) => state.config?.theme) ?? STARTUP_THEME;
  const session = useStore((state) => state.allSessions.find((item) => item.id === sessionId));
  const win = session?.windows[session.active_window];
  const [keysOpen, setKeysOpen] = useState(() => {
    try {
      return localStorage.getItem(KEYS_STORAGE) === 'true';
    } catch {
      return false;
    }
  });
  const toggleKeys = () => {
    setKeysOpen(!keysOpen);
    try {
      localStorage.setItem(KEYS_STORAGE, String(!keysOpen));
    } catch {}
  };
  // Rows of equal length (6 + 6, not 11 + 1): the fewest rows that fit, then the fewest columns for them.
  const rowRef = useRef<HTMLDivElement>(null);
  const toolbarRef = useRef<HTMLDivElement>(null);
  const [toolbarWidth, setToolbarWidth] = useState<number>();
  const hasMinimap = !!win && win.panes.length > 1;
  useLayoutEffect(() => {
    const row = rowRef.current;
    const toolbar = toolbarRef.current;
    if (!row || !toolbar) return;
    const fit = () => {
      // Popovers leave empty nodes beside their buttons.
      const count = [...toolbar.children].filter((child) => (child as HTMLElement).offsetWidth > 0).length;
      const room = row.clientWidth - (hasMinimap ? MINIMAP_LEAST : 0) - TOOLBAR_FRAME + 4;
      const perRow = Math.max(1, Math.floor(room / BUTTON));
      const columns = Math.ceil(count / Math.ceil(count / perRow));
      setToolbarWidth(columns * BUTTON - 4 + TOOLBAR_FRAME);
    };
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(row);
    // Tailnet and sandbox buttons come and go.
    const mutations = new MutationObserver(fit);
    mutations.observe(toolbar, { childList: true });
    return () => {
      observer.disconnect();
      mutations.disconnect();
    };
  }, [hasMinimap]);
  // An agent waiting on the user, else one with a result to read.
  const alert = useStore((state) => {
    let done = false;
    for (const session of state.allSessions)
      for (const win of session.windows)
        for (const pane of win.panes) {
          if (!state.agentPanes.has(pane.id)) continue;
          const group = agentGroup(pane.agent_status.state);
          if (group === 'blocked') return 'blocked';
          if (group === 'done') done = true;
        }
    return done ? 'done' : null;
  });
  const open = (surface: () => void) => {
    const state = useStore.getState();
    state.setOverlay(null);
    state.setSettingsOpen(false);
    state.setSwitcherOpen(false);
    state.setWindowGridOpen(false);
    state.setAgentGridOpen(false);
    state.setFileBrowserOpen(false);
    surface();
  };
  const openFiles = (mode: FileBrowserMode) =>
    open(() => {
      const pane = win?.panes[win.active_pane];
      if (!pane) return;
      if (mode === 'files') void openFileBrowserFiles(pane.id, pane.cwd ?? null);
      else useStore.getState().setFileBrowserOpen(true, pane.cwd ?? null, pane.id, mode);
    });
  const action = (label: string, icon: ReactNode, onClick: () => void) => (
    <IconButton label={label} variant="ghost" size="lg" icon={icon} onClick={onClick} />
  );
  const minimap = win && win.panes.length > 1 && (
    <PaneMinimap
      docked
      rects={computeRectsAndDividers(win.layout, { top: 0, left: 0, width: 100, height: 100 }, NO_OVERRIDES).rects}
      panes={win.panes}
      activePaneId={win.zoomed_pane ?? win.panes[win.active_pane]?.id ?? null}
      numbers={new Map(paneIdsInOrder(win.layout).map((id, i) => [id, i] as const))}
      theme={theme}
      onSelect={(paneId) => send({ type: 'select_pane', session_id: sessionId, pane_id: paneId })}
    />
  );
  return (
    <div
      style={{ display: 'flex', flexDirection: 'column', gap: 8, padding: 10 }}
      // Keep the keyboard open: a tap here mustn't take focus from the terminal.
      // Not on pointerdown, which cancels a tap's click in WebKit.
      onMouseDown={(event) => event.preventDefault()}
    >
      {keysOpen && (
        <div className="btm-toolbar-glass self-start" style={card(theme)}>
          <MobileKeyBar sessionId={sessionId} send={send} layout="panel" />
        </div>
      )}
      <div ref={rowRef} style={{ display: 'flex', alignItems: 'flex-end', gap: 10 }}>
        <div
          ref={toolbarRef}
          role="toolbar"
          aria-label="Quick actions"
          className="btm-toolbar-glass min-w-0 [&_button]:size-11"
          style={{
            ...card(theme),
            display: 'flex',
            // Take the room beside the minimap before wrapping.
            flex: '0 1 auto',
            maxWidth: toolbarWidth,
            // The first buttons stay on the bottom row, nearest the thumb.
            flexWrap: 'wrap-reverse',
            gap: 4,
            padding: 4,
          }}
        >
          <IconButton
            label="Keys"
            variant={keysOpen ? 'secondary' : 'ghost'}
            size="lg"
            icon={<Keyboard />}
            aria-pressed={keysOpen}
            onClick={toggleKeys}
          />
          {action('Hide keyboard', <KeyboardOff />, hideKeyboard)}
          {action('Commands', <Command />, () =>
            open(() => {
              const state = useStore.getState();
              state.setOverlay({ mode: 'command', title: 'Commands', commands: state.config?.commands ?? [] });
            }),
          )}
          <IconButton
            label={alert ? `Agents (${alert})` : 'Agents'}
            variant="ghost"
            size="lg"
            icon={
              <span className="relative">
                <Bot />
                {alert && (
                  <span
                    className="btm-agent-dot -top-0.5 -right-0.5 size-2 rounded-full"
                    // Inline: .btm-agent-dot sets position after the utilities.
                    style={{ position: 'absolute' }}
                    data-agent-state={alert}
                  />
                )}
              </span>
            }
            onClick={() => open(() => useStore.getState().setAgentGridOpen(true))}
          />
          {action('Switch session', <Search />, () => open(() => useStore.getState().setSwitcherOpen(true)))}
          {action(editor.label, editor.icon, () =>
            open(() => runAction('editor', sessionId, send, (name) => navigate(`/s/${encodeURIComponent(name)}`))),
          )}
          {action('File browser', <FolderOpen />, () => openFiles('files'))}
          {action('Git mode', <GitBranch />, () => openFiles('git'))}
          {action('Process viewer', <Activity />, () => openFiles('process'))}
          {narrow && action('Menu', <Menu />, () => open(() => useStore.getState().setMobileNavOpen(true)))}
          <TailscaleButton size="lg" />
          <MultipassButton size="lg" />
        </div>
        {minimap && (
          // The buttons' height, and the width they leave: narrow beside two rows, wider beside one.
          <div className="ml-auto self-stretch" style={{ flex: '1 0 64px', maxWidth: 160 }}>
            {minimap}
          </div>
        )}
      </div>
    </div>
  );
}
