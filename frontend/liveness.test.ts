import { afterAll, beforeAll, expect, test } from 'bun:test';

// A manual clock: timers run only as `advance` passes them.
let now = 0;
let nextId = 1;
const timers = new Map<number, { at: number; every: number | null; fn: () => void }>();
function advance(ms: number) {
  const end = now + ms;
  for (;;) {
    let due: [number, { at: number; every: number | null; fn: () => void }] | undefined;
    for (const entry of timers) if (entry[1].at <= end && (!due || entry[1].at < due[1].at)) due = entry;
    if (!due) break;
    const [id, timer] = due;
    now = timer.at;
    if (timer.every === null) timers.delete(id);
    else timer.at += timer.every;
    timer.fn();
  }
  now = end;
}
const schedule = (fn: () => void, ms: number, every: boolean) => {
  const id = nextId++;
  timers.set(id, { at: now + ms, every: every ? ms : null, fn });
  return id;
};

class FakeSocket extends EventTarget {
  static CONNECTING = 0;
  static OPEN = 1;
  readyState = 1;
  bufferedAmount = 0;
  sent: string[] = [];
  send(data: string) {
    this.sent.push(data);
  }
  receive() {
    this.dispatchEvent(new Event('message'));
  }
}

let liveness: typeof import('./src/lib/liveness');
const globals = ['window', 'document', 'WebSocket', 'clearTimeout'] as const;
const saved = globals.map((name) => (globalThis as any)[name]);
const realNow = Date.now;
afterAll(() => {
  Date.now = realNow;
  globals.forEach((name, i) => ((globalThis as any)[name] = saved[i]));
});
beforeAll(async () => {
  Date.now = () => now;
  const win = new EventTarget() as any;
  win.setTimeout = (fn: () => void, ms: number) => schedule(fn, ms, false);
  win.setInterval = (fn: () => void, ms: number) => schedule(fn, ms, true);
  (globalThis as any).window = win;
  (globalThis as any).clearTimeout = (id: number) => timers.delete(id);
  const doc = new EventTarget() as any;
  doc.visibilityState = 'visible';
  (globalThis as any).document = doc;
  (globalThis as any).WebSocket = FakeSocket;
  liveness = await import('./src/lib/liveness');
});

function watch(ping: string | null) {
  const socket = new FakeSocket();
  let dropped = false;
  liveness.watchSocket(socket as unknown as WebSocket, ping, () => (dropped = true));
  return { socket, dropped: () => dropped };
}

test('a silent socket is probed and dropped without a wake', () => {
  const { socket, dropped } = watch(liveness.PING);
  advance(10_500);
  expect(socket.sent).toEqual([]);
  advance(1000);
  expect(socket.sent).toEqual([liveness.PING]);
  advance(2400);
  expect(dropped()).toBe(false);
  advance(200);
  expect(dropped()).toBe(true);
});

test('an answered probe keeps the socket', () => {
  const { socket, dropped } = watch(liveness.PING);
  advance(11_500);
  expect(socket.sent.length).toBe(1);
  socket.receive();
  advance(12_000);
  expect(socket.sent.length).toBe(2);
  socket.receive();
  advance(5000);
  expect(dropped()).toBe(false);
});

test('a socket without pings has a receive deadline', () => {
  const { dropped, socket } = watch(null);
  for (let i = 0; i < 20; i++) {
    advance(1000);
    socket.receive();
  }
  expect(dropped()).toBe(false);
  advance(15_000);
  expect(dropped()).toBe(true);
});

test('a probe behind input still going out waits for it', () => {
  const { socket, dropped } = watch(liveness.PING);
  socket.bufferedAmount = 1_000_000;
  window.dispatchEvent(new Event('online'));
  expect(socket.sent).toEqual([liveness.PING]);
  advance(1000);
  socket.bufferedAmount = 500_000;
  advance(2100);
  expect(dropped()).toBe(false);
  advance(3000);
  expect(dropped()).toBe(true);
});
