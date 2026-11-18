// Run with `just test-frontend` (bun test).
import { expect, test } from 'bun:test';
import { KeyCaster, type Entry, type KeyInfo } from './src/lib/keyCast';

function setup(mac = true) {
  let now = 0;
  const caster = new KeyCaster(mac, () => now);
  const press = (key: string, mods: Partial<KeyInfo> = {}, note?: Parameters<KeyCaster['note']>[1]) => {
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
      hidden: false,
      ...mods,
    });
    if (note) caster.note(source, note);
    caster.flush();
    now += 100;
  };
  const wait = (ms: number) => (now += ms);
  return { caster, press, wait };
}

function describe(entry: Entry): string {
  const cap = (c: { mods: string[]; key: string }) => [...c.mods, c.key].join('');
  switch (entry.kind) {
    case 'text':
      return `text:${entry.tokens.map((t) => t.text + (t.count > 1 ? `×${t.count}` : '')).join('')}`;
    case 'chord':
      return `chord:${cap(entry.cap)}${entry.count > 1 ? `×${entry.count}` : ''}`;
    case 'hidden':
      return 'hidden';
    case 'binding':
      return [
        'binding:',
        entry.prefix ? `${cap(entry.prefix)} ` : '',
        entry.keys.map(cap).join(' '),
        entry.count > 1 ? `×${entry.count}` : '',
        entry.pending ? ' (pending)' : '',
        entry.cancelled ? ' (cancelled)' : '',
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

test('a prefix without a key shows as cancelled', () => {
  const { caster, press } = setup();
  press('a', { ctrlKey: true }, { kind: 'prefix' });
  caster.cancelPrefix();
  expect(shown(caster)).toEqual(['binding:⌃A  (cancelled)']);
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

test('Option text on macOS is typing, not a chord', () => {
  const { caster, press } = setup();
  // German layout: Option+L types "@".
  caster.key({}, keyInfo({ key: '@', code: 'KeyL', altKey: true }));
  caster.flush();
  press('x', { altKey: true });
  expect(shown(caster)).toEqual(['text:@', 'chord:⌥X']);
});

test('keys to a password prompt are hidden, but bindings still show', () => {
  const { caster, press } = setup();
  for (const key of 'hunter2') press(key, { hidden: true });
  press('a', { ctrlKey: true, hidden: true }, { kind: 'prefix' });
  press('o', { hidden: true }, { kind: 'binding', label: 'Next pane' });
  caster.text('composed', true);
  expect(shown(caster)).toEqual(['hidden', 'binding:⌃A o = Next pane', 'hidden']);
});

test('a synthetic copy replaces the key it copies', () => {
  const { caster } = setup();
  const original = {};
  caster.key(original, keyInfo({ key: 'c' }));
  caster.key({}, keyInfo({ key: 'c', ctrlKey: true }), true);
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
    hidden: false,
    ...info,
  };
}
