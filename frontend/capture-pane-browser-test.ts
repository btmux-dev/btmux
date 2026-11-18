/** `prefix + [` (capture-pane) against the isolated browser-test stack. */
import assert from 'node:assert/strict';
import { existsSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { chromium } from 'playwright';
import { waitForAsync } from './browser-test-utils';

const url = process.env.BTMUX_TEST_URL ?? 'http://localhost:5173';
const token = process.env.BTMUX_AUTH_TOKEN;
assert(token, 'BTMUX_AUTH_TOKEN is required');
const configHome = process.env.BTMUX_TEST_CONFIG_HOME;
assert(configHome, 'BTMUX_TEST_CONFIG_HOME is required');

const POPUP = '[data-popup]';

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
  const screen = async () => (await page.request.get(api(`/panes/${paneId}/capture`))).text();
  const waitForOutput = async (text: string) => {
    const deadline = Date.now() + 15_000;
    let last = '';
    while (Date.now() < deadline) {
      last = await screen();
      if (last.includes(text)) return;
      await page.waitForTimeout(50);
    }
    assert.fail(`never saw ${text} in:\n${last}`);
  };
  const editorFiles = () =>
    page.evaluate(async (id) => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      for (const session of useStore.getState().allSessions)
        for (const window of session.windows)
          for (const pane of window.panes) if (pane.id === id) return pane.editor.files as string[];
      return [];
    }, paneId);
  const capture = async () => {
    await page.locator('[data-pane-terminal]').first().click();
    await page.keyboard.press('Control+a');
    await page.keyboard.press('[');
  };

  // A program other than the shell has the terminal, as in an agent pane;
  // `cat` would echo anything typed into it.
  await run('clear; echo capture-marker; cat');
  await waitForOutput('capture-marker');

  // The native editor opens the capture over the pane.
  await capture();
  const editor = page.locator(`[data-native-editor][data-pane-id="${paneId}"]`);
  await editor.waitFor();
  await page.waitForFunction(
    (id) => document.querySelector(`[data-native-editor][data-pane-id="${id}"]`)?.textContent?.includes('capture-marker'),
    paneId,
  );
  const [path] = await editorFiles();
  assert.match(path, /btmux-capture-[0-9a-f]+\/capture-1\.txt$/);
  assert(existsSync(path), 'the capture is saved');
  assert(!(await screen()).includes('capture-1'), 'nothing was typed into the pane');
  console.log('PASS capture-pane opens the native editor over a busy pane');

  // Closing its tab deletes the file.
  await editor.click();
  await page.keyboard.press('Alt+w');
  await page.getByRole('tab', { name: 'capture-1.txt' }).waitFor({ state: 'detached' });
  for (let i = 0; i < 100 && existsSync(path); i++) await page.waitForTimeout(50);
  assert(!existsSync(path), 'closing the capture deletes it');
  assert.deepEqual(await editorFiles(), []);
  console.log('PASS closing a capture deletes it');

  // With `editor = "pane"`, `$EDITOR` (the stack's `test-editor`) runs in a
  // popup, and the capture is removed when it ends, however it ends.
  writeFileSync(join(configHome, 'btmux', 'config.toml'), 'editor = "pane"\n');
  await waitForAsync(page, async () => {
    const { useStore } = await import('/src/state/store.tsx' as string);
    return useStore.getState().config?.editor === 'pane';
  });
  const dir = dirname(path);
  const popupShows = (text: string) =>
    page.waitForFunction(
      ([sel, text]) => document.querySelector<HTMLElement>(sel)?.innerText.includes(text),
      [POPUP, text] as const,
      { timeout: 15_000 },
    );
  const captureRemoved = async () => {
    for (let i = 0; i < 100 && readdirSync(dir).length > 0; i++) await page.waitForTimeout(50);
    assert.deepEqual(readdirSync(dir), [], 'the popup removed its capture');
  };
  await capture();
  await popupShows('[test-editor]');
  const text = await page.locator(POPUP).innerText();
  assert(text.includes('capture-marker') && text.includes('Pane capture'));
  assert.equal(readdirSync(dir).length, 1);
  await page.waitForFunction(() => document.activeElement?.closest('[data-popup] [data-pane-terminal]') != null);
  await page.keyboard.press('Enter');
  await page.waitForSelector(POPUP, { state: 'detached', timeout: 15_000 });
  await captureRemoved();
  console.log('PASS capture-pane with editor = "pane" runs $EDITOR in a popup');

  await capture();
  await popupShows('[test-editor]');
  await page.locator(POPUP).getByRole('button', { name: 'Close popup' }).click();
  await page.waitForSelector(POPUP, { state: 'detached', timeout: 15_000 });
  await captureRemoved();
  assert(!(await screen()).includes('btmux-capture'), 'nothing was typed into the pane');
  console.log('PASS closing the popup removes its capture');

  assert.deepEqual(errors, []);
} finally {
  await browser.close();
}
