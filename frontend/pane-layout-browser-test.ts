/** Pane layout regressions: divider drags and layout motion, against the isolated browser-test stack. */
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { waitForAsync } from './browser-test-utils';

const url = process.env.BTMUX_TEST_URL ?? 'http://localhost:5173';
const token = process.env.BTMUX_AUTH_TOKEN;
assert(token, 'BTMUX_AUTH_TOKEN is required');
const browser = await chromium.launch({ headless: true, channel: process.env.BTMUX_TEST_BROWSER });
try {
  const page = await browser.newPage({ viewport: { width: 1400, height: 900 } });
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  // The control socket's replies can be held, and new pane connections slowed.
  let holdReplies = false;
  const held: (string | Buffer)[] = [];
  let control: { send: (message: string | Buffer) => void } | null = null;
  await page.routeWebSocket(/\/ws\/control/, (ws) => {
    control = ws;
    ws.connectToServer().onMessage((message) => (holdReplies ? held.push(message) : ws.send(message)));
  });
  let slowPanes = false;
  await page.routeWebSocket(/\/ws\/pane\//, async (ws) => {
    if (slowPanes) await new Promise((resolve) => setTimeout(resolve, 1500));
    ws.connectToServer();
  });
  await page.goto(url);
  await page.getByLabel('Access token').fill(token);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await waitForAsync(page, async () => {
    const { useStore } = await import('/src/state/store.tsx' as string);
    return useStore.getState().terminals.size > 0;
  });

  /** The shown window: its session, index, and pane ids. */
  const shown = () =>
    page.evaluate(async () => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      const session = useStore.getState().allSessions[0];
      const window = session.windows[session.active_window];
      return {
        sessionId: session.id as string,
        index: session.active_window as number,
        windowId: window.id as string,
        panes: window.panes.map((pane: { id: string }) => pane.id) as string[],
      };
    });
  const switchWindow = (sessionId: string, index: number) =>
    page.request.post(`${url}/api/sessions/${sessionId}/windows/switch`, {
      headers: { Authorization: `Bearer ${token}` },
      data: { index },
    });
  const box = (id: string) => page.locator(`[data-pane-id="${id}"][style]`);
  const width = async (id: string) => (await box(id).boundingBox())!.width;
  const cols = (id: string) =>
    page.evaluate(async (id) => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      return useStore.getState().terminals.get(id).cols as number;
    }, id);
  /** Counts `id`'s grid resizes from now; read with `resizes(id)`. */
  const countResizes = (id: string) =>
    page.evaluate(async (id) => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      const counts = ((window as any).__resizes ??= {});
      const offs = ((window as any).__resizeOffs ??= {});
      offs[id]?.();
      counts[id] = 0;
      offs[id] = useStore
        .getState()
        .terminals.get(id)
        .onResize(() => counts[id]++);
    }, id);
  const resizes = (id: string) => page.evaluate((id) => (window as any).__resizes[id] as number, id);
  const dragEnded = () =>
    waitForAsync(page, async () => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      return !useStore.getState().dividerDragging;
    });
  // Layout motion uses the Web Animations API; CSS animations (a connecting
  // pulse, the focus ring's bloom) aren't it.
  const settled = () =>
    page.evaluate(() =>
      Promise.all(
        document
          .getAnimations()
          .filter((animation) => animation.constructor === Animation && animation.playState === 'running')
          .map((animation) => animation.finished.catch(() => {})),
      ),
    );
  const split = async (key: '%' | '"') => {
    const before = (await shown()).panes.length;
    await page.keyboard.press('Control+a');
    await page.keyboard.press(key);
    await waitForAsync(
      page,
      async (before) => {
        const { useStore } = await import('/src/state/store.tsx' as string);
        const session = useStore.getState().allSessions[0];
        const panes = session.windows[session.active_window].panes;
        return (
          panes.length > before && panes.every((pane: { id: string }) => useStore.getState().terminals.has(pane.id))
        );
      },
      before,
    );
  };
  /** Open a window with one pane and focus it. */
  const newWindow = async () => {
    const before = await shown();
    await page.keyboard.press('Control+a');
    await page.keyboard.press('c');
    const id = await waitForAsync(
      page,
      async (before) => {
        const { useStore } = await import('/src/state/store.tsx' as string);
        const session = useStore.getState().allSessions[0];
        const window = session.windows[session.active_window];
        const id = window.panes[0]?.id;
        return window.id !== before && window.panes.length === 1 && useStore.getState().terminals.has(id)
          ? (id as string)
          : null;
      },
      before.windowId,
    );
    await box(id!).locator('[data-pane-terminal]').click();
    return id!;
  };
  // A pause takes effect at the next frame, once `ready` resolves.
  const pauseMotion = () =>
    page.evaluate(() =>
      Promise.all(
        document
          .getAnimations()
          .filter((animation) => animation.constructor === Animation)
          .map((animation) => {
            animation.pause();
            return animation.ready;
          }),
      ),
    );
  const divider = async () => (await page.locator('[style*="col-resize"]').last().boundingBox())!;

  // A split animates the box while the terminal resizes once, at the start,
  // and the new pane shows no connection label. Dividers sit at their final
  // positions, so they take no input until the boxes get there.
  await page.locator('[data-pane-terminal]').first().click();
  const left = await newWindow();
  const fullWidth = await width(left);
  await countResizes(left);
  const frames = box(left).evaluate(
    (el) =>
      new Promise<{ widths: number[]; labels: number; hits: number; settling: number }>((resolve) => {
        const result = { widths: [] as number[], labels: 0, hits: 0, settling: 0 };
        const end = performance.now() + 800;
        const tick = () => {
          result.widths.push(el.getBoundingClientRect().width);
          if (document.querySelector('[data-pane-id] > [role="status"]')) result.labels++;
          const divider = document.querySelector<HTMLElement>('[style*="col-resize"][data-settling]');
          if (divider) {
            result.settling++;
            const r = divider.getBoundingClientRect();
            if (document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2) === divider) result.hits++;
          }
          if (performance.now() < end) requestAnimationFrame(tick);
          else resolve(result);
        };
        requestAnimationFrame(tick);
      }),
  );
  await split('%');
  const splitFrames = await frames;
  await settled();
  const halfWidth = await width(left);
  assert.equal(await resizes(left), 1, 'the terminal resizes once');
  assert.equal(splitFrames.labels, 0, 'the new pane shows no connection label');
  assert(
    splitFrames.widths.some((w) => w < fullWidth - 50 && w > halfWidth + 50),
    `the box animates: ${splitFrames.widths.map(Math.round)}`,
  );
  assert(splitFrames.settling > 0 && splitFrames.hits === 0, 'the divider takes no input while boxes move');
  const settledDivider = await divider();
  assert.equal(
    await page.evaluate(
      ([x, y]) => !!document.elementFromPoint(x, y)?.matches('[style*="col-resize"]'),
      [settledDivider.x + settledDivider.width / 2, settledDivider.y + settledDivider.height / 2],
    ),
    true,
    'the divider takes input once they settle',
  );
  console.log('PASS a split animates, resizing the terminal once, and enables its divider once settled');

  // Dragging keeps the grid until the release, which the layout keeps while
  // the server confirms it, and the release resizes the grid once even where
  // the pointer moved since its last move event.
  const startWidth = await width(left);
  const startCols = await cols(left);
  let d = await divider();
  const y = d.y + d.height / 2;
  await countResizes(left);
  await page.mouse.move(d.x + d.width / 2, y);
  await page.mouse.down();
  for (let i = 1; i <= 10; i++) {
    await page.mouse.move(d.x + d.width / 2 - i * 20, y);
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(resolve)));
  }
  assert((await width(left)) < startWidth - 150, 'the pane follows the drag');
  assert.equal(await cols(left), startCols, 'the grid keeps its size while dragging');
  holdReplies = true;
  const releaseX = d.x + d.width / 2 - 260;
  const widths = box(left).evaluate(
    (el) =>
      new Promise<number[]>((resolve) => {
        const seen: number[] = [];
        const end = performance.now() + 300;
        const tick = () => {
          seen.push(el.getBoundingClientRect().width);
          if (performance.now() < end) requestAnimationFrame(tick);
          else resolve(seen);
        };
        requestAnimationFrame(tick);
      }),
  );
  await page.locator('[style*="col-resize"]').last().dispatchEvent('pointerup', { clientX: releaseX, clientY: y });
  await page.mouse.up();
  const released = await widths;
  holdReplies = false;
  for (const message of held.splice(0)) control!.send(message);
  assert(Math.max(...released) < startWidth - 200, 'the released layout never jumps back');
  await dragEnded();
  await page.waitForTimeout(300);
  assert.equal(await resizes(left), 1, 'the release resizes the grid once');
  assert((await cols(left)) < startCols);
  assert((await width(left)) < startWidth - 200, 'the server keeps the released layout');
  console.log('PASS a divider drag resizes the terminal once, at its release, and never jumps back');

  // A canceled drag puts the layout back without resizing.
  const beforeCancel = await width(left);
  d = await divider();
  await countResizes(left);
  await page.mouse.move(d.x + d.width / 2, y);
  await page.mouse.down();
  await page.mouse.move(d.x + d.width / 2 + 120, y, { steps: 4 });
  await page.locator('[style*="col-resize"]').last().dispatchEvent('pointercancel');
  await page.mouse.up();
  await dragEnded();
  await page.waitForTimeout(300);
  assert.equal(Math.round(await width(left)), Math.round(beforeCancel));
  assert.equal(await resizes(left), 0, 'a canceled drag never resizes the grid');
  console.log('PASS a canceled drag keeps the layout and the grid');

  // A drag whose window stops being shown ends, rather than leaving terminals
  // unfitted.
  const dragWindow = await shown();
  d = await divider();
  await page.mouse.move(d.x + d.width / 2, y);
  await page.mouse.down();
  await page.mouse.move(d.x + d.width / 2 - 150, y, { steps: 4 });
  await switchWindow(dragWindow.sessionId, 0);
  await dragEnded();
  await page.mouse.up();
  await switchWindow(dragWindow.sessionId, dragWindow.index);
  await waitForAsync(
    page,
    async (id) =>
      (await import('/src/state/store.tsx' as string)).useStore
        .getState()
        .allSessions[0].windows.some((w: { id: string }) => w.id === id),
    dragWindow.windowId,
  );
  await page.waitForFunction((id) => !!document.querySelector(`[data-pane-id="${id}"][style*="display: flex"]`), left);
  assert.equal(Math.round(await width(left)), Math.round(beforeCancel), 'the drag left no ratio behind');
  const narrowCols = await cols(left);
  await page.setViewportSize({ width: 1800, height: 900 });
  await waitForAsync(
    page,
    async ([id, before]) => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      return useStore.getState().terminals.get(id).cols > before;
    },
    [left, narrowCols] as const,
  );
  await page.setViewportSize({ width: 1400, height: 900 });
  console.log('PASS a drag ends when its window stops being shown');

  // A change during a motion starts from where the boxes show.
  await settled();
  await countResizes(left);
  const right = (await shown()).panes.find((id) => id !== left)!;
  await box(right).locator('[data-pane-terminal]').click();
  await page.keyboard.type('exit');
  await page.keyboard.press('Enter');
  // Pause the close's motion partway, then split the growing pane.
  await waitForAsync(page, () => document.getAnimations().some((animation) => animation.constructor === Animation));
  await page.waitForTimeout(40);
  await pauseMotion();
  const paused = await width(left);
  assert(paused > beforeCancel + 50 && paused < fullWidth - 50, `paused mid-motion at ${paused}`);
  // Right after the commit that adds a pane, its animations have started.
  const interrupted = box(left).evaluate(
    (el) =>
      new Promise<number>((resolve) => {
        const observer = new MutationObserver(() => {
          if (document.querySelectorAll('[data-pane-id][style*="display: flex"]').length < 2) return;
          observer.disconnect();
          resolve(el.getBoundingClientRect().width);
        });
        observer.observe(document.body, { childList: true, subtree: true, attributeFilter: ['style'] });
        setTimeout(() => resolve(-1), 3000);
      }),
  );
  await box(left).locator('[data-pane-terminal]').click();
  await page.keyboard.press('Control+a');
  await page.keyboard.press('"');
  const firstFrame = await interrupted;
  assert(Math.abs(firstFrame - paused) < 2, `the next motion starts at ${firstFrame}, not ${paused}`);
  // The closed pane's box, paused with the rest.
  await page.evaluate(() => document.getAnimations().forEach((a) => a.playState === 'paused' && a.play()));
  await settled();
  console.log('PASS a change during a motion starts from where the boxes show');

  // Closing a pane shrinks its empty box away while the other grows; switching
  // windows meanwhile takes it along.
  const bottom = (await shown()).panes.find((id) => id !== left)!;
  await box(bottom).locator('[data-pane-terminal]').click();
  await page.keyboard.type('exit');
  await page.keyboard.press('Enter');
  await page.locator('[data-pane-ghost]').waitFor();
  await page.locator('[data-pane-ghost]').waitFor({ state: 'detached' });
  await settled();
  assert.equal(Math.round(await width(left)), Math.round(fullWidth));
  await split('%');
  await settled();
  const closing = (await shown()).panes.find((id) => id !== left)!;
  await box(closing).locator('[data-pane-terminal]').click();
  await page.keyboard.type('exit');
  await page.keyboard.press('Enter');
  await page.locator('[data-pane-ghost]').waitFor();
  await pauseMotion();
  const ghostWindow = await shown();
  await switchWindow(ghostWindow.sessionId, 0);
  await page.locator('[data-pane-ghost]').waitFor({ state: 'detached' });
  await page.evaluate(() => document.getAnimations().forEach((a) => a.playState === 'paused' && a.play()));
  await switchWindow(ghostWindow.sessionId, ghostWindow.index);
  await page.waitForFunction((id) => !!document.querySelector(`[data-pane-id="${id}"][style*="display: flex"]`), left);
  console.log("PASS a close's box shrinks away, and only over its window");

  // A slow connection still says so.
  await settled();
  slowPanes = true;
  await page.keyboard.press('Control+a');
  await page.keyboard.press('"');
  const connecting = page.locator('[data-pane-id] > [role="status"]', { hasText: 'Connecting…' });
  await connecting.waitFor();
  slowPanes = false;
  await connecting.waitFor({ state: 'detached' });
  await settled();
  console.log('PASS a slow pane connection shows its label');

  // Reduced motion changes the layout at once.
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await split('%');
  assert.equal(
    await page.evaluate(() => document.getAnimations().filter((a) => a.constructor === Animation).length),
    0,
  );
  console.log('PASS reduced motion changes the layout at once');

  assert.deepEqual(errors, []);
} finally {
  await browser.close();
}
