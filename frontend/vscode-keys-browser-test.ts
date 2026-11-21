/** VS Code's keys without the prefix (`lib/workbenchKeys.ts`), on this platform. */
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { waitForAsync } from './browser-test-utils';

const url = process.env.BTMUX_TEST_URL ?? 'http://localhost:5173';
const token = process.env.BTMUX_AUTH_TOKEN;
assert(token, 'BTMUX_AUTH_TOKEN is required');

const browser = await chromium.launch({ headless: true, channel: process.env.BTMUX_TEST_BROWSER });

/** Each platform's keys, whatever the host's: `IS_MAC` reads `navigator.platform`. */
async function run(mac: boolean) {
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 } });
  await context.addInitScript((platform) => {
    Object.defineProperty(Navigator.prototype, 'platform', { get: () => platform });
  }, mac ? 'MacIntel' : 'Linux x86_64');
  const page = await context.newPage();
  const platform = mac ? 'macOS' : 'Linux';
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto(url);
  await page.getByLabel('Access token').fill(token);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await waitForAsync(page, async () => {
    const { useStore } = await import('/src/state/store.tsx' as string);
    return useStore.getState().terminals.size > 0;
  });

  const ui = () =>
    page.evaluate(async () => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      const state = useStore.getState();
      const session = state.allSessions[0];
      const window = session.windows[session.active_window];
      return {
        fileBrowser: state.fileBrowserOpen as boolean,
        mode: state.fileBrowserInitialMode as string,
        search: state.searchPaneId as string | null,
        windows: session.windows.length as number,
        panes: window.panes.length as number,
        pane: window.panes[window.active_pane].id as string,
      };
    });
  const waitForUi = async (check: (state: Awaited<ReturnType<typeof ui>>) => boolean, what: string) => {
    const deadline = Date.now() + 10_000;
    let last = await ui();
    while (!check(last)) {
      assert(Date.now() < deadline, `${what}: ${JSON.stringify(last)}`);
      await page.waitForTimeout(50);
      last = await ui();
    }
  };
  const closeFileBrowser = () =>
    page.evaluate(async () => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      useStore.getState().setFileBrowserOpen(false);
    });
  const focusTerminal = async () => {
    const { pane } = await ui();
    await page.locator(`[data-pane-id="${pane}"] [data-pane-terminal]`).first().click();
  };

  await focusTerminal();
  const mod = mac ? 'Meta' : 'Control';
  await page.keyboard.press(`${mod}+Comma`);
  const dialog = page.locator('dialog[open]');
  await dialog.getByRole('heading', { name: 'Settings', exact: true }).waitFor();
  await page.keyboard.press('Escape');
  await dialog.waitFor({ state: 'hidden' });
  console.log(`PASS ${platform}: settings key`);

  await focusTerminal();
  await page.keyboard.press(`${mod}+Shift+E`);
  await waitForUi((state) => state.fileBrowser && state.mode === 'files', 'Cmd+Shift+E');
  await closeFileBrowser();
  await focusTerminal();
  await page.keyboard.press('Control+Shift+G');
  await waitForUi((state) => state.fileBrowser && state.mode === 'git', 'Ctrl+Shift+G');
  await closeFileBrowser();
  console.log(`PASS ${platform}: file browser and Git mode keys`);

  const before = await ui();
  await focusTerminal();
  await page.keyboard.press('Control+Shift+Backquote');
  await waitForUi((state) => state.windows === before.windows + 1, 'Ctrl+Shift+`');
  await waitForAsync(
    page,
    async (id) => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      const state = useStore.getState();
      const session = state.allSessions[0];
      const pane = session.windows[session.active_window].panes[0].id;
      return pane !== id && state.terminals.has(pane);
    },
    before.pane,
  );
  await focusTerminal();
  await page.keyboard.press(mac ? 'Meta+Backslash' : 'Control+Shift+Digit5');
  await waitForUi((state) => state.panes === 2, 'split');
  console.log(`PASS ${platform}: new window and split keys`);

  if (mac) {
    await focusTerminal();
    await page.keyboard.press('Meta+F');
    await waitForUi((state) => state.search !== null, 'Cmd+F');
    await page.keyboard.press('Escape');
    await waitForUi((state) => state.search === null, 'Cmd+F closed');
    console.log(`PASS ${platform}: Cmd+F searches the pane`);
  }

  // Clearing leaves only the prompt, in this tab, the server's screen, and a replay.
  const { pane } = await ui();
  await waitForAsync(
    page,
    async (id) => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      return useStore.getState().terminals.has(id);
    },
    pane,
  );
  await page.request.post(`${url}/api/panes/${pane}/input`, {
    data: { text: "seq 1 200; printf 'CLEAR_%s\\n' MARK", keys: ['Enter'] },
  });
  const lines = () =>
    page.evaluate(async (id) => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      return (useStore.getState().terminals.get(id)?.readLines() ?? []) as string[];
    }, pane);
  const waitForLines = async (check: (lines: string[]) => boolean, what: string) => {
    const deadline = Date.now() + 10_000;
    let last = await lines();
    while (!check(last)) {
      assert(Date.now() < deadline, `${what}:\n${last.join('\n')}`);
      await page.waitForTimeout(50);
      last = await lines();
    }
    return last;
  };
  await waitForLines((lines) => lines.includes('CLEAR_MARK') && lines.at(-1) !== '', 'output');
  await focusTerminal();
  if (mac) {
    await page.keyboard.press('Meta+K');
  } else {
    await page.keyboard.press('Control+Shift+P');
    await page.keyboard.type('clear pane');
    await page.keyboard.press('Enter');
    await dialog.waitFor({ state: 'hidden' });
  }
  const cleared = (lines: string[]) => !lines.includes('CLEAR_MARK') && lines.filter(Boolean).length === 1;
  const prompt = (await waitForLines(cleared, 'cleared')).find(Boolean)!;
  assert.equal((await lines())[0], prompt, 'the prompt is the first line');
  const captured = await (await page.request.get(`${url}/api/panes/${pane}/capture`)).text();
  assert(!captured.includes('CLEAR_MARK') && !captured.includes('200'), captured);
  // The shell still edits its line where it is.
  await page.keyboard.type('echo after');
  await waitForLines((lines) => lines[0].endsWith('echo after'), 'typed after the clear');
  await page.reload();
  await waitForLines((lines) => lines[0]?.endsWith('echo after') && !lines.includes('CLEAR_MARK'), 'replay');
  console.log(`PASS ${platform}: ${mac ? 'Cmd+K' : 'clear-pane'} clears the pane`);

  assert.deepEqual(errors, []);
  await context.close();
}

try {
  await run(true);
  await run(false);
} finally {
  await browser.close();
}
