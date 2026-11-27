import { useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { IconButton } from '@astryxdesign/core/IconButton';
import { Activity, Bot, Command, FolderOpen, GitBranch, Keyboard, KeyboardOff, Menu, Search } from 'lucide-react';
import { useStore, type FileBrowserMode } from '../state/store';
import { agentGroup } from '../lib/agentGroups';
import { openFileBrowserFiles } from '../lib/openFileBrowserFiles';
import { computeRectsAndDividers, paneIdsInOrder } from '../state/layout';
import { STARTUP_THEME } from '../state/startupTheme';
import { useNavigate } from 'react-router-dom';
import { runAction } from '../hooks/useKeybindings';
import { editorButton } from './EditorIcon';
import { TailscaleButton } from './TailscaleButton';
import { MultipassButton } from './MultipassButton';
import { MobileKeyBar, hideKeyboard, useModifiers } from './MobileKeyBar';
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
  // Kept here: the keys remount when the dock compacts, which mustn't disarm them.
  const modifiers = useModifiers();
  const toggleKeys = () => {
    setKeysOpen(!keysOpen);
    try {
      localStorage.setItem(KEYS_STORAGE, String(!keysOpen));
    } catch {}
  };
  // Compact when the full dock would leave the panes less than half the app's height,
  // as when the keyboard is up. Full heights are measured only while full, per keys state.
  const dockRef = useRef<HTMLDivElement>(null);
  const [shellHeight, setShellHeight] = useState(0);
  const [fullHeights, setFullHeights] = useState<Partial<Record<'open' | 'closed', number>>>({});
  const keysState = keysOpen ? 'open' : 'closed';
  const fullHeight = fullHeights[keysState];
  const compact = fullHeight !== undefined && shellHeight > 0 && shellHeight - fullHeight < shellHeight / 2;
  const compactRef = useRef(compact);
  compactRef.current = compact;
  useLayoutEffect(() => {
    const dock = dockRef.current;
    const shell = dock?.closest('.astryx-app-shell');
    if (!dock || !shell) return;
    const measure = () => {
      setShellHeight(shell.clientHeight);
      if (compactRef.current) return;
      const height = dock.offsetHeight;
      setFullHeights((heights) => (heights[keysState] === height ? heights : { ...heights, [keysState]: height }));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(shell);
    observer.observe(dock);
    return () => observer.disconnect();
  }, [keysState, compact]);
  // Rows of equal length (6 + 6, not 11 + 1): the fewest rows that fit, then the fewest columns for them.
  const rowRef = useRef<HTMLDivElement>(null);
  const toolbarRef = useRef<HTMLDivElement>(null);
  const [toolbarWidth, setToolbarWidth] = useState<number>();
  const hasMinimap = !!win && win.panes.length > 1;
  useLayoutEffect(() => {
    const row = rowRef.current;
    const toolbar = toolbarRef.current;
    if (compact || !row || !toolbar) return;
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
  }, [hasMinimap, compact]);
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
    <IconButton key={label} label={label} variant="ghost" size="lg" icon={icon} onClick={onClick} />
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
  const keys = (
    <IconButton
      key="Keys"
      label="Keys"
      variant={keysOpen ? 'secondary' : 'ghost'}
      size="lg"
      icon={<Keyboard />}
      aria-pressed={keysOpen}
      onClick={toggleKeys}
    />
  );
  const hide = action('Hide keyboard', <KeyboardOff />, hideKeyboard);
  const surfaces = [
    action('Menu', <Menu />, () => open(() => useStore.getState().setMobileNavOpen(true))),
    action('Switch session', <Search />, () => open(() => useStore.getState().setSwitcherOpen(true))),
    <IconButton
      key="Agents"
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
    />,
    action('Commands', <Command />, () =>
      open(() => {
        const state = useStore.getState();
        state.setOverlay({ mode: 'command', title: 'Commands', commands: state.config?.commands ?? [] });
      }),
    ),
  ];
  const tools = [
    action('Git mode', <GitBranch />, () => openFiles('git')),
    action('File browser', <FolderOpen />, () => openFiles('files')),
    action(editor.label, editor.icon, () =>
      open(() => runAction('editor', sessionId, send, (name) => navigate(`/s/${encodeURIComponent(name)}`))),
    ),
    action('Process viewer', <Activity />, () => openFiles('process')),
    <MultipassButton key="Sandbox" size="lg" />,
    <TailscaleButton key="Tailnet" size="lg" />,
  ];
  const keyPanel = (
    <MobileKeyBar sessionId={sessionId} send={send} layout={compact ? 'row' : 'panel'} modifiers={modifiers} />
  );

  if (compact) {
    // Short screens (the keyboard is up): one scrolling row, Hide keyboard and the keys first.
    return (
      <div ref={dockRef} style={{ padding: '4px 6px' }} onMouseDown={(event) => event.preventDefault()}>
        <div
          role="toolbar"
          aria-label="Quick actions"
          // 40px keys: a little smaller, still easy to tap.
          className="btm-toolbar-glass [&_button]:size-10 [&>*]:shrink-0"
          style={{ ...card(theme), display: 'flex', alignItems: 'center', gap: 2, padding: 2, overflowX: 'auto' }}
        >
          {hide}
          {keys}
          {keysOpen && keyPanel}
          {surfaces}
          {tools}
        </div>
      </div>
    );
  }
  return (
    <div
      ref={dockRef}
      style={{ display: 'flex', flexDirection: 'column', gap: 8, padding: 10 }}
      // Keep the keyboard open: a tap here mustn't take focus from the terminal.
      // Not on pointerdown, which cancels a tap's click in WebKit.
      onMouseDown={(event) => event.preventDefault()}
    >
      {keysOpen && (
        <div className="btm-toolbar-glass self-start" style={card(theme)}>
          {keyPanel}
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
            flexWrap: 'wrap',
            gap: 4,
            padding: 4,
          }}
        >
          {surfaces}
          {keys}
          {hide}
          {tools}
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
