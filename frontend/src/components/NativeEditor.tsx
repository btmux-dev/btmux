import { useEffect, useMemo, useReducer, useRef, useState } from 'react';
import { Banner } from '@astryxdesign/core/Banner';
import { Button } from '@astryxdesign/core/Button';
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog';
import { IconButton } from '@astryxdesign/core/IconButton';
import { HStack, VStack } from '@astryxdesign/core/Layout';
import { Tab, TabList } from '@astryxdesign/core/TabList';
import { Text } from '@astryxdesign/core/Text';
import { DropdownMenu } from '@astryxdesign/core/DropdownMenu';
import { Circle, FileWarning, X } from 'lucide-react';
import type { EditorViewOp, PaneEditor as PaneEditorState } from '../generated/protocol';
import type { PaneEditor } from '../lib/nativeEditor/paneEditor';
import type { EditorAppearance } from '../lib/nativeEditor/theme';
import { onReveal } from '../lib/nativeEditor/reveal';
import { registerEditorCommands } from '../lib/nativeEditor/registry';
import { editorConnection } from '../lib/nativeEditor/connection';
import type { BufferSummary, LspServerStatus } from '../generated/protocol';
import { ClientConfig, LayoutRect } from '../state/types';
import { ClientMessage } from '../protocol/messages';
import { useStore } from '../state/store';
import { STARTUP_THEME } from '../state/startupTheme';
import { getTerminalFontFamily, getTerminalFontSize, getTerminalFontWeight } from '../state/configDefaults';
import { buildFontFamily, terminalTransparency } from './TerminalPane';
import { Spinner } from './Spinner';

let loader: Promise<typeof import('../lib/nativeEditor/paneEditor')> | null = null;
const loadEditor = () => (loader ??= import('../lib/nativeEditor/paneEditor'));

const basename = (path: string) => path.slice(path.lastIndexOf('/') + 1) || path;

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
  const [editor, setEditor] = useState<PaneEditor | null>(null);
  const [, changed] = useReducer((n: number) => n + 1, 0);
  const [closing, setClosing] = useState<string | null>(null);
  const [buffers, setBuffers] = useState<BufferSummary[]>([]);
  useEffect(() => editorConnection.subscribeBuffers(setBuffers), []);
  const [servers, setServers] = useState<LspServerStatus[]>([]);
  useEffect(() => editorConnection.onLspStatus(setServers), []);
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

  useEffect(() => {
    let disposed = false;
    let created: PaneEditor | null = null;
    void loadEditor().then(({ PaneEditor }) => {
      if (disposed || !hostRef.current) return;
      created = new PaneEditor(hostRef.current, paneId, appearanceRef.current, {
        changed,
        hide: () => opRef.current({ kind: 'hide' }),
        error: (message) => useStore.getState().showToast(message, 'error'),
        open: (path) => opRef.current({ kind: 'open', path, line: null }),
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
    if (!editor) return;
    return registerEditorCommands(paneId, (action) => editor.command(action));
  }, [editor, paneId]);

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
  const close = (path: string) => {
    if (editor?.status(path)?.dirty) setClosing(path);
    else op({ kind: 'close', path });
  };

  return (
    <div
      className="btm-mono"
      data-editor-pane={paneId}
      inert={!visible}
      aria-hidden={!visible}
      onMouseDown={() => {
        if (!focused) onActivate();
      }}
      style={{
        position: 'absolute',
        top: `${rect.top}%`,
        left: `${rect.left}%`,
        width: `${rect.width}%`,
        height: `${rect.height}%`,
        // Mirrors TerminalPane's border.
        border: `1px solid ${zoomed ? theme.magenta : focused ? theme.blue : theme.selectionBackground}`,
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
            const dirty = editor?.status(path)?.dirty;
            return (
              <Tab
                key={path}
                value={path}
                label={basename(path)}
                panelId={panelId}
                endContent={dirty ? <Circle aria-label="Unsaved" size={8} fill="currentColor" /> : undefined}
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
          {active && (
            <IconButton label="Close file" size="sm" variant="ghost" icon={<X />} onClick={() => close(active)} />
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
        {editor && !active && (
          <VStack hAlign="center" vAlign="center" gap={1} className="btm-sans absolute inset-0">
            <Text color="secondary">No open files</Text>
            <Text size="sm" color="secondary">
              Open one from the file browser (prefix + f).
            </Text>
          </VStack>
        )}
      </div>
      {active && (
        <HStack gap={3} vAlign="center" className="shrink-0 px-2 py-0.5">
          <Text size="sm" color="secondary" maxLines={1} className="min-w-0">
            {active}
          </Text>
          <HStack gap={3} className="ml-auto shrink-0">
            {editor && (
              <Text size="sm" color="secondary" hasTabularNumbers>
                Ln {editor.cursor.line}, Col {editor.cursor.column}
                {editor.cursor.selected > 0 && ` (${editor.cursor.selected} selected)`}
              </Text>
            )}
            {editor && (editor.problems.errors > 0 || editor.problems.warnings > 0) && (
              <Text size="sm" color="secondary" hasTabularNumbers>
                {editor.problems.errors} errors, {editor.problems.warnings} warnings
              </Text>
            )}
            {lsp.map((server) => (
              <Text
                key={server.id}
                size="sm"
                color="secondary"
                maxLines={1}
                className="min-w-0"
                aria-label={`Language server ${server.name}`}
              >
                {server.state === 'failed'
                  ? `${server.name}: ${server.message ?? 'failed'}`
                  : server.progress.length
                    ? `${server.name}: ${server.progress[0]}`
                    : server.state === 'starting'
                      ? `${server.name}…`
                      : server.name}
              </Text>
            ))}
            {status?.language && (
              <Text size="sm" color="secondary">
                {status.language}
              </Text>
            )}
            {status && (
              <Text size="sm" color="secondary">
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
              void editor?.save(false, path).then(() => {
                if (!editor?.status(path)?.dirty) op({ kind: 'close', path });
              });
            }}
          />
        </HStack>
      </Dialog>
    </div>
  );
}
