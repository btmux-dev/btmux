import { useEffect, useRef, useState } from 'react';
import type { ConnectionState } from '../lib/connectionState';
import { onWake, watchSocket } from '../lib/liveness';

export interface SysStats {
  cpu: number[];
  mem_used: number;
  mem_total: number;
  net_rx: number;
  net_tx: number;
  /** The server's home volume; null when unknown or stale. */
  disk: { total: number; available: number } | null;
}

const WS_URL = `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws/sysstat`;
const RECONNECT_MS = 2000;

export function useSysStats(): { stats: SysStats | null; state: ConnectionState } {
  const [stats, setStats] = useState<SysStats | null>(null);
  const [state, setState] = useState<ConnectionState>('connecting');
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;

    function connect() {
      if (!mountedRef.current) return;
      timerRef.current = null;
      const ws = new WebSocket(WS_URL);
      wsRef.current = ws;

      ws.onopen = () => setState('connected');

      ws.onmessage = (e) => {
        try {
          setStats(JSON.parse(e.data) as SysStats);
        } catch {}
      };

      const lost = () => {
        if (!mountedRef.current || wsRef.current !== ws) return;
        wsRef.current = null;
        setState('reconnecting');
        timerRef.current = setTimeout(connect, RECONNECT_MS);
      };
      ws.onclose = lost;
      // Frames come every second, so no ping is needed.
      watchSocket(ws, null, () => {
        lost();
        ws.close();
        if (timerRef.current) clearTimeout(timerRef.current);
        connect();
      });
    }

    connect();
    const offWake = onWake(() => {
      if (!mountedRef.current || !timerRef.current) return;
      clearTimeout(timerRef.current);
      connect();
    });

    return () => {
      mountedRef.current = false;
      offWake();
      if (timerRef.current) clearTimeout(timerRef.current);
      wsRef.current?.close();
    };
  }, []);

  return { stats, state };
}
