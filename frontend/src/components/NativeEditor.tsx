import { useEffect, useMemo, useReducer, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Banner } from '@astryxdesign/core/Banner';
import { Button } from '@astryxdesign/core/Button';
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog';
import { IconButton } from '@astryxdesign/core/IconButton';
import { HStack, VStack } from '@astryxdesign/core/Layout';
import { Tab, TabList } from '@astryxdesign/core/TabList';
import { StatusDot, type StatusDotProps } from '@astryxdesign/core/StatusDot';
import { Text } from '@astryxdesign/core/Text';
import { Tooltip } from '@astryxdesign/core/Tooltip';
import { DropdownMenu } from '@astryxdesign/core/DropdownMenu';
import { Circle, FileWarning, X } from 'lucide-react';
import type { EditorViewOp, PaneEditor as PaneEditorState } from '../generated/protocol';
import type { PaneEditor } from '../lib/nativeEditor/paneEditor';
import type { EditorAppearance } from '../lib/nativeEditor/theme';
import { onReveal } from '../lib/nativeEditor/reveal';
import { registerEditorCommands } from '../lib/nativeEditor/registry';
import { IS_MAC } from '../lib/nvimKeys';
import { editorConnection } from '../lib/nativeEditor/connection';
import type { BufferSummary, LspServerStatus } from '../generated/protocol';
import { ClientConfig, LayoutRect } from '../state/types';
import { ClientMessage } from '../protocol/messages';
import { useStore } from '../state/store';
import { runAction } from '../hooks/useKeybindings';
import { WhichKey } from './WhichKey';
import { STARTUP_THEME } from '../state/startupTheme';
import { paneRectStyle } from '../state/layout';
import { usePaneMotion } from '../lib/paneMotion';
import {
  getAnimations,
  getTerminalFontFamily,
  getTerminalFontSize,
  getTerminalFontWeight,
} from '../state/configDefaults';
import { buildFontFamily, terminalTransparency } from './TerminalPane';
import { Spinner } from './Spinner';
import { ProgressCard } from './ProgressCard';
import { lspProgress, type LspProgress } from '../lib/nativeEditor/lspProgress';

let loader: Promise<typeof import('../lib/nativeEditor/paneEditor')> | null = null;
const loadEditor = () => (loader ??= import('../lib/nativeEditor/paneEditor'));

const basename = (path: string) => path.slice(path.lastIndexOf('/') + 1) || path;
const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

/** A language server's status-line dot: ready, busy (`$/progress`), starting, not responding, or failed. */
function serverDot(server: LspServerStatus): {
  state: string;
  variant: StatusDotProps['variant'];
  pulsing: boolean;
  text: string;
} {
  if (server.state === 'failed') {
    return { state: 'failed', variant: 'error', pulsing: false, text: server.message ?? 'failed' };
  }
  if (server.state === 'starting') return { state: 'starting', variant: 'neutral', pulsing: true, text: 'starting' };
  if (server.unresponsive) return { state: 'unresponsive', variant: 'warning', pulsing: false, text: 'not responding' };
  const work = server.progress[0];
  if (work) {
    const text = [work.title, work.message, work.percent != null && `${work.percent}%`].filter(Boolean).join(' ');
    return { state: 'busy', variant: 'accent', pulsing: true, text: text || 'working' };
  }
  return { state: 'ready', variant: 'success', pulsing: false, text: 'ready' };
}

// A tab's close control; a button can't hold a button, so it isn't one.
// The unsaved dot gives way to it on hover.
function TabClose({
  name,
  dirty,
  selected,
  onClose,
}: {
  name: string;
  dirty: boolean;
  selected: boolean;
  onClose: () => void;
}) {
  // Touch screens have no hover: inactive tabs always show it, unsaved ones the dot.
  const x = dirty
    ? 'invisible group-hover:visible'
    : selected
      ? ''
      : 'invisible group-hover:visible pointer-coarse:visible';
  return (
    <span
      data-tab-close
      title={`Close ${name} (Delete)`}
      className="-mr-1 ml-1 grid size-4 place-items-center rounded-sm hover:bg-overlay-hover"
      // Keeps the editor's focus.
      onMouseDown={(event) => event.preventDefault()}
      onClick={(event) => {
        // Not a tab activation.
        event.preventDefault();
        event.stopPropagation();
        onClose();
      }}
    >
      {dirty && (
        <Circle
          aria-label="Unsaved"
          size={8}
          fill="currentColor"
          className="col-start-1 row-start-1 group-hover:invisible"
        />
      )}
      <X aria-hidden size={12} className={`col-start-1 row-start-1 ${x}`} />
    </span>
  );
}

/** The native editor over a pane, as the server's `PaneEditor` describes it. */
export function NativeEditor({
  paneId,
  state,
  rect,
  visible,
  focused,
  zoomed,
  config,
  send,
  onActivate,
}: {
  paneId: string;
  state: PaneEditorState;
  rect: LayoutRect;
  visible: boolean;
  /** The pane is the active one. */
  focused: boolean;
  zoomed: boolean;
  config: ClientConfig | null;
  send: (msg: ClientMessage) => void;
  onActivate: () => void;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  usePaneMotion(rootRef, rect);
  const [editor, setEditor] = useState<PaneEditor | null>(null);
  const [, changed] = useReducer((n: number) => n + 1, 0);
  const [closing, setClosing] = useState<string | null>(null);
  const [buffers, setBuffers] = useState<BufferSummary[]>([]);
  useEffect(() => editorConnection.subscribeBuffers(setBuffers), []);
  const [servers, setServers] = useState<LspServerStatus[]>([]);
  const [progress, setProgress] = useState<LspProgress[]>([]);
  useEffect(
    () =>
      editorConnection.onLspStatus((servers) => {
        setServers(servers);
        setProgress((prev) => lspProgress(prev, servers, Date.now()));
      }),
    [],
  );
  // Drop finished cards as they expire.
  useEffect(() => {
    const times = progress.map((p) => p.expires).filter(Number.isFinite);
    if (!times.length) return;
    const id = window.setTimeout(
      () => setProgress((prev) => prev.filter((p) => p.expires > Date.now())),
      Math.max(0, Math.min(...times) - Date.now()) + 16,
    );
    return () => window.clearTimeout(id);
  }, [progress]);
  // Unsaved files this pane doesn't show, as when their pane closed.
  const unsaved = buffers.filter((b) => b.dirty && !state.files.includes(b.path));
  const modalOpen = useStore(
    (s) => !!s.overlay || s.settingsOpen || s.fileBrowserOpen || s.switcherOpen || s.windowGridOpen || s.agentGridOpen,
  );
  const theme = config?.theme ?? STARTUP_THEME;
  const appearance: EditorAppearance = useMemo(
    () => ({
      theme,
      fontFamily: buildFontFamily(getTerminalFontFamily(config)),
      fontSize: getTerminalFontSize(config),
      fontWeight: getTerminalFontWeight(config),
      transparent: terminalTransparency(config),
      dark: config?.color_mode !== 'light',
    }),
    [theme, config],
  );
  const appearanceRef = useRef(appearance);
  appearanceRef.current = appearance;

  const op = (op: EditorViewOp) => send({ type: 'editor_view', pane_id: paneId, op });
  const opRef = useRef(op);
  opRef.current = op;

  const navigate = useNavigate();
  // A mapping to a btmux action: run it for this pane's session.
  const action = (name: string) => {
    const session = useStore
      .getState()
      .allSessions.find((s) => s.windows.some((w) => w.panes.some((p) => p.id === paneId)));
    if (session) runAction(name, session.id, send, (to) => navigate(`/s/${encodeURIComponent(to)}`));
  };
  const actionRef = useRef(action);
  actionRef.current = action;

  useEffect(() => {
    let disposed = false;
    let created: PaneEditor | null = null;
    void loadEditor().then(({ PaneEditor }) => {
      if (disposed || !hostRef.current) return;
      created = new PaneEditor(hostRef.current, paneId, appearanceRef.current, {
        changed,
        hide: () => opRef.current({ kind: 'hide' }),
        error: (message) => useStore.getState().showToast(message, 'error'),
        open: (path, line) => opRef.current({ kind: 'open', path, line: line ?? null }),
        close: (path, force) => closeRef.current(path, force),
        action: (name) => actionRef.current(name),
      });
      setEditor(created);
    });
    return () => {
      disposed = true;
      created?.destroy();
    };
  }, [paneId]);

  const filesKey = state.files.join('\n');
  useEffect(() => {
    editor?.setFiles(state.files, state.active);
  }, [editor, filesKey, state.active]);

  useEffect(() => {
    editor?.setAppearance(appearance);
  }, [editor, appearance]);

  useEffect(() => {
    if (editor) editor.formatOnSave = !!config?.format_on_save;
  }, [editor, config?.format_on_save]);

  useEffect(() => {
    editor?.setVim(!!config?.vim_mode);
  }, [editor, config?.vim_mode]);

  useEffect(() => {
    editor?.setVirtualText(!!config?.virtual_text);
  }, [editor, config?.virtual_text]);

  useEffect(() => {
    editor?.setKeys(
      config?.vim_binds ?? [],
      config?.vim_groups ?? [],
      config?.vim_timeout ?? 1000,
      config?.which_key ?? true,
    );
  }, [editor, config?.vim_binds, config?.vim_groups, config?.vim_timeout, config?.which_key]);

  useEffect(() => {
    editor?.setVisible(visible);
  }, [editor, visible]);

  useEffect(() => {
    if (!editor || !visible) return;
    return registerEditorCommands(paneId, (action) => editor.command(action));
  }, [editor, paneId, visible]);

  useEffect(() => {
    if (!editor) return;
    return onReveal(paneId, ({ path, line }) => editor.reveal(path, line));
  }, [editor, paneId]);

  // Take focus when the pane is active, and back after a modal closes.
  useEffect(() => {
    if (!editor || !visible || !focused || modalOpen) return;
    const id = window.setTimeout(() => editor.focus(), 0);
    return () => window.clearTimeout(id);
  }, [editor, visible, focused, modalOpen, state.active]);

  const active = state.active;
  const panelId = `native-editor-${paneId}`;
  const status = active && editor ? editor.status(active) : null;
  const lsp = status ? servers.filter((s) => status.servers.includes(s.id)) : [];
  const animations = getAnimations(config);
  const work = status ? progress.filter((p) => status.servers.includes(p.server)) : [];
  /** Close `path`, asking about unsaved changes; true if it closes without asking. */
  const close = (path: string, force = false) => {
    // Another pane showing the file keeps its unsaved changes.
    const elsewhere = useStore
      .getState()
      .allSessions.some((s) =>
        s.windows.some((w) => w.panes.some((p) => p.id !== paneId && p.editor.files.includes(path))),
      );
    if (!editor?.status(path)?.dirty || elsewhere) op({ kind: 'close', path });
    else if (force) void editor.revert(path).then(() => op({ kind: 'close', path }));
    else {
      setClosing(path);
      return false;
    }
    return true;
  };
  const closeRef = useRef(close);
  closeRef.current = close;

  return (
    <div
      ref={rootRef}
      className="btm-mono"
      data-editor-pane={paneId}
      inert={!visible}
      aria-hidden={!visible}
      onMouseDown={() => {
        if (!focused) onActivate();
      }}
      style={{
        position: 'absolute',
        ...paneRectStyle(rect),
        // Mirrors TerminalPane's border.
        border: `1px solid ${zoomed ? theme.magenta : focused ? theme.blue : theme.selectionBackground}`,
        borderRadius: 'var(--radius-container)',
        zIndex: 30,
        display: 'flex',
        flexDirection: 'column',
        overflow: 'hidden',
        visibility: visible ? undefined : 'hidden',
        background: appearance.transparent ? 'transparent' : theme.background,
      }}
    >
      <HStack gap={1} vAlign="center" className="btm-sans shrink-0 pr-1">
        <TabList size="sm" role="tablist" value={active ?? ''} onChange={(path) => op({ kind: 'activate', path })}>
          {state.files.map((path) => {
            const dirty = !!editor?.status(path)?.dirty;
            return (
              <Tab
                key={path}
                value={path}
                label={basename(path)}
                panelId={panelId}
                className="group"
                // Middle click closes, as in browsers.
                onMouseDown={(event) => event.button === 1 && event.preventDefault()}
                onAuxClick={(event) => event.button === 1 && close(path)}
                // The X can't be a button inside the tab, so Delete closes the focused tab.
                aria-keyshortcuts="Delete"
                aria-description="Delete closes the file"
                onKeyDown={(event) => {
                  if (event.key !== 'Delete' || event.ctrlKey || event.altKey || event.metaKey || event.shiftKey)
                    return;
                  event.preventDefault();
                  const tab = event.currentTarget;
                  const tabs = [
                    ...(tab.closest('[role="tablist"]')?.querySelectorAll<HTMLElement>('[role="tab"]') ?? []),
                  ];
                  const at = tabs.indexOf(tab);
                  // Focus stays in the strip once this tab goes; asking to save keeps it.
                  if (close(path)) (tabs[at + 1] ?? tabs[at - 1])?.focus();
                }}
                endContent={
                  <TabClose
                    name={basename(path)}
                    dirty={dirty}
                    selected={path === active}
                    onClose={() => close(path)}
                  />
                }
              />
            );
          })}
        </TabList>
        <HStack gap={0} className="ml-auto">
          {unsaved.length > 0 && (
            <DropdownMenu
              alignment="end"
              hasChevron={false}
              renderTrigger={(props) => (
                <IconButton
                  label="Other unsaved files"
                  tooltip="Other unsaved files"
                  size="sm"
                  variant="ghost"
                  icon={<FileWarning />}
                  {...props}
                />
              )}
              items={unsaved.map((b) => ({
                id: b.id,
                label: basename(b.path),
                description: b.path,
                onClick: () => op({ kind: 'open', path: b.path, line: null }),
              }))}
            />
          )}
        </HStack>
      </HStack>
      {status && status.disk !== 'same' && (
        <Banner
          status="warning"
          container="section"
          title={status.disk === 'deleted' ? 'Deleted on disk' : 'Changed on disk'}
          description={
            status.disk === 'deleted'
              ? 'Saving creates the file again.'
              : 'This file has unsaved changes and was also changed outside the editor.'
          }
          endContent={
            status.disk === 'changed' && (
              <HStack gap={2}>
                <Button label="Reload" size="sm" onClick={() => void editor?.revert()} />
                <Button label="Keep mine" size="sm" variant="secondary" onClick={() => void editor?.save(true)} />
              </HStack>
            )
          }
        />
      )}
      {status?.conflict && status.disk === 'same' && (
        <Banner
          status="warning"
          container="section"
          title="Not saved"
          description="The file changed on disk since it was opened."
          endContent={
            <HStack gap={2}>
              <Button label="Reload" size="sm" onClick={() => void editor?.revert()} />
              <Button label="Overwrite" size="sm" variant="secondary" onClick={() => void editor?.save(true)} />
            </HStack>
          }
        />
      )}
      {status?.error && <Banner status="error" container="section" title="Cannot open" description={status.error} />}
      <div id={panelId} role="tabpanel" style={{ position: 'relative', flex: 1, minHeight: 0 }}>
        {/* CodeMirror's own DOM, apart from React's. */}
        <div ref={hostRef} style={{ position: 'absolute', inset: 0 }} />
        {(!editor || (active && status && !status.ready && !status.error)) && (
          <VStack hAlign="center" vAlign="center" className="absolute inset-0">
            <Spinner size="lg" label="Opening…" />
          </VStack>
        )}
        {work.length > 0 && (
          <VStack
            gap={1}
            hAlign="end"
            className="pointer-events-none absolute inset-x-2 top-2 z-10"
            onMouseDown={(e) => e.preventDefault()}
          >
            {work.map((p) => (
              <div key={p.id} data-lsp-progress={p.id} className="flex w-full justify-end">
                <ProgressCard progress={p} />
              </div>
            ))}
          </VStack>
        )}
        {editor?.keys.popup && (
          <div className="pointer-events-none absolute inset-x-2 bottom-2 z-10" onMouseDown={(e) => e.preventDefault()}>
            <WhichKey pending={editor.keys.popup} />
          </div>
        )}
        {editor && !active && (
          <VStack hAlign="center" vAlign="center" gap={1} className="btm-sans absolute inset-0">
            <Text color="secondary">No open files</Text>
            <Text size="sm" color="secondary">
              Open one with quick open ({IS_MAC ? '⌘P' : 'Ctrl+P'}) or the file browser (prefix + f).
            </Text>
          </VStack>
        )}
      </div>
      {active && (
        // A container, so narrow panes drop the least useful items.
        <HStack gap={3} vAlign="center" className="@container shrink-0 px-2 py-0.5" data-editor-status>
          {editor?.vimMode && (
            <Text size="sm" weight="semibold" maxLines={1} className="shrink-0" aria-label="Vim mode">
              {editor.vimMode}
            </Text>
          )}
          {editor?.keys.showcmd && (
            <Text size="sm" maxLines={1} className="btm-mono shrink-0" aria-label="Pending keys">
              {editor.keys.showcmd}
            </Text>
          )}
          {/* The path gives up its room before the items after it. */}
          <Text size="sm" color="secondary" maxLines={1} className="min-w-0 shrink-[1000]">
            {active}
          </Text>
          {/* Items stay on one line; only server names shrink, and past that it clips. */}
          <HStack gap={3} className="ml-auto min-w-0 overflow-hidden whitespace-nowrap">
            {editor?.git && (editor.git.added > 0 || editor.git.changed > 0 || editor.git.removed > 0) && (
              <Text size="sm" color="secondary" hasTabularNumbers aria-label="Changes from the Git index">
                {[
                  editor.git.added && `+${editor.git.added}`,
                  editor.git.changed && `~${editor.git.changed}`,
                  editor.git.removed && `-${editor.git.removed}`,
                ]
                  .filter(Boolean)
                  .join(' ')}
              </Text>
            )}
            {editor && (
              <Text size="sm" color="secondary" hasTabularNumbers>
                Ln {editor.cursor.line}, Col {editor.cursor.column}
                {editor.cursor.selected > 0 && ` (${editor.cursor.selected} selected)`}
              </Text>
            )}
            {editor && (editor.problems.errors > 0 || editor.problems.warnings > 0) && (
              <Text size="sm" color="secondary" hasTabularNumbers>
                {plural(editor.problems.errors, 'error')}, {plural(editor.problems.warnings, 'warning')}
              </Text>
            )}
            {lsp.map((server) => {
              const dot = serverDot(server);
              const summary = `${server.name}: ${dot.text}`;
              return (
                // Narrow panes keep the dot; focus, hover, or a tap shows its tooltip.
                <Tooltip key={server.id} content={summary} touchTrigger="tap">
                  <button
                    type="button"
                    aria-label={`Language server ${summary}`}
                    data-lsp-state={dot.state}
                    className="flex min-w-0 items-center gap-1 rounded-sm"
                    // A click keeps the editor's focus.
                    onMouseDown={(event) => event.preventDefault()}
                  >
                    <StatusDot variant={dot.variant} isPulsing={animations && dot.pulsing} label={summary} />
                    <Text size="sm" color="secondary" maxLines={1} className="min-w-0 @max-md:hidden" data-lsp-name>
                      {server.state === 'failed'
                        ? `${server.name}: ${server.message ?? 'failed'}`
                        : server.state === 'starting'
                          ? `${server.name}…`
                          : server.unresponsive
                            ? `${server.name}: not responding`
                            : server.name}
                    </Text>
                  </button>
                </Tooltip>
              );
            })}
            {status?.language && (
              <Text size="sm" color="secondary" className="@max-md:hidden">
                {status.language}
              </Text>
            )}
            {status && (
              <Text size="sm" color="secondary" className="@max-md:hidden">
                {status.lineEnding.toUpperCase()}
              </Text>
            )}
          </HStack>
        </HStack>
      )}
      <Dialog isOpen={closing !== null} onOpenChange={(open) => !open && setClosing(null)} width={420} padding={4}>
        <DialogHeader
          title={`Save changes to ${closing ? basename(closing) : ''}?`}
          subtitle="Your changes are lost if you don't save them."
          hasDivider={false}
        />
        <HStack gap={2} hAlign="end" className="pt-4">
          <Button label="Cancel" variant="secondary" onClick={() => setClosing(null)} />
          <Button
            label="Don't save"
            variant="destructive"
            onClick={() => {
              const path = closing!;
              setClosing(null);
              void editor?.revert(path).then(() => op({ kind: 'close', path }));
            }}
          />
          <Button
            label="Save"
            variant="primary"
            onClick={() => {
              const path = closing!;
              setClosing(null);
              void editor?.save(false, path).then((saved) => {
                if (saved) op({ kind: 'close', path });
              });
            }}
          />
        </HStack>
      </Dialog>
    </div>
  );
}
