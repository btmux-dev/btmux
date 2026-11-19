import { useSyncExternalStore } from 'react';
import { useNavigate } from 'react-router-dom';
import { TabList, Tab } from '@astryxdesign/core/TabList';
import { IconButton } from '@astryxdesign/core/IconButton';
import { Button } from '@astryxdesign/core/Button';
import { StatusDot } from '@astryxdesign/core/StatusDot';
import { Spinner } from './Spinner';
import { KeyCap } from './KeyHint';
import { HStack } from '@astryxdesign/core/Layout';
import { Activity, Box, FolderOpen, GitBranch, Globe, Keyboard } from 'lucide-react';
import { useStore, type FileBrowserMode } from '../state/store';
import { chromePalette } from '../lib/chrome-colors';
import { sortWindows, WINDOW_MRU_EVENT } from '../state/windowMru';
import { getAnimations, getWindowSort } from '../state/configDefaults';
import { openFileBrowserFiles } from '../lib/openFileBrowserFiles';
import type { ClientMessage } from '../protocol/messages';
import { SysStatBar } from './SysStatBar';
import { editorButton } from './EditorIcon';
import { runAction } from '../hooks/useKeybindings';
import { TailscaleButton } from './TailscaleButton';
import { MultipassButton } from './MultipassButton';
import { EditorMenu } from './EditorMenu';
import type { AgentState } from '../state/types';

/** A window shows its most urgent agent. */
const AGENT_ORDER: readonly AgentState[] = ['blocked', 'working', 'waiting', 'done', 'idle', 'unknown'];

let mruVersion = 0;
window.addEventListener(WINDOW_MRU_EVENT, () => mruVersion++);

export function StatusBar({ sessionId, send }: { sessionId: string; send: (message: ClientMessage) => void }) {
  const sessions = useStore((state) => state.allSessions);
  const config = useStore((state) => state.config);
  const prefixActive = useStore((state) => state.prefixActive);
  const keyOverlay = useStore((state) => state.keyOverlay);
  const navigate = useNavigate();
  useSyncExternalStore(
    (callback) => {
      window.addEventListener(WINDOW_MRU_EVENT, callback);
      return () => window.removeEventListener(WINDOW_MRU_EVENT, callback);
    },
    () => mruVersion,
  );
  const session = sessions.find((session) => session.id === sessionId);
  if (!session) return null;
  const activeWindow = session.windows[session.active_window];
  const pane = activeWindow?.panes[activeWindow.active_pane];
  const state = useStore.getState;
  const closeModals = () => {
    state().setOverlay(null);
    state().setSettingsOpen(false);
    state().setSwitcherOpen(false);
    state().setWindowGridOpen(false);
    state().setAgentGridOpen(false);
  };
  const openFiles = (mode: FileBrowserMode) => {
    if (!pane) return;
    closeModals();
    if (mode === 'files') void openFileBrowserFiles(pane.id, pane.cwd ?? null);
    else state().setFileBrowserOpen(true, pane.cwd ?? null, pane.id, mode);
  };
  const editor = config?.editor ?? 'native';
  const editorLabel = editorButton(editor);
  const toggleEditor = () => {
    closeModals();
    state().setFileBrowserOpen(false);
    runAction('editor', session.id, send, (name) => navigate(`/s/${encodeURIComponent(name)}`));
  };
  const animations = getAnimations(config);
  return (
    <HStack
      gap={2}
      paddingInline={2}
      vAlign="center"
      className="btm-mono min-w-0"
      role="toolbar"
      aria-label="Terminal controls"
    >
      <Button
        label={session.name}
        icon={session.runtime.type === 'multipass' ? <Box /> : undefined}
        tooltip={session.runtime.type === 'multipass' ? 'Runs in the Multipass sandbox' : undefined}
        variant="ghost"
        size="sm"
        onClick={() => {
          closeModals();
          state().setSwitcherOpen(true);
        }}
      />
      <TabList
        value={activeWindow?.id ?? ''}
        onChange={(id) => {
          const index = session.windows.findIndex((item) => item.id === id);
          if (index < 0) return;
          state().setSettingsOpen(false);
          send({ type: 'switch_window', session_id: session.id, index });
          navigate(`/s/${encodeURIComponent(session.name)}/w/${encodeURIComponent(session.windows[index].name)}`);
        }}
        size="sm"
        className="min-w-0 flex-1"
      >
        {sortWindows(session.windows, getWindowSort(config)).map(({ win }, displayIndex) => {
          const notification = win.panes
            .map((pane) => pane.unread)
            .filter((item) => item != null)
            .sort(
              (a, b) =>
                ['error', 'attention', 'success', 'info'].indexOf(a.level) -
                ['error', 'attention', 'success', 'info'].indexOf(b.level),
            )[0];
          const agent = [...win.panes].sort(
            (a, b) => AGENT_ORDER.indexOf(a.agent_status.state) - AGENT_ORDER.indexOf(b.agent_status.state),
          )[0];
          const level =
            notification?.level ??
            (agent?.agent_status.state === 'blocked'
              ? 'attention'
              : agent?.agent_status.state === 'working' || agent?.agent_status.state === 'waiting'
                ? 'info'
                : agent?.agent_status.state === 'done'
                  ? 'success'
                  : null);
          return (
            <Tab
              key={win.id}
              value={win.id}
              label={`${displayIndex} · ${win.name}${win.zoomed_pane ? ' ⛶' : ''}`}
              endContent={
                animations && !notification && agent?.agent_status.state === 'working' ? (
                  <Spinner size="sm" aria-label="Agent working" />
                ) : level ? (
                  <StatusDot
                    label={notification?.body ?? `Agent ${agent?.agent_status.state}`}
                    variant={level === 'attention' ? 'warning' : level === 'info' ? 'accent' : level}
                    isPulsing={animations && (level === 'attention' || level === 'error')}
                  />
                ) : undefined
              }
            />
          );
        })}
      </TabList>
      {prefixActive && <KeyCap keys={config?.prefix ?? 'C-a'} />}
      <HStack gap={1} className="hidden lg:flex">
        <SysStatBar c={chromePalette(config?.theme ?? null)} barH={28} font={12} animations={getAnimations(config)} />
      </HStack>
      {editor === 'pane' || editor === 'native' ? (
        <IconButton
          label={editorLabel.label}
          tooltip={editorLabel.label}
          variant="ghost"
          size="sm"
          icon={editorLabel.icon}
          onClick={toggleEditor}
        />
      ) : (
        <EditorMenu
          key={editor}
          editor={editor}
          paneId={activeWindow?.zoomed_pane ?? pane?.id}
          onToggle={toggleEditor}
          onShutDown={() => send({ type: 'shut_down_editor', editor })}
        />
      )}
      {config?.browser && (
        <IconButton
          label="Browser"
          tooltip="Browser"
          variant="ghost"
          size="sm"
          icon={<Globe />}
          onClick={() => {
            closeModals();
            state().setFileBrowserOpen(false);
            runAction('browser', session.id, send, (name) => navigate(`/s/${encodeURIComponent(name)}`));
          }}
        />
      )}
      <MultipassButton />
      <IconButton
        label="File browser"
        tooltip="File browser"
        variant="ghost"
        size="sm"
        icon={<FolderOpen />}
        onClick={() => openFiles('files')}
      />
      <IconButton
        label="Git mode"
        tooltip="Git mode"
        variant="ghost"
        size="sm"
        icon={<GitBranch />}
        onClick={() => openFiles('git')}
      />
      <IconButton
        label="Process viewer"
        tooltip="Process viewer"
        variant="ghost"
        size="sm"
        icon={<Activity />}
        onClick={() => openFiles('process')}
      />
      <IconButton
        label={keyOverlay ? 'Hide pressed keys' : 'Show pressed keys'}
        tooltip={keyOverlay ? 'Hide pressed keys' : 'Show pressed keys'}
        variant={keyOverlay ? 'secondary' : 'ghost'}
        size="sm"
        icon={<Keyboard />}
        onClick={() => state().setKeyOverlay(!keyOverlay)}
      />
      <TailscaleButton />
    </HStack>
  );
}
