/** Language server features of the native editor, with test-bin/fake-lsp. Run through browser-test-stack.ts. */
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { chromium, type Page } from 'playwright';
import { waitForAsync } from './browser-test-utils';

const url = process.env.BTMUX_TEST_URL ?? 'http://localhost:5173';
const token = process.env.BTMUX_AUTH_TOKEN;
const lspDir = process.env.BTMUX_TEST_LSP;
const configHome = process.env.BTMUX_TEST_CONFIG_HOME;
assert(token && lspDir && configHome, 'run through browser-test-stack.ts');
const headers = { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' };
const api = async (path: string, method = 'GET', body?: unknown) => {
  const response = await fetch(`${url}${path}`, { headers, method, body: body ? JSON.stringify(body) : undefined });
  assert(response.ok, `${path}: ${response.status}`);
  const text = await response.text();
  return text ? JSON.parse(text) : null;
};

const dir = realpathSync(mkdtempSync(join(tmpdir(), 'btmux-lsp-')));
const main = join(dir, 'main.fake');
writeFileSync(main, 'let thing = bad;\nuse thing;\nworse\nend\n');
writeFileSync(join(dir, 'other.fake'), 'thing defined here\n');
mkdirSync(lspDir, { recursive: true });
mkdirSync(join(configHome, 'btmux'), { recursive: true });
writeFileSync(
  join(configHome, 'btmux', 'config.toml'),
  '[language-server.fake]\ncommand = "fake-lsp"\n[[language]]\nname = "fake"\nfile-types = ["fake"]\nlanguage-servers = ["fake"]\n',
);
const log = () => {
  try {
    return readFileSync(join(lspDir, 'log'), 'utf8');
  } catch {
    return '';
  }
};

const session: { id: string; name: string } = await api('/api/sessions', 'POST', { name: `lsp-${Date.now()}` });
const browser = await chromium.launch({ headless: true, channel: process.env.BTMUX_TEST_BROWSER });

const lines = (page: Page, paneId: string) =>
  page.evaluate((id) => {
    const content = document.querySelector(`[data-native-editor][data-pane-id="${id}"]`);
    // The text, without inlay hints.
    const text = (line: Element) => {
      const copy = line.cloneNode(true) as Element;
      for (const hint of copy.querySelectorAll('.cm-lsp-inlay-hint')) hint.remove();
      return copy.textContent;
    };
    return content ? Array.from(content.querySelectorAll(':scope > .cm-line'), text).join('\n') : null;
  }, paneId);

/** The center of `word`'s first occurrence on `line` (0-based). */
const wordRect = (page: Page, paneId: string, line: number, word: string) =>
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
      throw new Error(`${word} not on line ${line}`);
    },
    { id: paneId, line, word },
  );

const until = async (what: string, check: () => boolean | Promise<boolean>) => {
  for (let i = 0; i < 200; i++) {
    if (await check()) return;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`timed out waiting for ${what}`);
};

try {
  const page = await (await browser.newContext({ viewport: { width: 1200, height: 800 } })).newPage();
  page.on('pageerror', (error) => console.error('page error:', error.message));
  await page.goto(url);
  await page.getByLabel('Access token').fill(token);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await page.waitForFunction(() => !document.querySelector('#access-token'));
  await page.goto(`${url}/s/${encodeURIComponent(session.name)}`);
  const paneId: string = await waitForAsync(
    page,
    async (id) => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      const s = useStore.getState();
      return s.controlConnectionState === 'connected' && s.getActivePaneId(id);
    },
    session.id,
  );
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: main, line: 1, editor: 'native' });
  const pane = page.locator(`[data-editor-pane="${paneId}"]`);
  await pane.getByLabel('Language server fake').waitFor();
  await until('the file', async () => (await lines(page, paneId)) === 'let thing = bad;\nuse thing;\nworse\nend\n');

  // Pushed ("bad") and pulled ("worse") diagnostics together.
  await pane.getByText('2 errors, 0 warnings').waitFor();
  console.log('PASS pushed and pulled diagnostics');

  // Semantic tokens color "thing" as a function, over the parser's colors.
  await pane.locator('.cm-sem-function').first().waitFor();
  const colors = await page.evaluate(async () => {
    const { useStore } = await import('/src/state/store.tsx' as string);
    const el = document.querySelector('.cm-sem-function')!;
    const probe = document.createElement('span');
    probe.style.color = useStore.getState().config.theme.blue;
    document.body.append(probe);
    const blue = getComputedStyle(probe).color;
    probe.remove();
    return { text: getComputedStyle(el).color, blue };
  });
  assert.equal(colors.text, colors.blue);
  console.log('PASS semantic tokens');

  await pane.locator('.cm-lsp-inlay-hint', { hasText: ': hint' }).waitFor();
  console.log('PASS inlay hints');

  // Folding ranges from the server (no parser for .fake).
  await pane.locator('.cm-foldGutter [title="Fold line"]').first().waitFor();
  console.log('PASS folding ranges');

  // Document highlights for the word under the cursor.
  const thing = await wordRect(page, paneId, 1, 'thing');
  await page.mouse.click(thing.x, thing.y);
  await until('highlights', async () => (await pane.locator('.cm-lsp-highlight').count()) === 2);
  console.log('PASS document highlights');

  // Mod-i grows the selection to the word.
  await page.keyboard.press('ControlOrMeta+i');
  await until('the word selected', () => page.evaluate(() => getSelection()?.toString() === 'thing'));
  console.log('PASS selection ranges');

  // Code actions apply through the server's edit.
  // Inside "bad" at the end of line 1, by keys: inlay hints move text as they arrive.
  await page.keyboard.press('ControlOrMeta+Home');
  await page.keyboard.press('End');
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('ControlOrMeta+.');
  await pane.getByRole('option', { name: /Replace bad with good/ }).waitFor();
  await page.keyboard.press('Enter');
  await until('the fix', async () => (await lines(page, paneId))?.startsWith('let thing = good;') ?? false);
  await pane.getByText('1 error, 0 warnings').waitFor();
  console.log('PASS code actions');

  // Changes that need confirmation ask first, here and from the server.
  const last = async () => (await lines(page, paneId))?.split('\n').at(-2);
  await page.keyboard.press('ControlOrMeta+.');
  await pane.getByRole('option', { name: /^Shout/ }).click();
  await page.getByText('Shout includes changes that need confirmation: Uppercase end').waitFor();
  await page.getByText('Skip these changes', { exact: true }).click();
  await until('the change not needing confirmation', async () => (await last()) === 'end!');
  await page.keyboard.press('ControlOrMeta+.');
  await pane.getByRole('option', { name: /Shout later/ }).click();
  await page.getByText('fake: Shout includes changes that need confirmation: Uppercase end').waitFor();
  assert.equal(await page.getByText('Skip these changes', { exact: true }).count(), 0);
  await page.getByText('Apply all changes', { exact: true }).click();
  await until('the server edit', async () => (await last()) === 'END!');
  await until('its answer', () => log().includes('answer workspace/applyEdit {"applied":true}'));
  console.log('PASS change annotations needing confirmation');

  // Document symbols jump within the file.
  await page.keyboard.press('ControlOrMeta+Shift+o');
  await pane.getByRole('option').nth(1).waitFor();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await until('the second thing', () =>
    page.evaluate(() => document.querySelector('.cm-activeLine')?.textContent === 'use thing;'),
  );
  console.log('PASS document symbols');

  // Incoming calls list callers.
  await page.keyboard.press('Shift+Alt+h');
  await pane.getByRole('listbox', { name: 'Calls to thing' }).waitFor();
  await pane.getByRole('option', { name: /caller@1/ }).waitFor();
  await page.keyboard.press('Escape');
  console.log('PASS call hierarchy');

  // A code lens runs a command; the server's question comes up in the focused tab.
  await pane.locator('.cm-lsp-code-lens button', { hasText: 'Ask me' }).click();
  await page.getByText('Yes', { exact: true }).click();
  await until('the answer', () => log().includes('answer window/showMessageRequest {"title":"Yes"}'));
  console.log('PASS code lenses and server prompts');

  // Watched files reach the server.
  writeFileSync(join(dir, 'new.fake'), 'x\n');
  await until('the watched file', () => /watched .*new\.fake/.test(log()));
  console.log('PASS watched files');

  // Workspace symbols open the other file in this pane.
  await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
  await page.keyboard.press('ControlOrMeta+Alt+o');
  await page.keyboard.type('thi');
  await pane.getByRole('option', { name: /other\.fake/ }).click();
  await until('other.fake', async () => (await lines(page, paneId)) === 'thing defined here\n');
  console.log('PASS workspace symbols');

  // Files opened before and after the server is ready both have its language.
  assert.match(log(), new RegExp(`^open ${main} fake$`, 'm'));
  assert.match(log(), new RegExp(`^open ${join(dir, 'other.fake')} fake$`, 'm'));
  console.log('PASS language ids');

  // Server work shows as a progress toast, finished a moment after it ends.
  await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.type('busy');
  const toast = pane.locator('[data-lsp-progress]', { hasText: 'Indexing' });
  await toast.getByText('1/2').waitFor();
  await toast.getByRole('progressbar').waitFor();
  assert.equal(await toast.getByText('fake').count(), 1);
  // In a pane narrower than the card, it shrinks to fit.
  const splits: string[] = [];
  for (let i = 0; i < 2; i++)
    splits.push((await api(`/api/panes/${paneId}/split`, 'POST', { direction: 'right', select: false })).pane_id);
  // The card and its status indicator, the first labeled element.
  const inside = () =>
    page.evaluate((id) => {
      const p = document.querySelector(`[data-editor-pane="${id}"]`)!.getBoundingClientRect();
      const card = document.querySelector(`[data-editor-pane="${id}"] [data-lsp-progress] > *`);
      const status = card?.querySelector('[aria-label]');
      return (
        p.width < 400 &&
        !!card &&
        !!status &&
        [card, status].every((el) => {
          const r = el.getBoundingClientRect();
          return r.left >= p.left && r.right <= p.right;
        })
      );
    }, paneId);
  await until('the card inside a narrow pane', inside);
  for (const id of splits) await api(`/api/panes/${id}`, 'DELETE');
  await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
  await page.keyboard.press('ControlOrMeta+End');
  for (let i = 0; i < 4; i++) await page.keyboard.press('Backspace');
  await toast.getByLabel('success').waitFor();
  await toast.waitFor({ state: 'detached' });
  console.log('PASS progress toasts');

  // The command palette runs them too.
  await page.keyboard.press('Control+a');
  await page.keyboard.press(':');
  await page.keyboard.type('editor document symbols');
  await page.keyboard.press('Enter');
  await pane.getByRole('listbox', { name: 'Go to symbol in file' }).waitFor();
  await page.keyboard.press('Escape');
  console.log('PASS palette commands');

  // MCP: what the editor's language servers report, for agents.
  const mcpHeaders = { ...headers, Accept: 'application/json, text/event-stream' };
  const rpc = async (body: object, session?: string) => {
    // Vite doesn't proxy /mcp.
    const response = await fetch(`${process.env.BTMUX_TEST_BACKEND_URL ?? url}/mcp`, {
      method: 'POST',
      headers: session ? { ...mcpHeaders, 'mcp-session-id': session } : mcpHeaders,
      body: JSON.stringify({ jsonrpc: '2.0', ...body }),
    });
    assert(response.ok, `mcp: ${response.status}`);
    const text = await response.text();
    const data = text.split('\n').find((line) => line.startsWith('data:') && line.length > 6);
    return { session: response.headers.get('mcp-session-id'), body: data ? JSON.parse(data.slice(5)) : null };
  };
  const init = await rpc({
    id: 1,
    method: 'initialize',
    params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'test', version: '0' } },
  });
  const mcpSession = init.session!;
  await rpc({ method: 'notifications/initialized' }, mcpSession);
  const call = async (name: string, args: object) => {
    const { body } = await rpc({ id: 2, method: 'tools/call', params: { name, arguments: args } }, mcpSession);
    return JSON.parse(body.result.content[0].text);
  };
  const found = await call('editor_diagnostics', { path: dir });
  assert(
    found.some((d: any) => d.path === main && d.line === 3 && d.message === 'worse word' && d.severity === 'error'),
    JSON.stringify(found),
  );
  const buffers = await call('editor_buffers', {});
  assert(
    buffers.some((b: any) => b.path === main && b.language === 'fake'),
    JSON.stringify(buffers),
  );
  console.log('PASS MCP editor tools');

  // A restarted server's watches work too.
  const count = (re: RegExp) => (log().match(re) ?? []).length;
  writeFileSync(join(dir, 'crash.fake'), 'x\n');
  await until('the restart', () => count(/^answer client\/registerCapability/gm) === 2);
  writeFileSync(join(dir, 'after.fake'), 'x\n');
  await until('the watched file after the restart', () => /watched .*after\.fake/.test(log()));
  console.log('PASS watched files after a restart');
} catch (error) {
  const shot = process.env.TEST_SHOT;
  if (shot) for (const context of browser.contexts()) await context.pages()[0]?.screenshot({ path: shot });
  throw error;
} finally {
  await browser.close();
  await api(`/api/sessions/${session.id}`, 'DELETE').catch(() => {});
  rmSync(dir, { recursive: true, force: true });
}
