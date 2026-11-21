/** VS Code's keys without the prefix (`lib/workbenchKeys.ts`), for macOS and Linux. */
import assert from 'node:assert/strict';
import { mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { chromium, type Page } from 'playwright';
import { waitForAsync } from './browser-test-utils';

const url = process.env.BTMUX_TEST_URL ?? 'http://localhost:5173';
const token = process.env.BTMUX_AUTH_TOKEN;
assert(token, 'BTMUX_AUTH_TOKEN is required');
const configHome = process.env.BTMUX_TEST_CONFIG_HOME;
assert(configHome, 'BTMUX_TEST_CONFIG_HOME is required');
const configFile = join(configHome, 'btmux', 'config.toml');
// Quick open searches the pane's directory.
const dir = realpathSync(mkdtempSync(join(tmpdir(), 'btmux-vscode-keys-')));
writeFileSync(join(dir, 'vk-target.txt'), 'vk');

const browser = await chromium.launch({ headless: true, channel: process.env.BTMUX_TEST_BROWSER });

/** Writes config.toml and waits for the tab to get it. */
async function setConfig(page: Page, toml: string, applied: string) {
  writeFileSync(configFile, toml);
  await waitForAsync(
    page,
    async (applied) => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      const config = useStore.getState().config;
      return !!config && new Function('config', `return ${applied}`)(config);
    },
    applied,
  );
}

/** Each platform's keys, whatever the host's: `IS_MAC` reads `navigator.platform`. */
async function run(mac: boolean) {
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 } });
  await context.addInitScript((platform) => {
    Object.defineProperty(Navigator.prototype, 'platform', { get: () => platform });
    // `dropSelect` holds back pane selections, as a slow server would.
    const send = WebSocket.prototype.send;
    WebSocket.prototype.send = function (data) {
      if ((window as any).dropSelect && typeof data === 'string' && data.includes('"select_pane"')) return;
      send.call(this, data);
    };
  }, mac ? 'MacIntel' : 'Linux x86_64');
  const page = await context.newPage();
  const platform = mac ? 'macOS' : 'Linux';
  const mod = mac ? 'Meta' : 'Control';
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
        panes: window.panes.map((pane: { id: string }) => pane.id) as string[],
        pane: window.panes[window.active_pane].id as string,
        layout: window.layout,
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
    return last;
  };
  const closeFileBrowser = () =>
    page.evaluate(async () => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      useStore.getState().setFileBrowserOpen(false);
    });
  const terminal = (pane: string) => page.locator(`[data-pane-id="${pane}"] [data-pane-terminal]`).first();
  const focusTerminal = async () => terminal((await ui()).pane).click();
  const dropSelect = (on: boolean) => page.evaluate((on) => ((window as any).dropSelect = on), on);
  const dialog = page.locator('dialog[open]');

  await focusTerminal();
  await page.keyboard.press(`${mod}+Comma`);
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

  const split = mac ? 'Meta+Backslash' : 'Control+Shift+Digit5';
  const before = await ui();
  await focusTerminal();
  await page.keyboard.press('Control+Shift+Backquote');
  const opened = await waitForUi(
    (state) => state.windows === before.windows + 1 && !state.panes.includes(before.pane),
    'Ctrl+Shift+`',
  );
  await terminal(opened.pane).waitFor();
  await focusTerminal();
  await page.keyboard.press(split);
  const two = await waitForUi((state) => state.panes.length === 2, 'split');
  console.log(`PASS ${platform}: new window and split keys`);

  // Keys act on the focused terminal's pane, before the server selects it.
  const other = two.panes.find((pane) => pane !== two.pane)!;
  // Focus follows the server's selection, so press on the pane's input itself.
  const pressIn = (pane: string, init: KeyboardEventInit) =>
    terminal(pane)
      .getByRole('textbox')
      .evaluate((input, init) => input.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, ...init })), init);
  const splitKey = mac
    ? { key: '\\', code: 'Backslash', metaKey: true }
    : { key: '%', code: 'Digit5', ctrlKey: true, shiftKey: true };
  await terminal(other).waitFor();
  await dropSelect(true);
  await terminal(other).click();
  assert.equal((await ui()).pane, two.pane, 'the selection was held back');
  await pressIn(other, splitKey);
  const three = await waitForUi((state) => state.panes.length === 3, 'split of the focused pane');
  const added = three.panes.find((pane) => !two.panes.includes(pane));
  const splitOf = (node: any): string[] | null =>
    node.type === 'leaf'
      ? null
      : (node.type === 'v_split' ? [node.left, node.right] : [node.top, node.bottom]).every((n: any) => n.type === 'leaf')
        ? (node.type === 'v_split' ? [node.left, node.right] : [node.top, node.bottom]).map((n: any) => n.pane_id)
        : (splitOf(node.left ?? node.top) ?? splitOf(node.right ?? node.bottom));
  assert.deepEqual(splitOf(three.layout), [other, added], JSON.stringify(three.layout));
  if (mac) {
    await pressIn(other, { key: 'f', code: 'KeyF', metaKey: true });
    await waitForUi((state) => state.search === other, 'Cmd+F in the focused pane');
    await page.evaluate(async () => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      useStore.getState().setSearchPaneId(null);
    });
  }
  await dropSelect(false);
  console.log(`PASS ${platform}: keys act on the focused pane`);

  // The prefix outranks them: a `C-g` prefix pressed twice sends itself, not Git mode.
  await setConfig(page, 'prefix = "C-g"\n', 'config.prefix === "C-g"');
  await terminal(other).click();
  await page.keyboard.press('Control+g');
  await page.keyboard.press('Control+Shift+G');
  await page.waitForTimeout(300);
  assert.equal((await ui()).fileBrowser, false, 'the second C-g went to the prefix');
  // Turned off, none work.
  await setConfig(page, 'vscode-keys = false\n', 'config.vscode_keys === false');
  await terminal(other).click();
  await page.keyboard.press(`${mod}+Shift+E`);
  await page.keyboard.press(`${mod}+Comma`);
  await page.waitForTimeout(300);
  assert.equal((await ui()).fileBrowser, false, 'vscode-keys = false');
  assert.equal(await dialog.count(), 0, 'vscode-keys = false');
  await setConfig(page, '', 'config.vscode_keys === true && config.prefix === "C-a"');
  console.log(`PASS ${platform}: the prefix and vscode-keys = false`);

  if (mac) {
    // Cmd+P opens quick open over the focused terminal's pane.
    await page.request.post(`${url}/api/panes/${two.pane}/input`, {
      data: { text: `cd '${dir}'`, keys: ['Enter'] },
    });
    await waitForAsync(
      page,
      async ({ pane, dir }) => {
        const { findPaneState, useStore } = await import('/src/state/store.tsx' as string);
        return findPaneState(useStore.getState(), pane)?.cwd === dir;
      },
      { pane: two.pane, dir },
    );
    await dropSelect(true);
    await terminal(added!).click();
    await pressIn(two.pane, { key: 'p', code: 'KeyP', metaKey: true });
    const picker = (pane: string) =>
      page.locator(`[data-editor-pane="${pane}"]`).getByRole('combobox', { name: 'Go to file' });
    await picker(two.pane).waitFor();
    await page.keyboard.type('vk-target');
    await page.getByRole('option', { name: /vk-target\.txt/ }).waitFor();
    await page.keyboard.press('Enter');
    // A focused native editor runs its own, for its pane.
    const line = page.locator(`[data-native-editor][data-pane-id="${two.pane}"] .cm-line`, { hasText: 'vk' });
    await line.click();
    await page.keyboard.press('Meta+P');
    await picker(two.pane).waitFor();
    await page.keyboard.press('Escape');
    await dropSelect(false);
    // With another editor, Cmd+P isn't taken.
    await setConfig(page, 'editor = "pane"\n', 'config.editor === "pane"');
    await terminal(added!).click();
    await page.keyboard.press('Meta+P');
    await page.waitForTimeout(300);
    assert.equal(await picker(added!).count(), 0, 'editor = "pane"');
    await setConfig(page, '', 'config.editor === "native"');
    console.log(`PASS ${platform}: Cmd+P quick open`);
  }

  assert.deepEqual(errors, []);
  await context.close();
}

try {
  await run(true);
  await run(false);
} finally {
  await browser.close();
  rmSync(dir, { recursive: true, force: true });
}
