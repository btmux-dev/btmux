/** Run with `bun browser-test-stack.ts native-editor-git-browser-test.ts`. */
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { chromium, type Page } from 'playwright';
import { waitForAsync } from './browser-test-utils';

const url = process.env.BTMUX_TEST_URL ?? 'http://localhost:5173';
const token = process.env.BTMUX_AUTH_TOKEN;
assert(token, 'BTMUX_AUTH_TOKEN is required');
const headers = { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' };
const api = async (route: string, method = 'GET', body?: unknown) => {
  const response = await fetch(`${url}${route}`, { headers, method, body: body ? JSON.stringify(body) : undefined });
  assert(response.ok, `${route}: ${response.status}`);
  const text = await response.text();
  return text ? JSON.parse(text) : null;
};

const repo = realpathSync(mkdtempSync(path.join(tmpdir(), 'btmux-gitsigns-')));
const git = (...args: string[]) =>
  execFileSync('git', ['-c', 'user.name=Ada', '-c', 'user.email=ada@example.invalid', ...args], {
    cwd: repo,
    env: { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1' },
  }).toString();
const file = path.join(repo, 'f.txt');
git('init', '--initial-branch=main');
writeFileSync(file, 'one\ntwo\nthree\nfour\nfive\nsix\nseven\n');
git('add', '.');
// A body longer than the window, which the blame tooltip scrolls.
git('commit', '-m', 'Add f', '-m', ['With a body', ...Array.from({ length: 80 }, (_, i) => `Line ${i}`)].join('\n'));
// A changed line, an added one, and a removed one.
writeFileSync(file, 'one\nTWO\nthree\nfour\nnew\nfive\nseven\n');

const session: { id: string; name: string } = await api('/api/sessions', 'POST', {
  name: `gitsigns-${Date.now()}`,
  cwd: repo,
});
const browser = await chromium.launch({ headless: true, channel: process.env.BTMUX_TEST_BROWSER });

/** `line:kind` for each sign, by 1-based line. */
const signs = (page: Page) =>
  page.evaluate(() => {
    const lines = Array.from(document.querySelectorAll('[data-native-editor] > .cm-line'), (l) =>
      Math.round(l.getBoundingClientRect().top),
    );
    // Not the hidden spacer that sizes the gutter.
    const shown = Array.from(document.querySelectorAll('.cm-git-gutter .cm-git-sign')).filter(
      (sign) => (sign.parentElement as HTMLElement).style.visibility !== 'hidden',
    );
    return shown.map((sign) => {
      const top = Math.round(sign.parentElement!.getBoundingClientRect().top);
      return `${lines.indexOf(top) + 1}:${sign.className.replace('cm-git-sign cm-git-', '')}`;
    });
  });

const waitFor = async <T>(read: () => Promise<T>, expected: T) => {
  for (let i = 0; i < 100; i++) {
    try {
      assert.deepEqual(await read(), expected);
      return;
    } catch {
      await new Promise((r) => setTimeout(r, 50));
    }
  }
  assert.deepEqual(await read(), expected);
};

try {
  const page = await browser.newPage({ viewport: { width: 1200, height: 800 } });
  page.on('pageerror', (error) => console.error('page error:', error.message));
  await page.goto(url);
  await page.getByLabel('Access token').fill(token);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await page.waitForFunction(() => !document.querySelector('#access-token'));
  await page.goto(`${url}/s/${encodeURIComponent(session.name)}`);
  await waitForAsync(page, async () => {
    const { useStore } = await import('/src/state/store.tsx' as string);
    return useStore.getState().controlConnectionState === 'connected';
  });
  const paneId: string = await page.evaluate(async (id) => {
    const { useStore } = await import('/src/state/store.tsx' as string);
    return useStore.getState().getActivePaneId(id);
  }, session.id);
  const run = (action: string) =>
    page.evaluate(
      async ({ paneId, action }) => {
        const { runEditorCommand } = await import('/src/lib/nativeEditor/registry.ts' as string);
        runEditorCommand(paneId, action, () => console.error(`${action} failed`));
      },
      { paneId, action },
    );
  const cursorLine = () => page.evaluate(() => document.querySelector('.cm-activeLine')?.textContent ?? null);

  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: file, line: 1, editor: 'native' });
  const content = page.locator(`[data-native-editor][data-pane-id="${paneId}"]`);
  await content.waitFor();

  // Signs: changed line 2, added line 5, and "six" removed under line 6.
  await waitFor(() => signs(page), ['2:change', '5:add', '6:delete']);
  await page.getByLabel('Changes from the Git index').getByText('+1 ~1 -1').waitFor();
  console.log('PASS signs mark lines changed from the index');

  // Typing updates them.
  await content.click();
  await page.keyboard.press('ControlOrMeta+Home');
  await page.keyboard.press('End');
  await page.keyboard.type('!');
  await waitFor(() => signs(page), ['1:change', '2:change', '5:add', '6:delete']);
  await page.keyboard.press('Backspace');
  await waitFor(() => signs(page), ['2:change', '5:add', '6:delete']);
  console.log('PASS signs follow edits');

  // Alt-F5 moves through hunks, wrapping.
  await page.keyboard.press('Alt+F5');
  await waitFor(cursorLine, 'TWO');
  await page.keyboard.press('Alt+F5');
  await waitFor(cursorLine, 'new');
  await page.keyboard.press('Alt+F5');
  await waitFor(cursorLine, 'five');
  await page.keyboard.press('Alt+F5');
  await waitFor(cursorLine, 'TWO');
  await page.keyboard.press('Shift+Alt+F5');
  await waitFor(cursorLine, 'five');
  console.log('PASS hunk navigation wraps');

  // Clicking a sign previews the hunk inline, with the removed line.
  await page.locator('.cm-git-change').first().click();
  const preview = page.locator('.cm-git-preview');
  await preview.waitFor();
  assert.equal(
    await preview
      .locator('.cm-git-removed-line')
      .allTextContents()
      .then((l) => l.join('|')),
    'two',
  );
  assert.match((await preview.locator('.cm-git-preview-bar span').textContent()) ?? '', /Hunk 1 of 3/);
  await page.locator('.cm-git-added-line').first().waitFor();

  // Its Stage button stages just that hunk.
  await preview.getByRole('button', { name: 'Stage' }).click();
  await waitFor(() => signs(page), ['5:add', '6:delete']);
  await waitFor(async () => git('show', ':f.txt'), 'one\nTWO\nthree\nfour\nfive\nsix\nseven\n');
  assert.equal(await preview.count(), 0);
  console.log('PASS the preview shows a hunk and stages it');

  // Reset puts the index text back in the buffer.
  await content.click();
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.press('Shift+Alt+F5');
  await waitFor(cursorLine, 'five');
  await run('editor-reset-hunk');
  await waitFor(() => signs(page), ['5:add']);
  await waitFor(cursorLine, 'five');
  console.log('PASS resetting a hunk restores the removed line');

  // Staging from outside btmux updates the signs.
  await page.keyboard.press('ControlOrMeta+s');
  await waitFor(async () => (await page.locator('[aria-label="Unsaved"]').count()) === 0, true);
  git('add', 'f.txt');
  await waitFor(() => signs(page), []);
  await waitFor(async () => (await page.getByLabel('Changes from the Git index').count()) === 0, true);
  console.log('PASS outside index changes reach the signs');

  // Line blame: committed lines name their author, new ones are uncommitted.
  await run('editor-toggle-line-blame');
  await content.click();
  await page.keyboard.press('ControlOrMeta+Home');
  await page.locator('.cm-git-blame', { hasText: 'Ada, just now · Add f' }).waitFor();
  await page.keyboard.press('ArrowDown');
  await page.locator('.cm-git-blame', { hasText: 'Uncommitted changes' }).waitFor();
  // A commit from outside btmux changes it in place.
  git('commit', '-q', '-m', 'Shout');
  await page.locator('.cm-git-blame', { hasText: 'Ada, just now · Shout' }).waitFor();
  // And the full commit in a tooltip.
  await page.keyboard.press('ArrowUp');
  await run('editor-blame-line');
  await page.locator('.cm-git-blame-message', { hasText: 'With a body' }).waitFor();
  // CodeMirror places a tooltip off screen until it measures it.
  await page.waitForFunction(() => {
    const el = document.querySelector('.cm-git-blame-tooltip');
    return !!el && el.getBoundingClientRect().top > -1000;
  });
  const blameBox = await page.locator('.cm-git-blame-tooltip').evaluate((el) => {
    el.scrollTop = 100;
    const r = el.getBoundingClientRect();
    return { top: r.top, bottom: r.bottom, scrollTop: el.scrollTop, viewport: innerHeight };
  });
  assert.ok(
    blameBox.scrollTop > 0 && blameBox.top >= 0 && blameBox.bottom <= blameBox.viewport,
    JSON.stringify(blameBox),
  );
  await page.keyboard.press('ArrowDown');
  await page.locator('.cm-git-blame-tooltip').waitFor({ state: 'detached' });
  await run('editor-toggle-line-blame');
  await page.locator('.cm-git-blame').waitFor({ state: 'detached' });
  console.log('PASS line blame and blame tooltips');

  // Untracked files are all added; staging the buffer adds the file.
  const untracked = path.join(repo, 'u.txt');
  writeFileSync(untracked, 'a\nb\n');
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: untracked, line: 1, editor: 'native' });
  await waitFor(() => signs(page), ['1:add', '2:add']);
  await run('editor-stage-buffer');
  await waitFor(() => signs(page), []);
  assert.equal(git('show', ':u.txt'), 'a\nb\n');
  console.log('PASS untracked files show as added and can be staged');

  // The preview's removed words follow an edit that keeps the hunk's size.
  const words = path.join(repo, 'w.txt');
  writeFileSync(words, 'hello world\n');
  git('add', 'w.txt');
  writeFileSync(words, 'HELLO world\n');
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: words, line: 1, editor: 'native' });
  await waitFor(() => signs(page), ['1:change']);
  await page.locator('.cm-git-change').first().click();
  const removedWords = () => preview.locator('.cm-git-removed-line del').allTextContents();
  await waitFor(removedWords, ['hello']);
  await content.click();
  await page.keyboard.press('ControlOrMeta+Home');
  await page.keyboard.press('Shift+End');
  await page.keyboard.insertText('hello WORLD');
  await waitFor(removedWords, ['world']);
  await page.keyboard.press('ControlOrMeta+z');
  console.log('PASS the preview marks the words the current text removed');

  // Vim gets gitsigns' suggested mappings, on the Space leader.
  const configHome = process.env.BTMUX_TEST_CONFIG_HOME;
  if (configHome) {
    mkdirSync(path.join(configHome, 'btmux'), { recursive: true });
    writeFileSync(path.join(configHome, 'btmux', 'config.toml'), '[native-editor]\nvim = true\n');
    await waitForAsync(page, async () => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      return useStore.getState().config?.vim_mode === true;
    });
    git('reset', '-q', 'HEAD~');
    writeFileSync(file, 'one\nTWO\nthree\nfour\nfive\nsix\nseven\nEIGHT\nNINE\n');
    await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: file, line: 1, editor: 'native' });
    await waitFor(() => signs(page), ['2:change', '8:add', '9:add']);
    await page.locator('[aria-label="Vim mode"]', { hasText: 'NORMAL' }).waitFor();
    await content.click();
    await page.keyboard.press('g');
    await page.keyboard.press('g');
    await page.keyboard.type(']c');
    await waitFor(cursorLine, 'TWO');
    await page.keyboard.type(']c');
    await waitFor(cursorLine, 'EIGHT');
    await page.keyboard.type('[c');
    await waitFor(cursorLine, 'TWO');
    await page.keyboard.type(' hp');
    await page.locator('.cm-git-preview .cm-git-removed-line', { hasText: 'two' }).waitFor();
    await page.keyboard.type(' hr');
    await waitFor(() => signs(page), ['8:add', '9:add']);
    // `dih` deletes the hunk's lines.
    await page.keyboard.type('G');
    await page.keyboard.type('kdih');
    await waitFor(() => signs(page), []);
    await waitFor(async () => (await content.textContent())?.endsWith('sixseven'), true);
    assert.equal(readFileSync(file, 'utf8'), 'one\nTWO\nthree\nfour\nfive\nsix\nseven\nEIGHT\nNINE\n');
    console.log('PASS vim mappings navigate, preview, reset, and select hunks');
  }
} finally {
  await browser.close();
  await api(`/api/sessions/${session.id}`, 'DELETE').catch(() => {});
  rmSync(repo, { recursive: true, force: true });
}
