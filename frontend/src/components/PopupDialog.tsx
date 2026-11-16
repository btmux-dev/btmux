import { useEffect, useRef, useState } from 'react';
import { Dialog } from '@astryxdesign/core/Dialog';
import { IconButton } from '@astryxdesign/core/IconButton';
import { HStack, VStack } from '@astryxdesign/core/Layout';
import { Text } from '@astryxdesign/core/Text';
import { useMediaQuery } from '@astryxdesign/core/hooks';
import { X } from 'lucide-react';
import { clsx } from 'clsx';
import type { PopupSummary } from '../generated/protocol';
import type { ClientMessage } from '../protocol/messages';
import { usePaneTerminal } from '../hooks/usePaneTerminal';
import { useStore } from '../state/store';
import { STARTUP_THEME } from '../state/startupTheme';
import { getAnimations } from '../state/configDefaults';
import { CONNECTION_STATE_LABEL } from '../lib/connectionState';
import { NARROW_QUERY, TOUCH_QUERY } from '../lib/touch';
import { wideDialogProps } from '../lib/wideDialog';
import { useTerminalOptions } from './TerminalPane';
import { MobileKeyBar } from './MobileKeyBar';
import { PaneSearch } from './PaneSearch';
import { PaneWidget } from './PaneWidget';

interface Props {
  popup: PopupSummary;
  sessionId: string;
  send: (msg: ClientMessage) => void;
}

/** Whether a modal surface, which a popup opened now would cover, is open. */
function useOtherModalOpen() {
  return useStore(
    (s) => !!s.overlay || s.settingsOpen || s.switcherOpen || s.fileBrowserOpen || s.windowGridOpen || s.agentGridOpen,
  );
}

/**
 * A `btmux popup` program over its pane's window, sized like the file browser.
 * It closes when the program exits; Esc and backdrop clicks go to the program.
 * It waits for a modal surface open before it to close, so it doesn't take
 * the keyboard from one in use.
 */
export function PopupDialog(props: Props) {
  const otherModalOpen = useOtherModalOpen();
  const [ready, setReady] = useState(!otherModalOpen);
  useEffect(() => {
    if (!otherModalOpen) setReady(true);
  }, [otherModalOpen]);
  return ready ? <OpenPopupDialog {...props} /> : null;
}

function OpenPopupDialog({ popup, sessionId, send }: Props) {
  const narrow = useMediaQuery(NARROW_QUERY);
  const touch = useMediaQuery(TOUCH_QUERY);
  const config = useStore((s) => s.config);
  const termOptions = useTerminalOptions(config);
  const rootRef = useRef<HTMLDivElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const visibleRef = useRef(true);
  const { term, connection, connectionError, initialReplayRendered } = usePaneTerminal({
    containerRef,
    paneId: popup.id,
    termOptions,
    visibleRef,
  });

  // By value: every `popups` broadcast brings new objects, and rerunning
  // would close this popup's search.
  const { id, pane_id: paneId, title } = popup;
  useEffect(() => {
    const store = useStore.getState();
    store.setPopupShown({ id, pane_id: paneId, title });
    // A pane's search would be hidden under the popup, yet own the keyboard.
    if (store.searchPaneId !== null && store.searchPaneId !== id) store.setSearchPaneId(null);
    return () => {
      const store = useStore.getState();
      if (store.popupShown?.id === id) store.setPopupShown(null);
      if (store.searchPaneId === id) store.setSearchPaneId(null);
    };
  }, [id, paneId, title]);

  useEffect(() => {
    term?.setTheme(config?.theme ?? STARTUP_THEME);
  }, [config?.theme, term]);

  // Until the replay shows, keys go nowhere rather than to the close button.
  useEffect(() => rootRef.current?.focus(), []);
  // Focus the program once shown, and again when a dialog over it closes.
  const otherModalOpen = useOtherModalOpen();
  // The connection banner, during an upgrade.
  const disconnected = useStore((s) => s.controlConnectionState !== 'connected');
  const coveredByAnother = otherModalOpen || disconnected;
  // `btmux pick`, `confirm`, and `input` run in the popup, and search, show over it.
  const searchOpen = useStore((s) => s.searchPaneId === popup.id);
  const widget = useStore((s) => s.widgets.find((widget) => widget.pane_id === popup.id));
  const widgetId = widget?.id ?? null;
  useEffect(() => {
    if (term && initialReplayRendered && !coveredByAnother && !searchOpen) term.focus();
  }, [term, initialReplayRendered, coveredByAnother, searchOpen, widgetId]);

  const background = config?.theme?.background ?? STARTUP_THEME.background;
  return (
    <Dialog
      isOpen
      onOpenChange={() => {}}
      purpose="required"
      aria-label={popup.title}
      padding={0}
      {...wideDialogProps(narrow)}
    >
      <VStack ref={rootRef} tabIndex={-1} className="h-full min-h-0 overflow-hidden outline-none" data-popup={popup.id}>
        <HStack gap={3} vAlign="center" className="min-w-0 flex-none border-b border-border px-3 py-1.5">
          <Text size="sm" className="btm-mono min-w-0 flex-1 truncate">
            {popup.title}
          </Text>
          {connection !== 'connected' && (
            <Text size="sm" color="secondary" role="status" className={clsx(getAnimations(config) && 'animate-pulse')}>
              {connectionError ?? CONNECTION_STATE_LABEL[connection]}
            </Text>
          )}
          <IconButton
            label="Close popup"
            icon={<X />}
            variant="ghost"
            size="sm"
            onClick={() => send({ type: 'close_popup', id: popup.id })}
          />
        </HStack>
        <div className="relative min-h-0 flex-1" style={{ background }}>
          <div
            ref={containerRef}
            className="absolute inset-3"
            // `visibility` keeps layout, so the grid is sized while the replay stays hidden.
            style={{ visibility: initialReplayRendered ? 'visible' : 'hidden' }}
          />
          {widget && term && initialReplayRendered && (
            <PaneWidget key={widget.id} id={widget.id} kind={widget.kind} term={term} shown send={send} />
          )}
          {searchOpen && term && (
            <PaneSearch
              term={term}
              theme={config?.theme ?? null}
              onClose={() => useStore.getState().setSearchPaneId(null)}
            />
          )}
        </div>
        {/* The dialog is modal, so the app's key bar can't be reached under it. */}
        {touch && <MobileKeyBar sessionId={sessionId} send={send} />}
      </VStack>
    </Dialog>
  );
}
