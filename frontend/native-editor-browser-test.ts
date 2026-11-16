/** Run with `just test-browser`, or against a running stack with BTMUX_TEST_URL and BTMUX_AUTH_TOKEN. */
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { chromium, type Page } from 'playwright';
import { waitForAsync } from './browser-test-utils';

const url = process.env.BTMUX_TEST_URL ?? 'http://localhost:5173';
const token = process.env.BTMUX_AUTH_TOKEN;
assert(token, 'BTMUX_AUTH_TOKEN is required');
const headers = { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' };
const api = async (path: string, method = 'GET', body?: unknown) => {
  const response = await fetch(`${url}${path}`, { headers, method, body: body ? JSON.stringify(body) : undefined });
  assert(response.ok, `${path}: ${response.status}`);
  const text = await response.text();
  return text ? JSON.parse(text) : null;
};

const dir = realpathSync(mkdtempSync(join(tmpdir(), 'btmux-editor-')));
const fileA = join(dir, 'a.txt');
const fileB = join(dir, 'b.ts');
writeFileSync(fileA, 'one\ntwo\nthree\n');
writeFileSync(fileB, 'const x = 1;\n');

const session: { id: string; name: string } = await api('/api/sessions', 'POST', {
  name: `native-editor-${Date.now()}`,
});
const browser = await chromium.launch({ headless: true, channel: process.env.BTMUX_TEST_BROWSER });

async function openPage(): Promise<Page> {
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 } });
  const page = await context.newPage();
  page.on('pageerror', (error) => console.error('page error:', error.message));
  await page.goto(url);
  await page.getByLabel('Access token').fill(token!);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await page.waitForFunction(() => !document.querySelector('#access-token'));
  await page.goto(`${url}/s/${encodeURIComponent(session.name)}`);
  await waitForAsync(page, async () => {
    const { useStore } = await import('/src/state/store.tsx' as string);
    return useStore.getState().controlConnectionState === 'connected';
  });
  return page;
}

// The editor's lines, as shown (small files aren't virtualized).
const editorText = (page: Page, paneId: string) =>
  page.evaluate((id) => {
    const content = document.querySelector(`[data-native-editor][data-pane-id="${id}"]`);
    if (!content) return null;
    return Array.from(content.querySelectorAll(':scope > .cm-line'), (line) => line.textContent).join('\n');
  }, paneId);

const waitForText = async (page: Page, paneId: string, text: string) => {
  for (let i = 0; i < 200; i++) {
    if ((await editorText(page, paneId)) === text) return;
    await page.waitForTimeout(50);
  }
  assert.equal(await editorText(page, paneId), text);
};

try {
  const page = await openPage();
  const paneId: string = await page.evaluate(async (id) => {
    const { useStore } = await import('/src/state/store.tsx' as string);
    return useStore.getState().getActivePaneId(id);
  }, session.id);

  // `btmux open-editor` opens the native editor by default, on the given line.
  assert.deepEqual(await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: fileA, line: 2, editor: null }), {
    opened: true,
  });
  const input = page.locator(`[data-native-editor][data-pane-id="${paneId}"]`);
  await input.waitFor();
  await waitForText(page, paneId, 'one\ntwo\nthree\n');
  await waitForAsync(page, () => document.activeElement?.hasAttribute('data-native-editor') ?? false);
  const cursorLine = () =>
    page.evaluate(() => document.querySelector('.cm-activeLine')?.textContent ?? null);
  assert.equal(await cursorLine(), 'two');
  console.log('PASS open-editor shows the native editor on the given line, with focus');

  // Typing reaches a second tab, through the server.
  await page.keyboard.press('End');
  await page.keyboard.type('!');
  const other = await openPage();
  await other.locator(`[data-native-editor][data-pane-id="${paneId}"]`).waitFor();
  await waitForText(other, paneId, 'one\ntwo!\nthree\n');

  // Both type at once and converge.
  await other.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
  await other.keyboard.press('Control+End');
  await Promise.all([page.keyboard.type('abc'), other.keyboard.type('xyz')]);
  let converged = '';
  for (let i = 0; i < 100; i++) {
    const [a, b] = [await editorText(page, paneId), await editorText(other, paneId)];
    if (a === b && a?.includes('abc') && a.includes('xyz')) {
      converged = a;
      break;
    }
    await page.waitForTimeout(50);
  }
  assert(converged, 'both tabs show both edits');
  console.log('PASS two tabs edit one buffer at once and converge');

  // Unsaved changes show, and Mod-s writes them.
  await page.locator('[aria-label="Unsaved"]').first().waitFor();
  await input.click();
  await page.keyboard.press('ControlOrMeta+s');
  await page.locator('[aria-label="Unsaved"]').first().waitFor({ state: 'detached' });
  assert.equal(readFileSync(fileA, 'utf8'), converged);
  console.log('PASS saving writes the shared text');

  // A clean buffer follows the file on disk.
  writeFileSync(fileA, 'reloaded\n');
  await waitForText(page, paneId, 'reloaded\n');
  await waitForText(other, paneId, 'reloaded\n');
  // A dirty one warns instead, and Reload takes the disk's text.
  await input.click();
  await page.keyboard.type('mine ');
  writeFileSync(fileA, 'theirs\n');
  await page.getByText('Changed on disk', { exact: true }).waitFor();
  await page.getByRole('button', { name: 'Reload', exact: true }).click();
  await waitForText(page, paneId, 'theirs\n');
  console.log('PASS outside changes reload a clean buffer and warn about a dirty one');

  // prefix + e hides it for every tab, and shows it again.
  await input.click();
  await page.keyboard.press('Control+a');
  await page.keyboard.press('e');
  await input.waitFor({ state: 'hidden' });
  await other.locator(`[data-native-editor][data-pane-id="${paneId}"]`).waitFor({ state: 'hidden' });
  await waitForAsync(page, () => document.activeElement?.getAttribute('aria-label') === 'Terminal');
  await page.keyboard.press('Control+a');
  await page.keyboard.press('e');
  await input.waitFor({ state: 'visible' });
  // A double Esc hides it too.
  await input.click();
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');
  await input.waitFor({ state: 'hidden' });
  console.log('PASS prefix + e and a double Esc hide and show the editor in every tab');

  // Another pane gets its own editor, with both shown.
  await page.keyboard.press('Control+a');
  await page.keyboard.press('%');
  const secondPane: string = await waitForAsync(page, async (first) => {
    const { useStore } = await import('/src/state/store.tsx' as string);
    const s = useStore.getState();
    const session = s.allSessions.find((x: any) => x.name.startsWith('native-editor-'));
    const pane = session?.windows[session.active_window].panes.find((p: any) => p.id !== first);
    return pane?.id ?? false;
  }, paneId);
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: fileA, line: null, editor: 'native' });
  await api(`/api/panes/${secondPane}/open-editor`, 'POST', { path: fileB, line: null, editor: 'native' });
  await waitForText(page, secondPane, 'const x = 1;\n');
  await waitForText(page, paneId, 'theirs\n');
  assert(await input.isVisible());
  console.log('PASS editors over two panes at once');

  // The view is server state: a reload shows the same files.
  await page.reload();
  await waitForText(page, secondPane, 'const x = 1;\n');
  await waitForText(page, paneId, 'theirs\n');
  console.log('PASS the editor view survives a reload');

  // Language servers, with a fake one (test-bin/fake-lsp).
  const lspDir = process.env.BTMUX_TEST_LSP;
  const configHome = process.env.BTMUX_TEST_CONFIG_HOME;
  if (lspDir && configHome) {
    mkdirSync(lspDir, { recursive: true });
    const main = join(dir, 'main.fake');
    writeFileSync(main, 'let bad = 1;\nuse thing;\n');
    writeFileSync(join(dir, 'other.fake'), 'thing defined here\n');
    writeFileSync(join(dir, 'third.fake'), 'more thing\n');
    mkdirSync(join(configHome, 'btmux'), { recursive: true });
    writeFileSync(
      join(configHome, 'btmux', 'config.toml'),
      [
        '[native-editor]',
        'format-on-save = true',
        '[language-server.fake]',
        'command = "fake-lsp"',
        '[[language]]',
        'name = "fake"',
        'file-types = ["fake"]',
        'language-servers = ["fake"]',
        '',
      ].join('\n'),
    );
    await waitForAsync(page, async () => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      return useStore.getState().config?.format_on_save === true;
    });
    await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: main, line: 2, editor: 'native' });
    await waitForText(page, paneId, 'let bad = 1;\nuse thing;\n');
    await page.locator(`[data-editor-pane="${paneId}"] .cm-lintRange-error`).first().waitFor();
    await page.getByLabel('Language server fake').waitFor();
    await page.getByText('1 errors, 0 warnings').waitFor();
    console.log('PASS diagnostics from the language server');

    // The rect of `word`'s first occurrence on `line` (0-based), in the pane's editor.
    const wordRect = (line: number, word: string) =>
      page.evaluate(
        ({ id, line, word }) => {
          const content = document.querySelector(`[data-native-editor][data-pane-id="${id}"]`)!;
          const el = content.querySelectorAll(':scope > .cm-line')[line];
          const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
          for (let node = walker.nextNode(); node; node = walker.nextNode()) {
            const at = node.textContent!.indexOf(word);
            if (at < 0) continue;
            const range = document.createRange();
            range.setStart(node, at);
            range.setEnd(node, at + word.length);
            const r = range.getBoundingClientRect();
            return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
          }
          return null;
        },
        { id: paneId, line, word },
      );

    // Typing is synced: a new "bad" gets its own error.
    const thing = (await wordRect(1, 'thing'))!;
    await page.mouse.click(thing.x, thing.y);
    await page.keyboard.press('End');
    await page.keyboard.type(' bad');
    await page.getByText('2 errors, 0 warnings').waitFor();
    await page.keyboard.press('Backspace');
    await page.keyboard.press('Backspace');
    await page.keyboard.press('Backspace');
    await page.keyboard.press('Backspace');
    await page.getByText('1 errors, 0 warnings').waitFor();
    console.log('PASS edits reach the language server incrementally');

    await page.keyboard.press('End');
    await page.keyboard.press('Enter');
    await page.keyboard.type('fa');
    await page.locator('.cm-tooltip-autocomplete').getByText('fakeCompletion').click();
    await waitForText(page, paneId, 'let bad = 1;\nuse thing;\nfakeCompletion\n');
    console.log('PASS completion');

    const hover = (await wordRect(1, 'thing'))!;
    await page.mouse.move(hover.x, hover.y);
    await page.locator('.cm-tooltip').getByText('thing', { exact: true }).waitFor();
    console.log('PASS hover');

    // F12 jumps to the definition in another file, opened in this pane.
    await page.mouse.click(hover.x, hover.y);
    await page.keyboard.press('F12');
    await waitForAsync(
      page,
      async ({ id, path }) => {
        const { findPaneState, useStore } = await import('/src/state/store.tsx' as string);
        return findPaneState(useStore.getState(), id)?.editor.active === path;
      },
      { id: paneId, path: join(dir, 'other.fake') },
    );
    await waitForText(page, paneId, 'thing defined here\n');
    console.log('PASS go to definition opens the other file');

    // Rename edits every file, open or not; unopened ones wait unsaved.
    await page.getByRole('tab', { name: 'main.fake' }).click();
    await waitForText(page, paneId, 'let bad = 1;\nuse thing;\nfakeCompletion\n');
    const renamed = (await wordRect(1, 'thing'))!;
    await page.mouse.click(renamed.x, renamed.y);
    await page.keyboard.press('F2');
    await page.getByLabel('New name').fill('item');
    await page.keyboard.press('Enter');
    await waitForText(page, paneId, 'let bad = 1;\nuse item;\nfakeCompletion\n');
    const pane = page.locator(`[data-editor-pane="${paneId}"]`);
    await pane.getByRole('button', { name: 'Other unsaved files' }).click();
    await page.getByRole('menuitem', { name: /third\.fake/ }).click();
    await waitForText(page, paneId, 'more item\n');
    await page.getByRole('tab', { name: 'other.fake' }).click();
    await waitForText(page, paneId, 'item defined here\n');
    console.log('PASS rename changes open and unopened files');

    // Format on save.
    await page.getByRole('tab', { name: 'main.fake' }).click();
    await waitForText(page, paneId, 'let bad = 1;\nuse item;\nfakeCompletion\n');
    await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
    await page.keyboard.press('ControlOrMeta+End');
    await page.keyboard.type('end   ');
    await page.keyboard.press('ControlOrMeta+s');
    await waitForText(page, paneId, 'let bad = 1;\nuse item;\nfakeCompletion\nend');
    for (let i = 0; i < 100 && readFileSync(main, 'utf8') !== 'let bad = 1;\nuse item;\nfakeCompletion\nend'; i++)
      await page.waitForTimeout(50);
    assert.equal(readFileSync(main, 'utf8'), 'let bad = 1;\nuse item;\nfakeCompletion\nend');
    assert.match(readFileSync(join(lspDir, 'log'), 'utf8'), /textDocument\/didSave/);
    console.log('PASS format on save');

    // A server that isn't installed shows in the status line, and its features stay off.
    writeFileSync(
      join(configHome, 'btmux', 'config.toml'),
      [
        '[language-server.missing]',
        'command = "btmux-missing-lsp"',
        '[[language]]',
        'name = "nolsp"',
        'file-types = ["nolsp"]',
        'language-servers = ["missing"]',
        '',
      ].join('\n'),
    );
    await waitForAsync(page, async () => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      return useStore.getState().config?.format_on_save === false;
    });
    const nolsp = join(dir, 'x.nolsp');
    writeFileSync(nolsp, 'plain\n');
    await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: nolsp, line: null, editor: 'native' });
    await waitForText(page, paneId, 'plain\n');
    await page.getByText('missing: btmux-missing-lsp not found').waitFor();
    const plain = (await wordRect(0, 'plain'))!;
    await page.mouse.move(plain.x, plain.y);
    await page.waitForTimeout(1000);
    assert.equal(await page.locator('.cm-lsp-message-error').count(), 0);
    console.log('PASS a missing language server shows in the status line without errors');
  }
} finally {
  await browser.close();
  await api(`/api/sessions/${session.id}`, 'DELETE').catch(() => {});
  rmSync(dir, { recursive: true, force: true });
}
