import { useEffect, useRef, useState, type RefObject } from 'react';
import { PaneTerminal, type PaneTerminalOptions } from '../lib/paneTerminal';
import { useStore } from '../state/store';
import { announceWallpaperKeyboardCursor } from '../lib/wallpaperInteraction';
import { clipboardImage, pasteImage } from '../lib/pasteImage';
import { paneMessageHandler } from '../lib/paneSocket';
import { terminalInput, type InputSocket } from '../lib/terminalInput';
import type { ConnectionState } from '../lib/connectionState';

interface Options {
  /** The element wterm renders into. */
  containerRef: RefObject<HTMLDivElement | null>;
  /** A pane's or a popup's id, as `/ws/pane/{id}` takes it. */
  paneId: string;
  termOptions: PaneTerminalOptions | null;
  /**
   * Read inside the ResizeObserver/rAF closures so they always see the current
   * visibility without re-running the mount effect (which would dispose+rebuild
   * the terminal). A hidden pane has a 0-size container, so it must neither fit
   * nor open its socket — doing so would size the PTY to garbage.
   */
  visibleRef: RefObject<boolean>;
  /** Shared paneId→Terminal map; see `TerminalPane`. */
  registry?: Map<string, PaneTerminal>;
  /** Called after each write of output. */
  onWriteRef?: RefObject<(() => void) | null>;
}

/** A pane's emulator and its `/ws/pane` socket, with size ownership and replay. */
export function usePaneTerminal({ containerRef, paneId, termOptions, visibleRef, registry, onWriteRef }: Options) {
  const termRef = useRef<PaneTerminal | null>(null);
  // Mirrors termRef for render-time consumers (search, LaTeX scan).
  const [term, setTerm] = useState<PaneTerminal | null>(null);
  const fitRef = useRef<(() => void) | null>(null);
  // Takes size ownership of the PTY from another viewer.
  const claimRef = useRef<(() => void) | null>(null);
  // A follower whose server grid doesn't fit its container, so it's clipped.
  const [clipped, setClipped] = useState(false);
  // Live terminal dimensions, shown as "cols×rows" in the title bar. Updated from
  // the fit/resize path so the bar tracks the pane's real grid.
  const [dims, setDims] = useState<{ cols: number; rows: number } | null>(null);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [connection, setConnection] = useState<ConnectionState>('connecting');
  // Keep a newly-created emulator hidden while the server streams its initial
  // checkpoint + journal. Parsing writes is synchronous, but paints happen on
  // animation frames; revealing only after the frame queued by `ready` avoids
  // showing a long replay as rapidly scrolling terminal output.
  const [initialReplayRendered, setInitialReplayRendered] = useState(false);

  useEffect(() => {
    const container = containerRef.current;
    if (!container || !termOptions) return;
    setConnection('connecting');
    setInitialReplayRendered(false);

    let disposed = false;
    let cleanup = () => {};
    const start = async () => {
      let term: PaneTerminal;
      try {
        term = await PaneTerminal.open(container, termOptions);
      } catch (e) {
        if (!disposed) setConnectionError(String(e));
        return;
      }
      if (disposed) {
        term.dispose();
        return;
      }
      cleanup = mount(term);
    };

    const mount = (term: PaneTerminal) => {
      termRef.current = term;
      setTerm(term);
      registry?.set(paneId, term);
      // Also register in the store so actions outside the pane tree (capture-pane
      // in useKeybindings) can read this pane's emulator buffer.
      useStore.getState().registerTerminal(paneId, term);

      let ws: WebSocket | null = null;
      let ready = false;
      let input: InputSocket | null = null;
      const detachInput = () => {
        if (input) terminalInput.detach(paneId, input);
        input = null;
      };

      const syncDims = () =>
        setDims((prev) =>
          prev?.cols === term.cols && prev?.rows === term.rows ? prev : { cols: term.cols, rows: term.rows },
        );

      // Size ownership: the owner's container size drives the PTY. Followers
      // report their size (applied when they claim) but keep the server's grid:
      // a local resize reflows and clears the prompt, and the shell never
      // redraws because the PTY size did not change.
      let owner = true;
      let claimed = false;
      let serverSize: { cols: number; rows: number } | null = null;
      let reported = '';
      const report = (cols: number, rows: number) => {
        const key = `${cols}x${rows}`;
        if (ws?.readyState !== WebSocket.OPEN || !visibleRef.current || key === reported) return;
        reported = key;
        ws.send(JSON.stringify({ type: 'resize', cols, rows }));
      };
      const updateClipped = () => {
        const size = term.proposeDimensions();
        setClipped(!owner && !!serverSize && !!size && (serverSize.cols > size.cols || serverSize.rows > size.rows));
      };
      const adoptServerSize = () => {
        if (serverSize) term.resize(serverSize.cols, serverSize.rows);
        updateClipped();
      };
      const fit = () => {
        const size = term.proposeDimensions();
        if (!size) return;
        if (!owner) adoptServerSize();
        else term.resize(size.cols, size.rows);
        updateClipped();
        report(size.cols, size.rows);
      };
      fitRef.current = fit;
      const claim = () => {
        if (owner || claimed || ws?.readyState !== WebSocket.OPEN) return;
        claimed = true;
        fit();
        ws.send(JSON.stringify({ type: 'claim' }));
      };
      claimRef.current = claim;
      // pointerdown also covers touch, where a swipe sends no mousedown.
      container.addEventListener('keydown', claim, true);
      container.addEventListener('paste', claim, true);
      container.addEventListener('pointerdown', claim, true);
      // wterm ignores pastes without text.
      const onPaste = (e: ClipboardEvent) => {
        const image = clipboardImage(e.clipboardData);
        if (!image) return;
        e.preventDefault();
        e.stopPropagation();
        void pasteImage(paneId, term, image);
      };
      container.addEventListener('paste', onPaste, true);

      let reconnectTimer = 0;
      let attempts = 0;
      let connectRaf = 0;
      let revealRaf = 0;
      const connect = () => {
        if (disposed || ws) return;
        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        const size = term.proposeDimensions() ?? { cols: term.cols, rows: term.rows };
        reported = `${size.cols}x${size.rows}`;
        claimed = false;
        const socket = new WebSocket(
          `${protocol}//${window.location.host}/ws/pane/${paneId}?cols=${size.cols}&rows=${size.rows}`,
        );
        ws = socket;
        socket.binaryType = 'arraybuffer';
        socket.onopen = () => {
          if (disposed || ws !== socket) return;
          attempts = 0;
          setConnectionError(null);
        };
        const messages = paneMessageHandler({
          binary: (bytes) => {
            if (disposed || ws !== socket) return;
            term.write(bytes);
            onWriteRef?.current?.();
          },
          text: (data) => {
            if (disposed || ws !== socket) return;
            try {
              const msg = JSON.parse(data);
              if (msg.type === 'input_mode' && input)
                terminalInput.report(input, { hidden: msg.hidden, inputs: msg.inputs });
              if (msg.type === 'ready') {
                ready = true;
                detachInput();
                input = terminalInput.attach(paneId, (on) => {
                  if (ws === socket && socket.readyState === WebSocket.OPEN)
                    socket.send(JSON.stringify({ type: 'watch_input', on }));
                });
                // wterm paints on the next frame; reveal after it.
                cancelAnimationFrame(revealRaf);
                revealRaf = requestAnimationFrame(() =>
                  requestAnimationFrame(() => {
                    if (disposed || ws !== socket) return;
                    term.scrollToBottom();
                    setInitialReplayRendered(true);
                    setConnection('connected');
                  }),
                );
              }
              if (msg.type === 'error') setConnectionError(msg.message);
              if (msg.type === 'size') {
                // Applied in stream order, so output is parsed at the size it was
                // produced for. The owner's own echo is a no-op.
                serverSize = { cols: msg.cols, rows: msg.rows };
                term.resize(msg.cols, msg.rows);
                updateClipped();
              }
              if (msg.type === 'owner') {
                owner = msg.owner;
                claimed = false;
                if (owner) fit();
                else adoptServerSize();
              }
            } catch {
              /* Unknown protocol frames are not terminal input. */
            }
          },
        });
        socket.onmessage = messages.onMessage;
        socket.onclose = () => {
          messages.dispose();
          if (disposed || ws !== socket) return;
          ws = null;
          ready = false;
          detachInput();
          setConnection('reconnecting');
          reconnectTimer = window.setTimeout(connect, Math.min(1000 * 2 ** attempts++, 10000));
        };
        socket.onerror = () => socket.close();
      };
      const offData = term.onData((bytes) => {
        if (ws?.readyState === WebSocket.OPEN && ready) {
          for (let i = 0; i < bytes.length; i += 16384) {
            ws.send(bytes.slice(i, i + 16384));
            if (input) terminalInput.sent(input);
          }
        }
        const rect = container.getBoundingClientRect();
        const cursor = term.cursor();
        announceWallpaperKeyboardCursor(
          rect.left + ((cursor.col + 0.5) / Math.max(1, term.cols)) * rect.width,
          rect.top + ((cursor.row + 0.5) / Math.max(1, term.rows)) * rect.height,
        );
      });
      const offResize = term.onResize((cols, rows) => setDims({ cols, rows }));
      // While a divider is dragged, each resize would reflow the grid (clearing
      // the prompt) and send the shell a SIGWINCH to redraw it, so the prompt
      // flickers. Fit once when the drag ends instead.
      let fitAfterDrag = false;
      const offDrag = useStore.subscribe((s, prev) => {
        if (s.dividerDragging || !prev.dividerDragging || !fitAfterDrag) return;
        fitAfterDrag = false;
        if (!visibleRef.current) return;
        fit();
        syncDims();
      });
      const observer = new ResizeObserver(() => {
        if (!visibleRef.current) return;
        if (ws && useStore.getState().dividerDragging) {
          fitAfterDrag = true;
          return;
        }
        fit();
        syncDims();
        if (!ws && !reconnectTimer) {
          cancelAnimationFrame(connectRaf);
          connectRaf = requestAnimationFrame(() => {
            if (!visibleRef.current) return;
            fit();
            syncDims();
            connect();
          });
        }
      });
      observer.observe(container);
      // Refit after wterm remeasures cells; its observer runs first.
      term.observeCellSize(observer);

      return () => {
        clearTimeout(reconnectTimer);
        offData();
        offResize();
        offDrag();
        container.removeEventListener('keydown', claim, true);
        container.removeEventListener('paste', claim, true);
        container.removeEventListener('paste', onPaste, true);
        container.removeEventListener('pointerdown', claim, true);
        cancelAnimationFrame(connectRaf);
        cancelAnimationFrame(revealRaf);
        observer.disconnect();
        detachInput();
        ws?.close();
        term.dispose();
        if (registry && registry.get(paneId) === term) registry.delete(paneId);
        useStore.getState().unregisterTerminal(paneId, term);
        termRef.current = null;
        setTerm(null);
        fitRef.current = null;
        claimRef.current = null;
        setClipped(false);
      };
    };

    void start();

    return () => {
      disposed = true;
      cleanup();
    };
    // Rebuilding on termOptions re-themes existing panes after a live config
    // reload; the pane socket replays scrollback on reconnect so content is kept.
  }, [paneId, termOptions, registry]);

  return { term, termRef, fitRef, claimRef, clipped, dims, connection, connectionError, initialReplayRendered };
}
