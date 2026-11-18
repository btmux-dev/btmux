/** The key overlay (status bar keyboard button) against the isolated browser-test stack. */
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { waitForAsync } from './browser-test-utils';

const url = process.env.BTMUX_TEST_URL ?? 'http://localhost:5173';
const token = process.env.BTMUX_AUTH_TOKEN;
assert(token, 'BTMUX_AUTH_TOKEN is required');

const browser = await chromium.launch({ headless: true, channel: process.env.BTMUX_TEST_BROWSER });
try {
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 } });
  const page = await context.newPage();
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto(url);
  await page.getByLabel('Access token').fill(token);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await waitForAsync(page, async () => {
    const { useStore } = await import('/src/state/store.tsx' as string);
    return useStore.getState().terminals.size > 0;
  });
  const paneId = await page.evaluate(async () => {
    const { useStore } = await import('/src/state/store.tsx' as string);
    return [...useStore.getState().terminals.keys()][0] as string;
  });
  const api = (path: string) => `${url}/api${path}`;
  const run = (command: string) =>
    page.request.post(api(`/panes/${paneId}/input`), { data: { text: command, keys: ['Enter'] } });
  const hidden = (id: string) =>
    page.evaluate(async (id) => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      return useStore.getState().hiddenInput.has(id) as boolean;
    }, id);
  const waitForHidden = async (want: boolean) => {
    const deadline = Date.now() + 10_000;
    while ((await hidden(paneId)) !== want) {
      assert(Date.now() < deadline, `hidden input never became ${want}`);
      await page.waitForTimeout(50);
    }
  };
  const entries = page.locator('[data-key-overlay-entry]');
  const overlayText = async () => (await entries.allTextContents()).join('\n');
  const waitForEntry = async (text: string) => {
    await page.waitForFunction(
      (text) =>
        [...document.querySelectorAll('[data-key-overlay-entry]')].some((entry) => entry.textContent?.includes(text)),
      text,
      { timeout: 10_000 },
    );
  };

  // Off by default.
  await page.locator('[data-pane-terminal]').first().click();
  await page.keyboard.type('x');
  await page.waitForTimeout(100);
  assert.equal(await entries.count(), 0, 'nothing shows while off');
  await page.keyboard.press('Backspace');

  await page.getByRole('button', { name: 'Show pressed keys' }).click();
  await page.locator('[data-pane-terminal]').first().click();
  await page.keyboard.type('echo hi');
  await page.keyboard.press('Enter');
  await waitForEntry('echo␣hi⏎');
  console.log('PASS typing shows as one run');

  // The prefix waits for its key, then shows the binding it ran.
  await page.keyboard.press('Control+a');
  await waitForEntry('prefix…');
  await page.keyboard.press('z');
  await waitForEntry('Zoom pane');
  assert(!(await overlayText()).includes('prefix…'), 'the prefix no longer waits');
  await page.keyboard.press('Control+a');
  await page.keyboard.press('z');
  console.log('PASS prefix sequences show their action');

  // A password prompt hides what's typed, and what's typed after it shows again.
  await run("stty -echo; printf 'Password: '; read secret; stty echo; echo; echo got-$secret");
  await waitForHidden(true);
  await page.keyboard.type('hunter2');
  await waitForEntry('Password input hidden');
  assert(!(await overlayText()).includes('hunter2'), 'the password never shows');
  await page.keyboard.press('Enter');
  await waitForHidden(false);
  await page.keyboard.type('visible');
  await waitForEntry('visible');
  assert(!(await overlayText()).includes('hunter2'), 'the password never shows');
  await page.keyboard.press('Control+u');
  console.log('PASS password prompts hide their input');

  // It stays above modal dialogs: it shows itself again, last in the top layer.
  await page.evaluate(() => {
    const host = document.querySelector('[data-key-overlay-entry]')?.parentElement;
    (window as unknown as { promoted: number }).promoted = 0;
    host?.addEventListener('toggle', (e) => {
      if ((e as ToggleEvent).newState === 'open') (window as unknown as { promoted: number }).promoted += 1;
    });
  });
  await page.keyboard.press('Control+a');
  await page.keyboard.press('?');
  await page.locator('dialog:modal').first().waitFor();
  await waitForEntry('List keys');
  await page.waitForFunction(() => (window as unknown as { promoted: number }).promoted > 0);
  if (process.env.KEY_OVERLAY_SCREENSHOT) await page.screenshot({ path: process.env.KEY_OVERLAY_SCREENSHOT });
  console.log('PASS the overlay stays above dialogs');
  await page.keyboard.press('Escape');

  await page.getByRole('button', { name: 'Hide pressed keys' }).click();
  assert.equal(await entries.count(), 0, 'hiding clears it');
  assert.deepEqual(errors, []);
  console.log('PASS key overlay');
} finally {
  await browser.close();
}
