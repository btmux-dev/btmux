/** `btmux popup` and `[[popup]]` keys against the isolated browser-test stack. */
import assert from 'node:assert/strict';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
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
  const popup = page.locator(POPUP);
  const popupShows = (text: string) =>
    page.waitForFunction(
      ([sel, text]) => document.querySelector(sel)?.textContent?.includes(text),
      [POPUP, text] as const,
      { timeout: 15_000 },
    );
  const closed = () => page.waitForSelector(POPUP, { state: 'detached' });
  // Its size once the open animation ends.
  const settledDialog = async () => {
    let last = '';
    for (;;) {
      const box = JSON.stringify(await page.locator('dialog[open]').boundingBox());
      if (box === last) return JSON.parse(box);
      last = box;
      await page.waitForTimeout(150);
    }
  };
  const terminalFocused = () =>
    page.evaluate(() => {
      const focused = document.activeElement;
      return focused?.closest('[data-pane-terminal]') !== null && focused?.closest('[data-popup]') === null;
    });

  // The file browser's size, for comparison.
  await page.locator('[data-pane-terminal]').first().click();
  await page.keyboard.press('Control+a');
  await page.keyboard.press('f');
  await page.waitForSelector('dialog[open]');
  const fileBrowser = await settledDialog();
  await page.keyboard.press('Escape');
  await page.waitForSelector('dialog[open]', { state: 'detached' });

  // A program and its arguments run directly, sized like the file browser; Esc
  // reaches the program, and its exit status is the command's.
  await run(
    `clear; "$BTMUX_BIN" popup -T Test sh -c 'printf "ready\\n"; read -r line; echo "got $line"; exit 7'; echo "rc=$?"`,
  );
  await popupShows('ready');
  assert((await popup.innerText()).includes('Test'));
  assert.deepEqual(await settledDialog(), fileBrowser);
  await page.waitForFunction(() => document.activeElement?.closest('[data-popup] [data-pane-terminal]') != null);
  await page.keyboard.press('Escape');
  await page.waitForTimeout(200);
  assert.equal(await popup.count(), 1, 'Esc goes to the program');
  await page.keyboard.type('hello');
  await page.keyboard.press('Enter');
  await closed();
  await waitForOutput('rc=7');
  assert(await terminalFocused(), 'the pane has focus again');

  // One argument is a command line; the caller's directory and environment apply.
  await run(`clear; cd /tmp && "$BTMUX_BIN" popup -e FOO=bar 'echo "$FOO:$(pwd -P)"; read x; exit 3'; echo "rc=$?"; cd -`);
  await popupShows('bar:');
  assert(/bar:\/(private\/)?tmp/.test(await popup.innerText()), await popup.innerText());
  await page.keyboard.press('Enter');
  await closed();
  await waitForOutput('rc=3');

  // A popup outlives a reload, and the close button hangs it up.
  await run(`clear; "$BTMUX_BIN" popup sh -c 'echo waiting; sleep 60'; echo "rc=$?"`);
  await popupShows('waiting');
  await page.reload();
  await popupShows('waiting');
  await popup.getByRole('button', { name: 'Close popup' }).click();
  await closed();
  await waitForOutput('rc=129');

  // A program that can't start fails the command.
  await run(`clear; "$BTMUX_BIN" popup /nonexistent/btmux-popup x; echo "rc=$?"`);
  await waitForOutput('rc=127');
  assert.equal(await popup.count(), 0);

  // Without a tab showing the pane, or outside btmux, it runs in place.
  await run(
    `clear; p=$("$BTMUX_BIN" new-window -d); "$BTMUX_BIN" popup -t "$p" sh -c 'echo inline-run'; echo "rc=$?"`,
  );
  await waitForOutput('rc=0');
  assert((await screen()).includes('inline-run'));
  assert.equal(await popup.count(), 0);
  await run(`clear; BTMUX_PANE_ID= "$BTMUX_BIN" popup 'echo outside; exit 4'; echo "rc=$?"`);
  await waitForOutput('rc=4');
  assert((await screen()).includes('outside'));

  // A [[popup]] key runs its command in the pane's directory, through the shell;
  // `prefix prefix` reaches the program.
  await run(`clear; cd /tmp`);
  writeFileSync(
    join(configHome, 'btmux', 'config.toml'),
    '[[popup]]\nkey = "u"\ncommand = "pwd -P; od -An -c; read x"\ntitle = "Bound"\n',
  );
  await waitForAsync(page, async () => {
    const { useStore } = await import('/src/state/store.tsx' as string);
    return useStore.getState().config?.popups.length === 1;
  });
  await page.locator('[data-pane-terminal]').first().click();
  await page.keyboard.press('Control+a');
  await page.keyboard.press('u');
  await popupShows('tmp');
  assert((await popup.innerText()).includes('Bound'));
  await page.waitForFunction(() => document.activeElement?.closest('[data-popup] [data-pane-terminal]') != null);
  await page.keyboard.press('Control+a');
  await page.keyboard.press('Control+a');
  await page.keyboard.press('Enter');
  await page.keyboard.press('Control+d');
  await popupShows('001');
  await page.keyboard.press('Enter');
  await closed();
  assert(await terminalFocused(), 'the pane has focus again');
  assert(!(await screen()).includes('\x01'), 'the prefix reached only the popup');

  const popupFocused = () =>
    page.waitForFunction(() => document.activeElement?.closest('[data-popup] [data-pane-terminal]') != null);
  const popupId = () =>
    page.evaluate(async () => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      return useStore.getState().popupShown?.id as string;
    });
  const searchPane = () =>
    page.evaluate(async () => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      return useStore.getState().searchPaneId as string | null;
    });

  // Clipboard and image pastes, and pane search, go to the popup.
  await run(`clear; "$BTMUX_BIN" popup sh -c 'echo paste-ready; read -r a; read -r b; echo "got=$a,$b"; read -r x'; echo "rc=$?"`);
  await popupShows('paste-ready');
  await popupFocused();
  await page.evaluate(() =>
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { readText: async () => 'CLIP_MARKER' },
    }),
  );
  await page.keyboard.press('Control+a');
  await page.keyboard.press(']');
  await popupShows('CLIP_MARKER');
  await page.keyboard.press('Enter');
  await page.evaluate(() => {
    const data = new DataTransfer();
    data.items.add(new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], 'shot.png', { type: 'image/png' }));
    document
      .querySelector('[data-popup] [data-pane-terminal] textarea')!
      .dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
  });
  await popupShows('paste-');
  await page.keyboard.press('Enter');
  await popupShows('got=CLIP_MARKER,');
  // Long paths wrap.
  const pasted = (await popup.innerText()).replace(/\s+/g, '');
  assert(/got=CLIP_MARKER,\/\S*paste-[0-9a-f]+\.png/.test(pasted), pasted);
  assert(!(await screen()).includes('CLIP_MARKER'), 'the pane got no paste');
  await page.keyboard.press('Control+a');
  await page.keyboard.press('/');
  const search = popup.getByLabel('Search scrollback');
  await search.waitFor();
  assert(await search.evaluate((input) => input === document.activeElement), 'the search has focus');
  assert.equal(await searchPane(), await popupId());
  await page.keyboard.type('CLIP_MARKER');
  await popup.getByText(/\d+\/\d+/).waitFor();
  await page.keyboard.press('Escape');
  await search.waitFor({ state: 'detached' });
  await popupFocused();
  // Closing the popup closes its search, which would keep prefix keys.
  await page.keyboard.press('Control+a');
  await page.keyboard.press('/');
  await search.waitFor();
  await popup.getByRole('button', { name: 'Close popup' }).click();
  await closed();
  await waitForOutput('rc=129');
  assert.equal(await searchPane(), null);

  // Widgets of programs in a popup show in it.
  await run(`clear; "$BTMUX_BIN" popup sh -c 'printf "one\\ntwo\\n" | "$BTMUX_BIN" pick; read -r x'; echo "rc=$?"`);
  const picker = popup.locator('[data-pane-widget]:not(.opacity-0)');
  await picker.waitFor();
  await page.waitForFunction(() => document.activeElement?.closest('[data-popup] [data-pane-widget]') != null);
  await page.keyboard.type('two');
  await page.keyboard.press('Enter');
  await picker.waitFor({ state: 'detached' });
  await popupShows('two');
  await popupFocused();
  await page.keyboard.press('Enter');
  await closed();
  await waitForOutput('rc=0');

  // The pane's own widgets, under the popup, aren't shown, so their callers fall back.
  await run(
    `clear; (sleep 1; printf "a\\n" | "$BTMUX_BIN" pick; echo "bg=$?") & "$BTMUX_BIN" popup sh -c 'echo cover; read -r x'; wait; echo "rc=$?"`,
  );
  await popupShows('cover');
  await waitForOutput('bg=2');
  await popupFocused();
  await page.keyboard.press('Enter');
  await closed();
  await waitForOutput('rc=0');

  assert.deepEqual(errors, []);
  console.log('popup browser test passed');
} finally {
  await browser.close();
}
