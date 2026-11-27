// Notices sockets that died without closing, as after the laptop slept or a
// VPN route changed, and reconnects as soon as the network is back rather
// than when TCP gives up.

/** A connect attempt not open by then is stuck (the network was down when it started). */
const CONNECT_TIMEOUT_MS = 8000;
/** How long a probed socket has to answer. */
const PROBE_MS = 3000;
/** A socket silent this long is probed, wake or not: a route can die with nothing to report it. */
const IDLE_MS = 10000;
/** A 1 s timer firing this late means the machine slept. */
const SLEEP_GAP_MS = 5000;

/** The probe every btmux socket that only answers requests understands. */
export const PING = JSON.stringify({ type: 'ping' });

const wakeListeners = new Set<() => void>();
const tickListeners = new Set<() => void>();
let started = false;

function wake() {
  for (const fn of [...wakeListeners]) fn();
}

function start() {
  if (started) return;
  started = true;
  window.addEventListener('online', wake);
  window.addEventListener('pageshow', wake);
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') wake();
  });
  let last = Date.now();
  window.setInterval(() => {
    const now = Date.now();
    // Hidden tabs' timers are throttled, which isn't sleep.
    if (now - last > SLEEP_GAP_MS && document.visibilityState === 'visible') wake();
    last = now;
    for (const fn of [...tickListeners]) fn();
  }, 1000);
}

/** Runs `fn` when connections may have died: after sleep, on `online`, or when the tab is shown. */
export function onWake(fn: () => void): () => void {
  start();
  wakeListeners.add(fn);
  return () => wakeListeners.delete(fn);
}

/**
 * Gives up on `socket` if it doesn't open in time, or if nothing arrives soon
 * after `ping` (null for sockets that send every second on their own), sent
 * on wake and after `IDLE_MS` of silence. `drop` must treat the socket as
 * closed at once, since a dead connection fires `close` only once the closing
 * handshake times out, and reconnect without waiting. Stops watching on close.
 */
export function watchSocket(socket: WebSocket, ping: string | null, drop: () => void): () => void {
  start();
  const opened = Date.now();
  let lastReceived = opened;
  let received = 0;
  let probe = 0;
  const giveUp = () => {
    stop();
    drop();
  };
  const connectTimer = window.setTimeout(() => {
    if (socket.readyState === WebSocket.CONNECTING) giveUp();
  }, CONNECT_TIMEOUT_MS);
  const onMessage = () => {
    received++;
    lastReceived = Date.now();
  };
  const check = () => {
    if (socket.readyState !== WebSocket.OPEN || probe) return;
    const before = received;
    if (ping) socket.send(ping);
    let buffered = socket.bufferedAmount;
    const expire = () => {
      probe = 0;
      if (received !== before || socket.readyState !== WebSocket.OPEN) return;
      // The ping waits behind input (a paste) that is still going out.
      if (socket.bufferedAmount < buffered) {
        buffered = socket.bufferedAmount;
        probe = window.setTimeout(expire, PROBE_MS);
        return;
      }
      giveUp();
    };
    probe = window.setTimeout(expire, PROBE_MS);
  };
  const onWakeUp = () => {
    if (socket.readyState === WebSocket.CONNECTING) {
      // Started before the network was back; a new attempt goes out at once.
      if (Date.now() - opened > 1000) giveUp();
      return;
    }
    check();
  };
  const onTick = () => {
    if (Date.now() - lastReceived > IDLE_MS) check();
  };
  wakeListeners.add(onWakeUp);
  tickListeners.add(onTick);
  const stop = () => {
    clearTimeout(connectTimer);
    clearTimeout(probe);
    wakeListeners.delete(onWakeUp);
    tickListeners.delete(onTick);
    socket.removeEventListener('message', onMessage);
    socket.removeEventListener('close', stop);
  };
  socket.addEventListener('message', onMessage);
  socket.addEventListener('close', stop);
  return stop;
}
