/** Run with `just test-browser`, or against a running stack with BTMUX_TEST_URL and BTMUX_AUTH_TOKEN. */
import assert from 'node:assert/strict';
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { chromium, type BrowserContext, type Page } from 'playwright';
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
  cwd: dir,
});
const browser = await chromium.launch({ headless: true, channel: process.env.BTMUX_TEST_BROWSER });

/** Every URL the pages asked for. */
const requested: string[] = [];

async function openPage(setup?: (context: BrowserContext) => Promise<void>): Promise<Page> {
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 } });
  await setup?.(context);
  const page = await context.newPage();
  page.on('pageerror', (error) => console.error('page error:', error.message));
  page.on('request', (request) => requested.push(request.url()));
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
    // Without inlay hints.
    const text = (line: Element) => {
      const copy = line.cloneNode(true) as Element;
      for (const hint of copy.querySelectorAll('.cm-lsp-inlay-hint')) hint.remove();
      return copy.textContent;
    };
    return Array.from(content.querySelectorAll(':scope > .cm-line'), text).join('\n');
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
  const cursorLine = () => page.evaluate(() => document.querySelector('.cm-activeLine')?.textContent ?? null);
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
  const secondPane: string = await waitForAsync(
    page,
    async (first) => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      const s = useStore.getState();
      const session = s.allSessions.find((x: any) => x.name.startsWith('native-editor-'));
      const pane = session?.windows[session.active_window].panes.find((p: any) => p.id !== first);
      return pane?.id ?? false;
    },
    paneId,
  );
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

  // Edits typed while disconnected survive the server dropping the clean
  // buffer, and merge with the file changing meanwhile.
  await other.context().close();
  await input.click();
  await page.keyboard.press('ControlOrMeta+End');
  await page.evaluate(async () => {
    const { editorConnection } = await import('/src/lib/nativeEditor/connection.ts' as string);
    (editorConnection as any).ws.close();
  });
  await page.keyboard.type('UNSENT');
  await page.waitForTimeout(300);
  writeFileSync(fileA, 'outside\ntheirs\n');
  await waitForText(page, paneId, 'outside\ntheirs\nUNSENT');
  await page.getByText('Merged edits made while disconnected', { exact: true }).waitFor();
  await page.locator('[aria-label="Unsaved"]').first().waitFor();
  await input.click();
  await page.keyboard.press('ControlOrMeta+s');
  await page.locator('[aria-label="Unsaved"]').first().waitFor({ state: 'detached' });
  assert.equal(readFileSync(fileA, 'utf8'), 'outside\ntheirs\nUNSENT');
  console.log('PASS edits made while disconnected survive a reopen');

  // Quick open searches the pane's folder and opens the choice in its editor.
  const editorPane = page.locator(`[data-editor-pane="${paneId}"]`);
  const goToFile = editorPane.getByRole('combobox', { name: 'Go to file' });
  const activeFile = (path: string) =>
    waitForAsync(
      page,
      async ({ id, path }) => {
        const { findPaneState, useStore } = await import('/src/state/store.tsx' as string);
        return findPaneState(useStore.getState(), id)?.editor.active === path;
      },
      { id: paneId, path },
    );
  mkdirSync(join(dir, 'deep', 'nested'), { recursive: true });
  const target = join(dir, 'deep', 'nested', 'quickopen-target.md');
  writeFileSync(target, 'found me\nline two\nline three\n');
  // Files viewed in a focused editor are recent.
  await page.locator(`[data-native-editor][data-pane-id="${secondPane}"]`).click();
  await input.click();
  await page.keyboard.press('ControlOrMeta+p');
  await goToFile.waitFor();
  // Without a query: the pane's other files, then recent ones (b.ts, shown in the other pane).
  await editorPane.getByRole('option', { name: /^b\.ts/ }).waitFor();
  // The picker fills its panel, so the list isn't clipped, and fits the editor.
  const pickerBox = await editorPane.locator('.cm-btmux-picker').evaluate((picker) => ({
    picker: picker.getBoundingClientRect().height,
    panels: picker.closest('.cm-panels')!.clientHeight,
    editor: picker.closest('.cm-editor')!.clientHeight,
  }));
  assert.ok(Math.abs(pickerBox.picker - pickerBox.panels) <= 1, JSON.stringify(pickerBox));
  assert.ok(pickerBox.panels <= pickerBox.editor / 2, JSON.stringify(pickerBox));
  await page.keyboard.type('qotgt');
  const targetOption = editorPane.getByRole('option', { name: /quickopen-target\.md/ });
  await targetOption.waitFor();
  assert.equal(await targetOption.locator('.cm-btmux-picker-match').allTextContents().then((m) => m.join('')), 'qotgt');
  await page.keyboard.press('Enter');
  await activeFile(target);
  await waitForText(page, paneId, 'found me\nline two\nline three\n');
  console.log('PASS quick open finds a file by fuzzy query and opens it in the pane');

  // `file:line` opens on that line, and `:line` alone moves in the shown file.
  await page.keyboard.press('ControlOrMeta+p');
  await page.keyboard.type('qotgt:3');
  await editorPane.getByRole('option', { name: /quickopen-target\.md:3/ }).waitFor();
  await page.keyboard.press('Enter');
  await editorPane.locator('.cm-activeLine', { hasText: /^line three$/ }).waitFor();
  await page.keyboard.press('ControlOrMeta+p');
  await page.keyboard.type(':2');
  await page.keyboard.press('Enter');
  await editorPane.locator('.cm-activeLine', { hasText: /^line two$/ }).waitFor();
  console.log('PASS quick open goes to file:line and :line');

  // Ctrl-N and Ctrl-P move through the results, wrapping, as in vim.
  await page.keyboard.press('ControlOrMeta+p');
  await page.keyboard.type('.');
  const options = editorPane.locator('.cm-btmux-picker [role=option]');
  await options.nth(2).waitFor();
  const selectedOption = () =>
    options.evaluateAll((items) => items.findIndex((li) => li.getAttribute('aria-selected') === 'true'));
  const optionCount = await options.count();
  await page.keyboard.press('Control+n');
  assert.equal(await selectedOption(), 1);
  await page.keyboard.press('Control+p');
  await page.keyboard.press('Control+p');
  assert.equal(await selectedOption(), optionCount - 1);
  await page.keyboard.press('Control+n');
  assert.equal(await selectedOption(), 0);
  // Ctrl-J and Ctrl-K too, as in fzf (browsers off macOS keep Ctrl-N).
  await page.keyboard.press('Control+j');
  assert.equal(await selectedOption(), 1);
  await page.keyboard.press('Control+k');
  assert.equal(await selectedOption(), 0);
  await page.keyboard.press('Escape');
  await goToFile.waitFor({ state: 'detached' });
  console.log('PASS Ctrl-N/P and Ctrl-J/K move through quick open results');

  // A file created after the first search shows up for the query typed once
  // the stale index (over 5 s old) is listed again.
  const late = join(dir, 'late-arrival.txt');
  writeFileSync(late, 'late\n');
  await page.waitForTimeout(5500);
  await page.keyboard.press('ControlOrMeta+p');
  await goToFile.waitFor();
  await page.keyboard.type('latearr');
  // Enter waits for the search of the new list, past the stale one.
  await page.keyboard.press('Enter');
  await activeFile(late);
  await waitForText(page, paneId, 'late\n');
  console.log('PASS quick open finds files created after the first search');

  // The command palette shows a hidden editor with quick open.
  await input.click();
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');
  await input.waitFor({ state: 'hidden' });
  await page.keyboard.press('Control+a');
  await page.keyboard.press(':');
  await page.keyboard.type('editor quick open');
  await page.keyboard.press('Enter');
  await goToFile.waitFor();
  await waitForAsync(page, () => document.activeElement?.getAttribute('aria-label') === 'Go to file');
  await page.keyboard.type('b.ts');
  await editorPane.getByRole('option', { name: /^b\.ts/ }).waitFor();
  await page.keyboard.press('Enter');
  await activeFile(fileB);
  await waitForText(page, paneId, 'const x = 1;\n');
  // And with the editor already shown, typing still goes to quick open.
  await page.keyboard.press('Control+a');
  await page.keyboard.press(':');
  await page.keyboard.type('editor quick open');
  await page.keyboard.press('Enter');
  await goToFile.waitFor();
  await waitForAsync(page, () => document.activeElement?.getAttribute('aria-label') === 'Go to file');
  await page.keyboard.type('qotgt');
  assert.equal(await goToFile.inputValue(), 'qotgt');
  await targetOption.waitFor();
  await page.keyboard.press('Enter');
  await activeFile(target);
  assert.equal(readFileSync(fileB, 'utf8'), 'const x = 1;\n');
  console.log('PASS the command palette runs quick open');

  // Esc after Enter, before the results arrive, cancels the pick.
  const slow = await openPage(async (context) => {
    await context.routeWebSocket(/\/ws\/files/, (ws) => {
      const server = ws.connectToServer();
      server.onMessage((message) => setTimeout(() => ws.send(message), 1500));
    });
  });
  const slowInput = slow.locator(`[data-native-editor][data-pane-id="${paneId}"]`);
  await waitForText(slow, paneId, 'found me\nline two\nline three\n');
  await slowInput.click();
  await slow.keyboard.press('ControlOrMeta+p');
  await slow.locator(`[data-editor-pane="${paneId}"]`).getByRole('combobox', { name: 'Go to file' }).waitFor();
  await slow.keyboard.type('b.ts');
  await slow.keyboard.press('Enter');
  await slow.keyboard.press('Escape');
  await slow.waitForTimeout(2500);
  await activeFile(target);
  await slow.context().close();
  console.log('PASS Esc cancels a pick waiting for results');

  // Over a symlinked folder, results still show relative to it (the server
  // searches it with symlinks resolved); and a pane whose cwd is on another
  // host gets a message, not an editor over it.
  const link = join(tmpdir(), `btmux-editor-link-${Date.now()}`);
  symlinkSync(dir, link);
  const remote: { id: string; name: string; pane_id: string } = await api('/api/sessions', 'POST', {
    name: `native-editor-remote-${Date.now()}`,
    cwd: dir,
  });
  const paneState = (id: string) =>
    page.evaluate(async (id) => {
      const { findPaneState, useStore } = await import('/src/state/store.tsx' as string);
      const pane = findPaneState(useStore.getState(), id);
      return { cwd: pane?.cwd, remote: pane?.cwd_remote, shown: pane?.editor.shown };
    }, id);
  try {
    await page.goto(`${url}/s/${encodeURIComponent(remote.name)}`);
    await page.getByLabel('Terminal').first().waitFor();
    await api(`/api/panes/${remote.pane_id}/input`, 'POST', {
      text: `printf '\\033]7;file://${link}\\007'`,
      keys: ['Enter'],
    });
    for (let i = 0; i < 200 && (await paneState(remote.pane_id)).cwd !== link; i++) await page.waitForTimeout(50);
    assert.equal((await paneState(remote.pane_id)).cwd, link);
    await api(`/api/panes/${remote.pane_id}/open-editor`, 'POST', { path: fileA, line: null, editor: 'native' });
    const remoteInput = page.locator(`[data-native-editor][data-pane-id="${remote.pane_id}"]`);
    await remoteInput.click();
    await page.keyboard.press('ControlOrMeta+p');
    const linked = page
      .locator(`[data-editor-pane="${remote.pane_id}"]`)
      .getByRole('option', { name: /quickopen-target/ });
    // Recent files, which have resolved paths, are listed under it…
    await linked.waitFor();
    assert.equal(await linked.textContent(), 'quickopen-target.mddeep/nested');
    // …and so are search results (highlighted, unlike recent files).
    await page.keyboard.type('qotgt');
    await linked.locator('.cm-btmux-picker-match').first().waitFor();
    assert.equal(await linked.textContent(), 'quickopen-target.mddeep/nested');
    await page.keyboard.press('Escape');
    await page.keyboard.press('Escape');
    await page.keyboard.press('Escape');
    await remoteInput.waitFor({ state: 'hidden' });
    console.log('PASS quick open over a symlinked folder shows paths relative to it');

    await api(`/api/panes/${remote.pane_id}/input`, 'POST', {
      text: "printf '\\033]7;file://elsewhere.example/tmp\\007'",
      keys: ['Enter'],
    });
    for (let i = 0; i < 200 && !(await paneState(remote.pane_id)).remote; i++) await page.waitForTimeout(50);
    assert.equal((await paneState(remote.pane_id)).remote, true);
    await page.keyboard.press('Control+a');
    await page.keyboard.press(':');
    await page.keyboard.type('editor quick open');
    await page.keyboard.press('Enter');
    await page.getByText('Quick open needs a folder on this machine').first().waitFor();
    assert.equal((await paneState(remote.pane_id)).shown, false);
  } finally {
    await api(`/api/sessions/${remote.id}`, 'DELETE');
    rmSync(link, { force: true });
  }
  await page.goto(`${url}/s/${encodeURIComponent(session.name)}`);
  await input.waitFor();
  console.log('PASS quick open over a remote cwd shows a message, not the editor');

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
    await page.locator(`[data-editor-pane="${paneId}"] [data-lsp-state="ready"]`).waitFor();
    await page.getByText('1 error, 0 warnings').waitFor();
    // Lenses and hints move text as they arrive; positions are measured after.
    await page.locator(`[data-editor-pane="${paneId}"] .cm-lsp-code-lens`).first().waitFor();
    console.log('PASS diagnostics from the language server');

    // A narrow pane's status line drops its least useful items rather than clip.
    const splits: { pane_id: string }[] = [];
    splits.push(await api(`/api/panes/${paneId}/split`, 'POST', { direction: 'right', select: false }));
    const statusLine = () =>
      page.getByLabel('Language server fake').evaluate((server) => {
        const line = server.closest('[data-editor-status]')!;
        return {
          server: getComputedStyle(server.querySelector('[data-lsp-name]')!).display,
          dot: getComputedStyle(server.firstElementChild!).display,
          // On one line, without clipping.
          fits: line.scrollWidth <= line.clientWidth && line.clientHeight < 30,
          width: line.clientWidth,
          text: line.textContent,
        };
      });
    let narrow = await statusLine();
    for (let i = 0; i < 100 && !(narrow.width < 350 && narrow.server === 'none' && narrow.fits); i++) {
      await page.waitForTimeout(50);
      narrow = await statusLine();
    }
    assert.ok(
      narrow.width < 350 && narrow.fits && narrow.dot !== 'none' && narrow.text!.includes('1 error, 0 warnings'),
      JSON.stringify(narrow),
    );
    for (const { pane_id } of splits) await api(`/api/panes/${pane_id}`, 'DELETE');
    await page.getByLabel('Language server fake').waitFor();
    console.log('PASS a narrow status line fits');

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
    await page.getByText('1 error, 0 warnings').waitFor();
    console.log('PASS edits reach the language server incrementally');

    await page.keyboard.press('End');
    await page.keyboard.press('Enter');
    await page.keyboard.type('fa');
    // Long docs scroll in a box that fits the window, with markdown
    // paragraphs a line apart, not three.
    const info = page.locator('.cm-completionInfo');
    await info.getByText('Completion paragraph 0.').waitFor();
    const infoBox = await info.evaluate((el) => {
      el.scrollTop = 100;
      const [first, second] = Array.from(el.querySelectorAll('p'), (p) => p.getBoundingClientRect());
      const r = el.getBoundingClientRect();
      return { top: r.top, bottom: r.bottom, scrollTop: el.scrollTop, gap: second.top - first.bottom, viewport: innerHeight };
    });
    assert.ok(infoBox.scrollTop > 0 && infoBox.top >= 0 && infoBox.bottom <= infoBox.viewport, JSON.stringify(infoBox));
    assert.ok(infoBox.gap < 10, JSON.stringify(infoBox));
    await page.locator('.cm-tooltip-autocomplete .cm-completionLabel', { hasText: 'fakeCompletion' }).click();
    await waitForText(page, paneId, 'let bad = 1;\nuse thing;\nfakeCompletion\n');
    console.log('PASS completion');

    // Esc hides signature help until it is asked for, a call starts, or it
    // moves to another parameter. Escs are apart, as two quickly hide the editor.
    const signature = page.locator('.cm-lsp-signature-tooltip');
    const escape = async () => {
      await page.waitForTimeout(400);
      await page.keyboard.press('Escape');
      await signature.waitFor({ state: 'hidden' });
    };
    await page.keyboard.type(' sig(');
    await signature.waitFor();
    await escape();
    // Typing in the same parameter keeps it hidden.
    await page.keyboard.type('x');
    await page.waitForTimeout(400);
    assert.equal(await signature.isVisible(), false);
    await page.keyboard.press('ControlOrMeta+Shift+Space');
    await signature.waitFor();
    await escape();
    // A new call to it, at the same parameter: lsp-client keeps the tooltip.
    await page.keyboard.type(') sig(');
    await signature.waitFor();
    await escape();
    await page.keyboard.type('y,');
    await signature.waitFor();
    await page.keyboard.press('End');
    await page.keyboard.press('Shift+Home');
    await page.keyboard.type('fakeCompletion');
    await waitForText(page, paneId, 'let bad = 1;\nuse thing;\nfakeCompletion\n');
    await signature.waitFor({ state: 'detached' });
    console.log('PASS Esc closes signature help');

    const hover = (await wordRect(1, 'thing'))!;
    await page.mouse.move(hover.x, hover.y);
    await page.locator('.cm-tooltip').getByText('thing', { exact: true }).waitFor();
    console.log('PASS hover');

    // A hover taller than the space left scrolls inside its box.
    const use = (await wordRect(1, 'use'))!;
    await page.mouse.move(use.x, use.y);
    const longHover = page.locator('.cm-tooltip', { hasText: 'Long hover paragraph 0.' });
    await longHover.waitFor();
    const longBox = await longHover.evaluate((el) => {
      el.scrollTop = 100;
      const r = el.getBoundingClientRect();
      return { top: r.top, bottom: r.bottom, scrollTop: el.scrollTop, viewport: innerHeight };
    });
    assert.ok(longBox.scrollTop > 0 && longBox.top >= 0 && longBox.bottom <= longBox.viewport, JSON.stringify(longBox));
    await page.mouse.move(0, 0);
    await longHover.waitFor({ state: 'detached' });
    console.log('PASS a long hover scrolls');

    // F12 jumps to the definition in another file, opened in this pane.
    const definition = (await wordRect(1, 'thing'))!;
    await page.mouse.click(definition.x, definition.y);
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
    // didSave follows the write.
    const lspLog = () => readFileSync(join(lspDir, 'log'), 'utf8');
    for (let i = 0; i < 100 && !/textDocument\/didSave/.test(lspLog()); i++) await page.waitForTimeout(50);
    assert.match(lspLog(), /textDocument\/didSave/);
    console.log('PASS format on save');

    // A format answered after switching files changes the file it was for.
    const slow = join(dir, 'slow.fake');
    const two = join(dir, 'two.fake');
    writeFileSync(slow, 'slow   \n');
    writeFileSync(two, 'two\n');
    await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: slow, line: null, editor: 'native' });
    await waitForText(page, paneId, 'slow   \n');
    const formats = () =>
      (readFileSync(join(lspDir, 'log'), 'utf8').match(/^textDocument\/formatting$/gm) ?? []).length;
    const asked = formats();
    await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
    await page.keyboard.press('ControlOrMeta+s');
    for (let i = 0; i < 100 && formats() === asked; i++) await page.waitForTimeout(20);
    await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: two, line: null, editor: 'native' });
    await waitForText(page, paneId, 'two\n');
    for (let i = 0; i < 100 && readFileSync(slow, 'utf8') !== 'slow\n'; i++) await page.waitForTimeout(50);
    assert.equal(readFileSync(slow, 'utf8'), 'slow\n');
    assert.equal(await editorText(page, paneId), 'two\n');
    assert.equal(readFileSync(two, 'utf8'), 'two\n');
    console.log('PASS formatting follows its file when the view switches');

    // A server that isn't installed shows in the status line, and its features stay off.
    const lspConfig = [
      '[language-server.missing]',
        'command = "btmux-missing-lsp"',
        '[language-server.stall]',
        'command = "fake-lsp"',
        'timeout = 2',
        '[language-server.crashinit]',
        'command = "fake-lsp"',
        'args = ["--crash-init"]',
        '[language-server.noinit]',
        'command = "fake-lsp"',
        'args = ["--no-init"]',
        'timeout = 2',
        '[[language]]',
        'name = "nolsp"',
        'file-types = ["nolsp"]',
        'language-servers = ["missing"]',
        '[[language]]',
        'name = "stall"',
        'file-types = ["stall"]',
        'language-servers = ["stall"]',
        '[[language]]',
        'name = "crashinit"',
        'file-types = ["crashinit"]',
        'language-servers = ["crashinit"]',
        '[[language]]',
        'name = "noinit"',
        'file-types = ["noinit"]',
      'language-servers = ["noinit"]',
      '',
    ].join('\n');
    writeFileSync(join(configHome, 'btmux', 'config.toml'), lspConfig);
    await waitForAsync(page, async () => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      return useStore.getState().config?.format_on_save === false;
    });
    const nolsp = join(dir, 'x.nolsp');
    writeFileSync(nolsp, 'plain\n');
    await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: nolsp, line: null, editor: 'native' });
    await waitForText(page, paneId, 'plain\n');
    // The dot's tooltip has the same text.
    await page.getByLabel('Language server missing').getByText('missing: btmux-missing-lsp not found').waitFor();
    await page.locator(`[data-editor-pane="${paneId}"] [data-lsp-state="failed"]`).waitFor();
    const plain = (await wordRect(0, 'plain'))!;
    await page.mouse.move(plain.x, plain.y);
    await page.waitForTimeout(1000);
    assert.equal(await page.locator('.cm-lsp-message-error').count(), 0);
    console.log('PASS a missing language server shows in the status line without errors');

    // A server silent past its timeout on a request isn't responding, until it answers.
    const stall = join(dir, 'x.stall');
    writeFileSync(stall, 'stall\n');
    await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: stall, line: null, editor: 'native' });
    await waitForText(page, paneId, 'stall\n');
    const dot = (state: string) => page.locator(`[data-editor-pane="${paneId}"] [data-lsp-state="${state}"]`);
    await dot('ready').waitFor();
    const word = (await wordRect(0, 'stall'))!;
    await page.mouse.move(word.x, word.y);
    await dot('unresponsive').waitFor({ timeout: 5000 });
    await page.getByLabel('Language server stall').getByText('stall: not responding').waitFor();
    await dot('ready').waitFor({ timeout: 8000 });
    console.log('PASS a silent language server shows as not responding until it answers');

    // A tab whose initialization failed (the server crashed starting) initializes again once it's ready.
    const crashinit = join(dir, 'x.crashinit');
    writeFileSync(crashinit, 'hover\n');
    await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: crashinit, line: null, editor: 'native' });
    await waitForText(page, paneId, 'hover\n');
    await dot('ready').waitFor();
    const hovered = (await wordRect(0, 'hover'))!;
    // The pointer is still there from the last file.
    await page.mouse.move(0, 0);
    await page.mouse.move(hovered.x, hovered.y);
    await page.locator('.cm-tooltip').getByText('Fake hover for').waitFor();
    await page.mouse.move(0, 0);
    console.log('PASS a tab initializes again after its server restarts');

    // A working server's dot pulses, unless animations are off.
    const busy = join(dir, 'busy.stall');
    writeFileSync(busy, 'busy\n');
    await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: busy, line: null, editor: 'native' });
    await waitForText(page, paneId, 'busy\n');
    await dot('busy').waitFor();
    const pulses = () => dot('busy').evaluate((button) => button.getAnimations({ subtree: true }).length);
    assert.ok((await pulses()) > 0);
    writeFileSync(join(configHome, 'btmux', 'config.toml'), `animations = false\n${lspConfig}`);
    await waitForAsync(page, async () => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      return useStore.getState().config?.animations === false;
    });
    for (let i = 0; i < 50 && (await pulses()) > 0; i++) await page.waitForTimeout(20);
    assert.equal(await pulses(), 0);
    console.log('PASS dots pulse only with animations');

    // The status is reachable by keyboard: focus shows its tooltip.
    await page.keyboard.press('Shift');
    await dot('busy').focus();
    await page.getByRole('tooltip').filter({ hasText: 'stall: Indexing' }).waitFor();
    await dot('busy').blur();
    console.log("PASS a server's status shows on keyboard focus");

    // One that doesn't answer `initialize` in time fails.
    const noinit = join(dir, 'x.noinit');
    writeFileSync(noinit, 'plain\n');
    await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: noinit, line: null, editor: 'native' });
    await waitForText(page, paneId, 'plain\n');
    await dot('failed').waitFor({ timeout: 8000 });
    await page.getByLabel('Language server noinit').getByText("noinit: noinit didn't start within 2 s").waitFor();
    console.log('PASS a language server that never starts fails after its timeout');
  }

  // Vim mode, from config.toml.
  if (configHome) {
    const config = join(configHome, 'btmux', 'config.toml');
    const vimDir = join(dir, 'vim');
    mkdirSync(vimDir, { recursive: true });
    mkdirSync(join(configHome, 'btmux'), { recursive: true });
    const vimFile = join(vimDir, 'vim.txt');
    writeFileSync(vimFile, 'alpha\nbeta\n');
    const lspConfig = lspDir
      ? [
          '[language-server.fake]',
          'command = "fake-lsp"',
          '[[language]]',
          'name = "fake"',
          'file-types = ["fake"]',
          'language-servers = ["fake"]',
        ]
      : [];
    // The vim package loads only once vim is turned on.
    const vimLoaded = () => requested.some((url) => /codemirror-vim|nativeEditor\/vim\.ts/.test(url));
    assert.equal(vimLoaded(), false);
    writeFileSync(config, ['[native-editor]', 'vim = true', ...lspConfig, ''].join('\n'));
    const vimMode = (on: boolean) =>
      waitForAsync(
        page,
        async (on) => {
          const { useStore } = await import('/src/state/store.tsx' as string);
          return useStore.getState().config?.vim_mode === on;
        },
        on,
      );
    await vimMode(true);
    await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: vimFile, line: 1, editor: 'native' });
    await waitForText(page, paneId, 'alpha\nbeta\n');
    const content = page.locator(`[data-native-editor][data-pane-id="${paneId}"]`);
    const mode = page.locator(`[data-editor-pane="${paneId}"] [aria-label="Vim mode"]`);
    const showsMode = (label: string, p = page) =>
      p.locator(`[data-editor-pane="${paneId}"] [aria-label="Vim mode"]`).getByText(label, { exact: true }).waitFor();
    await content.click();
    await page.keyboard.press('g');
    await page.keyboard.press('g');
    await showsMode('NORMAL');
    const cursor = page.locator(`[data-editor-pane="${paneId}"] .cm-vimCursorLayer .cm-fat-cursor`);
    await cursor.waitFor();
    // The theme's cursor color, not codemirror-vim's default.
    const [background, themed] = await cursor.evaluate(async (el) => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      const probe = document.createElement('div');
      probe.style.color = useStore.getState().config.theme.cursor;
      document.body.append(probe);
      const themed = getComputedStyle(probe).color;
      probe.remove();
      return [getComputedStyle(el).backgroundColor, themed];
    });
    assert.equal(background, themed);
    assert.equal(vimLoaded(), true);
    console.log('PASS vim mode shows NORMAL and a block cursor, loading vim on demand');

    await page.keyboard.type('ihello ');
    await waitForText(page, paneId, 'hello alpha\nbeta\n');
    await showsMode('INSERT');
    // A double Esc in insert mode only leaves it.
    await page.keyboard.press('Escape');
    await page.keyboard.press('Escape');
    await page.waitForTimeout(400);
    assert(await content.isVisible(), 'a double Esc in insert mode keeps the editor');
    await showsMode('NORMAL');
    console.log('PASS insert mode, and a double Esc in it keeps the editor');

    // The mobile key bar's Esc: a synthetic keydown on the editor.
    await page.keyboard.press('A');
    await showsMode('INSERT');
    await page.evaluate(async (id) => {
      const { dispatchKey } = await import('/src/lib/paneTerminal.ts' as string);
      dispatchKey(document.querySelector(`[data-native-editor][data-pane-id="${id}"]`)!, 'Escape');
    }, paneId);
    await showsMode('NORMAL');
    console.log('PASS a synthetic Esc (mobile key bar) leaves insert mode');

    // Quick open's Ctrl-P (off macOS) wins over vim's in normal mode.
    const linux = await openPage((context) =>
      context.addInitScript(() => Object.defineProperty(Navigator.prototype, 'platform', { get: () => 'Linux x86_64' })),
    );
    await waitForText(linux, paneId, 'hello alpha\nbeta\n');
    await linux.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
    await showsMode('NORMAL', linux);
    await linux.keyboard.press('Control+p');
    await linux.locator(`[data-editor-pane="${paneId}"]`).getByRole('combobox', { name: 'Go to file' }).waitFor();
    await linux.context().close();
    console.log('PASS Ctrl-P opens quick open in vim normal mode');

    const command = async (text: string) => {
      await page.keyboard.press(':');
      await page.locator(`[data-editor-pane="${paneId}"] .cm-vim-panel input`).waitFor();
      await page.keyboard.type(text);
      await page.keyboard.press('Enter');
    };
    const unsaved = page.getByRole('tab', { name: 'vim.txt' }).locator('[aria-label="Unsaved"]');
    await unsaved.waitFor();
    await command('w');
    await unsaved.waitFor({ state: 'detached' });
    assert.equal(readFileSync(vimFile, 'utf8'), 'hello alpha\nbeta\n');
    console.log('PASS :w saves to disk');

    // The first Esc clears a search highlight, so Esc Esc doesn't hide.
    await page.keyboard.press('/');
    await page.locator(`[data-editor-pane="${paneId}"] .cm-vim-panel input`).waitFor();
    await page.keyboard.type('beta');
    await page.keyboard.press('Enter');
    await page.locator(`[data-editor-pane="${paneId}"] .cm-searchMatch`).first().waitFor();
    await page.waitForTimeout(400);
    await page.keyboard.press('Escape');
    await page.keyboard.press('Escape');
    await page.waitForTimeout(400);
    assert(await content.isVisible(), 'Esc Esc after a search keeps the editor');
    assert.equal(await page.locator(`[data-editor-pane="${paneId}"] .cm-searchMatch`).count(), 0);
    await page.keyboard.type('gg');
    console.log('PASS the first Esc after a search clears its highlight');

    await page.keyboard.type('dd');
    await waitForText(page, paneId, 'beta\n');
    await page.keyboard.type('u');
    await waitForText(page, paneId, 'hello alpha\nbeta\n');
    console.log('PASS dd and u');

    // A reopen (a new state) in insert mode stays in insert mode.
    await page.keyboard.type('ggi');
    await showsMode('INSERT');
    const reopened = () =>
      waitForAsync(
        page,
        async (path) => {
          const { editorConnection } = await import('/src/lib/nativeEditor/connection.ts' as string);
          const doc = [...(editorConnection as any).listeners].find((l: any) => l.path === path);
          return doc.status.ready && doc.view && doc.view.state !== (window as any).beforeReopen;
        },
        vimFile,
      );
    await page.evaluate(async (path) => {
      const { editorConnection } = await import('/src/lib/nativeEditor/connection.ts' as string);
      const doc = [...(editorConnection as any).listeners].find((l: any) => l.path === path);
      (window as any).beforeReopen = doc.view.state;
      // As when the update log no longer reaches its version: a fresh state.
      doc.resume = () => null;
      editorConnection.reopen(doc);
      delete doc.resume;
    }, vimFile);
    await reopened();
    await page.keyboard.type('Q');
    await waitForText(page, paneId, 'Qhello alpha\nbeta\n');
    await showsMode('INSERT');
    await page.keyboard.press('Backspace');
    await page.keyboard.press('Escape');
    await waitForText(page, paneId, 'hello alpha\nbeta\n');
    console.log('PASS a reopen keeps insert mode');

    // Each tab has its own mode; edits still reach both.
    const second = await openPage();
    await waitForText(second, paneId, 'hello alpha\nbeta\n');
    await content.click();
    await page.keyboard.type('ggOtop');
    await showsMode('INSERT');
    await waitForText(second, paneId, 'top\nhello alpha\nbeta\n');
    await showsMode('NORMAL', second);
    await page.keyboard.press('Escape');
    await second.context().close();
    console.log('PASS vim state is per tab; edits go through the server');

    // From normal mode, a double Esc hides the editor, as does :q.
    await page.waitForTimeout(400);
    await page.keyboard.press('Escape');
    await page.keyboard.press('Escape');
    await content.waitFor({ state: 'hidden' });
    await page.keyboard.press('Control+a');
    await page.keyboard.press('e');
    await content.waitFor({ state: 'visible' });
    await content.click();
    await command('q');
    await content.waitFor({ state: 'hidden' });
    await page.keyboard.press('Control+a');
    await page.keyboard.press('e');
    await content.waitFor({ state: 'visible' });
    await content.click();
    await command('wq');
    await content.waitFor({ state: 'hidden' });
    assert.equal(readFileSync(vimFile, 'utf8'), 'top\nhello alpha\nbeta\n');
    console.log('PASS a double Esc from normal mode, :q, and :wq hide the editor');

    const diskIs = async (path: string, text: string) => {
      for (let i = 0; i < 100 && readFileSync(path, 'utf8') !== text; i++) await page.waitForTimeout(50);
      assert.equal(readFileSync(path, 'utf8'), text);
    };
    // A failed write keeps the editor, even for a clean file.
    await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: vimFile, line: 1, editor: 'native' });
    await content.waitFor({ state: 'visible' });
    await waitForText(page, paneId, 'top\nhello alpha\nbeta\n');
    chmodSync(vimFile, 0o444);
    await content.click();
    await command('wq');
    await page.waitForTimeout(1000);
    chmodSync(vimFile, 0o644);
    assert(await content.isVisible(), ':wq keeps the editor when the write fails');
    // So does one while disconnected, with edits the server doesn't have.
    await page.evaluate(async () => {
      const { editorConnection } = await import('/src/lib/nativeEditor/connection.ts' as string);
      const connection = editorConnection as any;
      connection.held = connection.connect;
      connection.connect = () => (connection.reconnectTimer = null);
      connection.ws.close();
    });
    await page.waitForTimeout(200);
    await page.keyboard.type('ggAY');
    await page.keyboard.press('Escape');
    await command('wq');
    await page.waitForTimeout(1000);
    assert(await content.isVisible(), ':wq keeps the editor while disconnected');
    assert.equal(readFileSync(vimFile, 'utf8'), 'top\nhello alpha\nbeta\n');
    await page.evaluate(async () => {
      const { editorConnection } = await import('/src/lib/nativeEditor/connection.ts' as string);
      const connection = editorConnection as any;
      connection.connect = connection.held;
      connection.connect();
    });
    // Saves made before the edits are sent again are refused, so retry.
    for (let i = 0; i < 40 && readFileSync(vimFile, 'utf8') !== 'topY\nhello alpha\nbeta\n'; i++) {
      await command('w');
      await page.waitForTimeout(250);
    }
    await diskIs(vimFile, 'topY\nhello alpha\nbeta\n');
    await page.keyboard.type('u');
    await waitForText(page, paneId, 'top\nhello alpha\nbeta\n');
    await command('w');
    await diskIs(vimFile, 'top\nhello alpha\nbeta\n');
    // And when the server dropped the edit as stale (another tab got there first).
    await page.evaluate(async () => {
      const { editorConnection } = await import('/src/lib/nativeEditor/connection.ts' as string);
      const connection = editorConnection as any;
      connection.realPush = connection.push;
      connection.push = (buffer: string, version: number, updates: unknown[]) =>
        connection.realPush.call(connection, buffer, version - 1, updates);
    });
    await page.keyboard.type('ggAZ');
    await page.keyboard.press('Escape');
    await waitForText(page, paneId, 'topZ\nhello alpha\nbeta\n');
    await command('wq');
    await page.waitForTimeout(1000);
    assert(await content.isVisible(), ':wq keeps the editor when its edit was dropped');
    assert.equal(readFileSync(vimFile, 'utf8'), 'top\nhello alpha\nbeta\n');
    await page.evaluate(async () => {
      const { editorConnection } = await import('/src/lib/nativeEditor/connection.ts' as string);
      const connection = editorConnection as any;
      connection.push = connection.realPush;
    });
    // The next edit sends both again.
    await page.keyboard.type('AW');
    await page.keyboard.press('Escape');
    await command('w');
    await diskIs(vimFile, 'topZW\nhello alpha\nbeta\n');
    await page.keyboard.type('u');
    await page.keyboard.type('u');
    await waitForText(page, paneId, 'top\nhello alpha\nbeta\n');
    await command('w');
    await diskIs(vimFile, 'top\nhello alpha\nbeta\n');
    console.log('PASS :wq keeps the editor when the save fails or edits are unsent');

    if (lspDir) {
      writeFileSync(join(vimDir, 'main.fake'), 'use thing;\nlet bad = 1;\n');
      writeFileSync(join(vimDir, 'other.fake'), 'first\nthing defined here\n');
      await api(`/api/panes/${paneId}/open-editor`, 'POST', {
        path: join(vimDir, 'main.fake'),
        line: 1,
        editor: 'native',
      });
      await waitForText(page, paneId, 'use thing;\nlet bad = 1;\n');
      await page.locator(`[data-editor-pane="${paneId}"] .cm-lintRange-error`).first().waitFor();
      await content.click();
      await page.keyboard.type('gg');
      await page.keyboard.type(']d');
      await page.locator(`[data-editor-pane="${paneId}"]`).getByText('Ln 2, Col 5', { exact: true }).waitFor();
      await page.keyboard.type('gg');
      await page.keyboard.type('w');
      await page.keyboard.type('K');
      await page.locator('.cm-tooltip').getByText('Fake hover for').waitFor();
      await page.keyboard.type('gd');
      await waitForText(page, paneId, 'first\nthing defined here\n');
      // Alt chords reach the editor's keymaps (vim takes them on macOS).
      await page.keyboard.press('Shift+Alt+h');
      await page.locator(`[data-editor-pane="${paneId}"]`).getByRole('listbox', { name: 'Calls to thing' }).waitFor();
      await page.keyboard.press('Escape');
      // Ctrl-[ leaves insert mode and closes completions, as Esc does.
      await content.click();
      await page.keyboard.type('Gofa');
      await page.locator('.cm-tooltip-autocomplete').waitFor();
      await page.keyboard.press('Control+BracketLeft');
      await showsMode('NORMAL');
      await page.locator('.cm-tooltip-autocomplete').waitFor({ state: 'detached' });
      for (let i = 0; i < 3 && (await editorText(page, paneId)) !== 'first\nthing defined here\n'; i++) {
        await page.keyboard.type('u');
        await page.waitForTimeout(100);
      }
      await waitForText(page, paneId, 'first\nthing defined here\n');
      console.log('PASS ]d, K, gd, and Alt chords use the language server; Ctrl-[ closes completions');

      writeFileSync(config, ['[native-editor]', 'vim = true', 'format-on-save = true', ...lspConfig, ''].join('\n'));
      await waitForAsync(page, async () => {
        const { useStore } = await import('/src/state/store.tsx' as string);
        return useStore.getState().config?.format_on_save === true;
      });
      const slow = join(vimDir, 'slow.fake');
      writeFileSync(slow, 'slow   \n');
      await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: slow, line: 1, editor: 'native' });
      await waitForText(page, paneId, 'slow   \n');
      // A hover answered (700 ms late) after Esc isn't shown.
      await content.click();
      await page.keyboard.type('gg');
      await page.keyboard.type('K');
      await page.keyboard.press('Escape');
      await page.waitForTimeout(1200);
      assert.equal(await page.locator('.cm-tooltip', { hasText: 'Fake hover for' }).count(), 0);
      await page.keyboard.type('K');
      await page.locator('.cm-tooltip').getByText('Fake hover for').waitFor();
      await page.keyboard.press('Escape');
      console.log('PASS a late hover after Esc is dropped');

      // Switching files while :wq formats (700 ms) keeps the editor.
      await command('wq');
      await page.getByRole('tab', { name: 'vim.txt' }).click();
      await waitForText(page, paneId, 'top\nhello alpha\nbeta\n');
      await diskIs(slow, 'slow\n');
      await page.waitForTimeout(300);
      assert(await content.isVisible(), 'a late :wq leaves a newly shown file alone');
      console.log('PASS a slow :wq keeps the editor after switching files');
    }

    // :bd closes the shown file, asking about unsaved changes; :bd! discards them.
    const bdFile = join(vimDir, 'bd.txt');
    writeFileSync(bdFile, 'keep\n');
    const bdTab = page.getByRole('tab', { name: 'bd.txt' });
    const openBd = async () => {
      await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: bdFile, line: 1, editor: 'native' });
      await content.waitFor({ state: 'visible' });
      await waitForText(page, paneId, 'keep\n');
      await content.click();
    };
    await openBd();
    await page.keyboard.type('Ax');
    await page.keyboard.press('Escape');
    await bdTab.locator('[aria-label="Unsaved"]').waitFor();
    await command('bd');
    const saveChanges = page.getByRole('dialog', { name: 'Save changes to bd.txt?' });
    await saveChanges.waitFor();
    await saveChanges.getByRole('button', { name: 'Cancel', exact: true }).click();
    await saveChanges.waitFor({ state: 'hidden' });
    await bdTab.waitFor();
    await content.click();
    await command('bd!');
    await bdTab.waitFor({ state: 'detached' });
    assert.equal(readFileSync(bdFile, 'utf8'), 'keep\n');
    await openBd();
    await command('bd');
    await bdTab.waitFor({ state: 'detached' });
    assert.equal(await saveChanges.count(), 0);
    console.log('PASS :bd closes the file, asking about unsaved changes; :bd! discards them');

    // Turning it off applies live, without remounting the editor.
    await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: vimFile, line: 1, editor: 'native' });
    await waitForText(page, paneId, 'top\nhello alpha\nbeta\n');
    const handle = await content.elementHandle();
    writeFileSync(config, lspConfig.join('\n'));
    await vimMode(false);
    await mode.waitFor({ state: 'detached' });
    await content.click();
    await page.keyboard.press('ControlOrMeta+Home');
    await page.keyboard.type('ix');
    await waitForText(page, paneId, 'ixtop\nhello alpha\nbeta\n');
    assert.equal(await handle!.evaluate((el) => el.isConnected), true);
    assert.equal(await page.locator(`[data-editor-pane="${paneId}"] .cm-vimCursorLayer`).count(), 0);
    console.log('PASS turning vim off applies live');

    // Alt-W closes the file, asking about unsaved changes.
    await page.getByRole('tab', { name: 'vim.txt' }).locator('[aria-label="Unsaved"]').waitFor();
    await page.keyboard.press('Alt+w');
    const saveVim = page.getByRole('dialog', { name: 'Save changes to vim.txt?' });
    await saveVim.getByRole('button', { name: "Don't save", exact: true }).click();
    await page.getByRole('tab', { name: 'vim.txt' }).waitFor({ state: 'detached' });
    assert.equal(readFileSync(vimFile, 'utf8'), 'top\nhello alpha\nbeta\n');
    console.log('PASS Alt-W closes the file');

    // It goes by the physical key with Caps Lock or another layout.
    for (const key of ['W', 'ц']) {
      await openBd();
      await content.evaluate((el, key) => {
        const init = { key, code: 'KeyW', keyCode: 87, altKey: true, bubbles: true, cancelable: true };
        el.dispatchEvent(new KeyboardEvent('keydown', init));
      }, key);
      await bdTab.waitFor({ state: 'detached' });
    }
    console.log('PASS Alt-W closes the file with Caps Lock or another layout');
  }
} finally {
  await browser.close();
  await api(`/api/sessions/${session.id}`, 'DELETE').catch(() => {});
  rmSync(dir, { recursive: true, force: true });
}
