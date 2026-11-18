// Run with `just test-frontend` (bun test).
import { expect, test } from 'bun:test';
import { HOLD_MS, KeyCaster, type Entry, type KeyInfo } from './src/lib/keyCast';
import type { InputSocket } from './src/lib/terminalInput';

/** Terminal sockets the test drives: input it sends and the server's reports. */
function fakeInputs() {
  const sockets = new Map<string, InputSocket>();
  const listeners = new Set<() => void>();
  return {
    socket: (id: string) => sockets.get(id) ?? null,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    open(id: string) {
      const socket: InputSocket = { sent: 0, reports: [], closed: false, watch: () => {} };
      sockets.set(id, socket);
      return socket;
    },
    report(socket: InputSocket, hidden: boolean | null, inputs = socket.sent) {
      socket.reports.push({ inputs, hidden });
      for (const listener of listeners) listener();
    },
    close(socket: InputSocket) {
      socket.closed = true;
      for (const listener of listeners) listener();
    },
  };
}

function setup(mac = true) {
  let now = 0;
  const inputs = fakeInputs();
  const caster = new KeyCaster(mac, inputs, () => now);
  const press = (
    key: string,
    mods: Partial<KeyInfo> = {},
    note?: Parameters<KeyCaster['note']>[1],
    terminal: { id: string; socket: InputSocket } | null = null,
  ) => {
    const source = {};
    const code = /^[a-z]$/i.test(key) ? `Key${key.toUpperCase()}` : '';
    caster.key(source, {
      key,
      code,
      ctrlKey: false,
      altKey: false,
      shiftKey: false,
      metaKey: false,
      altGraph: false,
      isComposing: false,
      ...mods,
    }, terminal?.id ?? null);
    if (note) caster.note(source, note);
    // The terminal sends what isn't a binding.
    if (terminal && !note) terminal.socket.sent += 1;
    caster.flush();
    now += 100;
  };
  const wait = (ms: number) => (now += ms);
  return { caster, press, wait, inputs };
}

function describe(entry: Entry): string {
  const cap = (c: { mods: string[]; key: string }) => [...c.mods, c.key].join('');
  switch (entry.kind) {
    case 'text':
      return `text:${entry.tokens.map((t) => t.text + (t.count > 1 ? `×${t.count}` : '')).join('')}`;
    case 'chord':
      return `chord:${cap(entry.cap)}${entry.count > 1 ? `×${entry.count}` : ''}`;
    case 'hidden':
      return `hidden:${entry.reason}`;
    case 'binding':
      return [
        'binding:',
        entry.prefix ? `${cap(entry.prefix)} ` : '',
        entry.keys.map(cap).join(' '),
        entry.count > 1 ? `×${entry.count}` : '',
        entry.pending ? ' (pending)' : '',
        entry.canceled ? ' (canceled)' : '',
        entry.label ? ` = ${entry.label}` : '',
      ].join('');
  }
}

const shown = (caster: KeyCaster) => caster.snapshot().map(describe);

test('typing merges into a run, with named keys compressed', () => {
  const { caster, press } = setup();
  for (const key of 'ls -l') press(key);
  press('Backspace');
  press('Backspace');
  press('Enter');
  expect(shown(caster)).toEqual(['text:ls␣-l⌫×2⏎']);
});

test('a pause starts a new run', () => {
  const { caster, press, wait } = setup();
  press('a');
  wait(2000);
  press('b');
  expect(shown(caster)).toEqual(['text:a', 'text:b']);
});

test('the prefix waits for its key and shows the action', () => {
  const { caster, press } = setup();
  press('a', { ctrlKey: true }, { kind: 'prefix' });
  expect(shown(caster)).toEqual(['binding:⌃A  (pending)']);
  press('c', {}, { kind: 'binding', label: 'New window' });
  expect(shown(caster)).toEqual(['binding:⌃A c = New window']);
  press('a', { ctrlKey: true }, { kind: 'prefix' });
  press('C', { shiftKey: true }, { kind: 'binding', label: 'New session' });
  press('a', { ctrlKey: true, shiftKey: true });
  expect(shown(caster).slice(1)).toEqual(['binding:⌃A C = New session', 'chord:⌃⇧A']);
});

test('a prefix without a key shows as canceled', () => {
  const { caster, press } = setup();
  press('a', { ctrlKey: true }, { kind: 'prefix' });
  caster.cancelPrefix();
  expect(shown(caster)).toEqual(['binding:⌃A  (canceled)']);
});

test('repeats count on the binding they repeat', () => {
  const { caster, press } = setup();
  press('a', { ctrlKey: true }, { kind: 'prefix' });
  press('ArrowUp', { ctrlKey: true }, { kind: 'binding', label: 'Resize pane up' });
  press('ArrowUp', { ctrlKey: true }, { kind: 'repeat', label: 'Resize pane up' });
  press('ArrowUp', { ctrlKey: true }, { kind: 'repeat', label: 'Resize pane up' });
  expect(shown(caster)).toEqual(['binding:⌃A ⌃↑×3 = Resize pane up']);
});

test('unbound keys after the prefix say so', () => {
  const { caster, press } = setup();
  press('a', { ctrlKey: true }, { kind: 'prefix' });
  press('y', {}, { kind: 'binding', label: null });
  const entry = caster.snapshot()[0];
  expect(entry.kind === 'binding' && entry.label).toBe(null);
});

test('chords count repeats; chords mode drops typing', () => {
  const { caster, press } = setup(false);
  caster.mode = 'chords';
  press('c', { ctrlKey: true });
  press('c', { ctrlKey: true });
  press('x');
  press('Enter');
  press('Tab', { altKey: true, shiftKey: true });
  expect(shown(caster)).toEqual(['chord:CtrlC×2', 'chord:AltShift⇥']);
});

test('chords mode shows no hidden entry for plain typing', () => {
  const { caster, press, inputs } = setup();
  caster.mode = 'chords';
  const socket = inputs.open('pane');
  const pane = { id: 'pane', socket };
  press('x', {}, undefined, pane);
  press('c', { ctrlKey: true }, undefined, pane);
  inputs.report(socket, null);
  expect(shown(caster)).toEqual(['hidden:unknown']);
  expect(caster.snapshot().length).toBe(1);
});

test('Option text on macOS is typing, not a chord', () => {
  const { caster, press } = setup();
  // German layout: Option+L types "@".
  caster.key({}, keyInfo({ key: '@', code: 'KeyL', altKey: true }), null);
  caster.flush();
  press('x', { altKey: true });
  expect(shown(caster)).toEqual(['text:@', 'chord:⌥X']);
});

test('terminal input waits for the server to say it was shown', () => {
  const { caster, press, inputs } = setup();
  const socket = inputs.open('pane');
  const pane = { id: 'pane', socket };
  press('l', {}, undefined, pane);
  press('s', {}, undefined, pane);
  expect(shown(caster)).toEqual([]);
  inputs.report(socket, false, 1);
  expect(shown(caster)).toEqual(['text:l']);
  inputs.report(socket, false, 2);
  expect(shown(caster)).toEqual(['text:ls']);
});

test('input a report says was hidden is hidden, bindings still show in order', () => {
  const { caster, press, inputs } = setup();
  const socket = inputs.open('pane');
  const pane = { id: 'pane', socket };
  for (const key of 'hunter2') press(key, {}, undefined, pane);
  press('a', { ctrlKey: true }, { kind: 'prefix' }, pane);
  press('o', {}, { kind: 'binding', label: 'Next pane' }, pane);
  press('Enter', {}, undefined, pane);
  // Held behind the password, which the server hasn't confirmed.
  expect(shown(caster)).toEqual([]);
  // One report for all of it: the Enter turned echo back on, but the password hid.
  inputs.report(socket, true);
  expect(shown(caster)).toEqual(['hidden:password', 'binding:⌃A o = Next pane', 'hidden:password']);
});

test('input is hidden when the server cannot tell, the socket closed, or it never answered', () => {
  const { caster, press, wait, inputs } = setup();
  const windows = inputs.open('windows');
  press('a', {}, undefined, { id: 'windows', socket: windows });
  inputs.report(windows, null);
  const closed = inputs.open('closed');
  press('b', {}, undefined, { id: 'closed', socket: closed });
  inputs.close(closed);
  const silent = inputs.open('silent');
  press('c', {}, undefined, { id: 'silent', socket: silent });
  expect(shown(caster)).toEqual(['hidden:unknown']);
  wait(HOLD_MS);
  caster.drain();
  expect(shown(caster)).toEqual(['hidden:unknown']);
  expect(caster.snapshot().length).toBe(1);
  // A terminal btmux doesn't know.
  press('d', {}, undefined, { id: 'gone', socket: inputs.open('other') });
  expect(shown(caster)).toEqual(['hidden:unknown']);
});

test('text without a keydown goes through the same check', () => {
  const { caster, inputs } = setup();
  const socket = inputs.open('pane');
  caster.text('secret', 'pane');
  socket.sent += 1;
  caster.flush();
  expect(shown(caster)).toEqual([]);
  inputs.report(socket, true);
  caster.text('ok', null);
  caster.flush();
  expect(shown(caster)).toEqual(['hidden:password', 'text:ok']);
});

test('canceling the prefix tells subscribers', () => {
  const { caster, press } = setup();
  let changes = 0;
  caster.subscribe(() => changes++);
  press('a', { ctrlKey: true }, { kind: 'prefix' });
  const before = changes;
  caster.cancelPrefix();
  expect(changes).toBeGreaterThan(before);
});

test('a synthetic copy replaces the key it copies', () => {
  const { caster } = setup();
  const original = {};
  caster.key(original, keyInfo({ key: 'c' }), null);
  caster.key({}, keyInfo({ key: 'c', ctrlKey: true }), null, true);
  caster.flush();
  expect(shown(caster)).toEqual(['chord:⌃C']);
});

test('modifiers alone and compositions show nothing', () => {
  const { caster, press } = setup();
  press('Shift', { shiftKey: true });
  press('Dead');
  press('a', { isComposing: true });
  expect(shown(caster)).toEqual([]);
});

test('entries expire, except a pending prefix', () => {
  const { caster, press, wait } = setup();
  press('x');
  press('a', { ctrlKey: true }, { kind: 'prefix' });
  wait(5000);
  caster.expire();
  expect(shown(caster)).toEqual(['binding:⌃A  (pending)']);
});

function keyInfo(info: Partial<KeyInfo> & { key: string }): KeyInfo {
  return {
    code: '',
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    metaKey: false,
    altGraph: false,
    isComposing: false,
    ...info,
  };
}
