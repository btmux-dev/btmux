/** Language server features of the native editor, with test-bin/fake-lsp. Run through browser-test-stack.ts. */
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
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
  '[language-server.fake]\ncommand = "fake-lsp"\n[[language]]\nname = "fake"\nfile-types = ["fake"]\nlanguage-servers = ["fake"]\n' +
    '[[language]]\nname = "rust"\nfile-types = ["rs"]\nlanguage-servers = ["fake"]\n' +
    // Two servers for one file, started only by the last test.
    '[language-server.first]\ncommand = "fake-lsp"\nargs = ["--no-format", "--slow-publish"]\n' +
    '[language-server.second]\ncommand = "fake-lsp"\nargs = ["--second"]\n' +
    '[[language]]\nname = "duo"\nfile-types = ["duo"]\nlanguage-servers = ["first", "second"]\n' +
    '[language-server.late]\ncommand = "fake-lsp"\nargs = ["--second", "--slow-init"]\n' +
    '[[language]]\nname = "late"\nfile-types = ["late"]\nlanguage-servers = ["late"]\n' +
    // A server for one file, for the problems list.
    '[language-server.solo]\ncommand = "fake-lsp"\nargs = ["--second"]\n' +
    '[[language]]\nname = "solo"\nfile-types = ["solo"]\nlanguage-servers = ["solo"]\n',
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
  // Enter takes the safe choice, listed first.
  await page.keyboard.press('Enter');
  await until('the change not needing confirmation', async () => (await last()) === 'end!');
  const shoutLater = async () => {
    await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
    await page.keyboard.press('ControlOrMeta+.');
    await pane.getByRole('option', { name: /Shout later/ }).click();
    await page.getByText('fake: Shout includes changes that need confirmation: Uppercase end').waitFor();
  };
  await shoutLater();
  assert.equal(await page.getByText('Skip these changes', { exact: true }).count(), 0);
  await page.keyboard.press('Enter');
  await until('the declined edit', () => log().includes('answer workspace/applyEdit {"applied":false'));
  assert.equal(await last(), 'end!');
  await shoutLater();
  await page.getByText('Apply all changes', { exact: true }).click();
  await until('the server edit', async () => (await last()) === 'END!');
  await until('its answer', () => log().includes('answer workspace/applyEdit {"applied":true}'));
  console.log('PASS change annotations needing confirmation');

  // The server says what renames, under which name, or why not.
  const use = await wordRect(page, paneId, 1, 'use');
  await page.mouse.click(use.x, use.y);
  await page.keyboard.press('F2');
  await page.getByText('use is a keyword').first().waitFor();
  const renaming = await wordRect(page, paneId, 1, 'thing');
  await page.mouse.click(renaming.x, renaming.y);
  await page.keyboard.press('F2');
  await until(
    'the name',
    async () =>
      (await page
        .getByLabel('New name')
        .inputValue()
        .catch(() => '')) === 'thing',
  );
  await page.keyboard.press('Escape');
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.press('F2');
  await page.getByText('Nothing to rename here').first().waitFor();
  assert.equal(await page.getByLabel('New name').count(), 0);
  console.log('PASS prepare rename');

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

  // A code lens runs a command; the server's question comes up in the focused
  // tab, and the chosen item goes back whole, even when titles repeat.
  await pane.locator('.cm-lsp-code-lens button', { hasText: 'Ask me' }).click();
  await page.getByText('Yes', { exact: true }).nth(1).click();
  await until('the answer', () => log().includes('answer window/showMessageRequest {"choice":2,"title":"Yes"}'));
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

  // Completions resolve: documentation for the selected one, an import once applied.
  const complete = join(dir, 'complete.fake');
  writeFileSync(complete, 'start\n');
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: complete, line: 2, editor: 'native' });
  await until('complete.fake', async () => (await lines(page, paneId)) === 'start\n');
  await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.type('imp');
  const completions = page.locator('.cm-tooltip-autocomplete');
  // Enter right after the list opens is ignored (autocomplete's interaction delay).
  const acceptCompletion = async () => {
    await page.waitForTimeout(150);
    await page.keyboard.press('Enter');
  };
  await completions.locator('.cm-completionLabel', { hasText: 'importedThing' }).waitFor();
  await completions.getByText('lib', { exact: true }).waitFor();
  await page.locator('.cm-completionInfo').getByText('importedThing', { exact: true }).waitFor();
  await page.locator('.cm-completionInfo').getByText('fn importedThing()').waitFor();
  await acceptCompletion();
  await until('the import', async () => (await lines(page, paneId)) === 'use lib;\nstart\nimportedThing');
  assert.match(log(), /^resolve importedThing \{"from":"defaults"\}$/m);
  console.log('PASS completion resolve');

  // An import resolved after switching files still lands in the file completed in.
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.press('Enter');
  await page.keyboard.type('slowI');
  await completions.locator('.cm-completionLabel', { hasText: 'slowImport' }).waitFor();
  await acceptCompletion();
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: join(dir, 'other.fake'), line: 1, editor: 'native' });
  await until('other.fake', async () => (await lines(page, paneId))?.startsWith('thing defined here') ?? false);
  await until('the late resolve', () => log().includes('resolve slowImport'));
  await page.waitForTimeout(1000);
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: complete, line: 1, editor: 'native' });
  await until(
    'the late import',
    async () => (await lines(page, paneId)) === 'use slow;\nuse lib;\nstart\nimportedThing\nslowImport',
  );
  console.log('PASS completion resolve after switching files');

  // Completions don't leave workspace mappings behind once they end.
  const activeMappings = () =>
    waitForAsync(
      page,
      async (id) => {
        // The app's own module instances, by the URLs Vite rewrote its imports to.
        const source = await (await fetch('/src/lib/nativeEditor/completion.ts')).text();
        const url = (name: string) => source.match(new RegExp(`from "([^"]*${name}[^"]*)"`))![1];
        const { EditorView } = await import(url('@codemirror_view'));
        const { LSPPlugin } = await import(url('@codemirror_lsp-client'));
        const content = document.querySelector(`[data-native-editor][data-pane-id="${id}"]`)!;
        const view = EditorView.findFromDOM(content.closest('.cm-editor') as HTMLElement);
        return String(LSPPlugin.get(view)?.client.activeMappings.length ?? 'none');
      },
      paneId,
    );
  for (let i = 0; i < 3; i++) {
    await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
    await page.keyboard.press('ControlOrMeta+End');
    await page.keyboard.type(' fa');
    await completions.locator('.cm-completionLabel', { hasText: 'fakeCompletion' }).waitFor();
    await page.keyboard.press('Escape');
  }
  assert.equal(await activeMappings(), '0');
  console.log('PASS completion mappings are released');

  // An additional edit right beside the completion (a postfix one's receiver) applies.
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.press('Enter');
  await page.keyboard.type('foo.i');
  await completions.locator('.cm-completionLabel', { hasText: /^if$/ }).waitFor();
  await acceptCompletion();
  await until('the postfix completion', async () => (await lines(page, paneId))?.endsWith('\nif foo {}') ?? false);
  console.log('PASS postfix completions');

  // A signature is highlighted in the buffer's language; Rust's `fn(…)` too.
  const rust = join(dir, 'detail.rs');
  writeFileSync(rust, '\n');
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: rust, line: 1, editor: 'native' });
  await until('detail.rs', async () => (await lines(page, paneId)) === '\n');
  await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
  // Two options, so the signature's row is selected in one state and not the other.
  await page.keyboard.type('fa');
  const signature = completions.locator('.cm-completionDetail', { hasText: 'fn(&str) -> Response<Body>' });
  await signature.waitFor();
  await completions.locator('.cm-completionLabel', { hasText: 'fakeField' }).waitFor();
  const detailColors = () =>
    signature.evaluate((el) => ({
      selected: el.closest('li')!.getAttribute('aria-selected') === 'true',
      row: getComputedStyle(el.closest('li')!).color,
      tokens: [...el.childNodes].map((n) => [n.textContent, n instanceof Element ? getComputedStyle(n).color : null]),
    }));
  const states = [await detailColors()];
  await page.keyboard.press('ArrowDown');
  await until('the selection moved', async () => (await detailColors()).selected !== states[0].selected);
  states.push(await detailColors());
  const unselected = states.find((state) => !state.selected)!;
  assert.equal(unselected.tokens.map(([text]) => text).join(''), 'fn(&str) -> Response<Body>');
  const color = (text: string) => unselected.tokens.find(([t]) => t === text)?.[1];
  assert(color('fn') && color('Response') && color('fn') !== color('Response'), JSON.stringify(unselected));
  // The selected row's own color, as token colors aren't checked against its background.
  const selected = states.find((state) => state.selected)!;
  for (const [text, tokenColor] of selected.tokens)
    if (tokenColor) assert.equal(tokenColor, selected.row, `${text}: ${JSON.stringify(selected)}`);
  assert.equal(await signature.evaluate((el) => getComputedStyle(el).fontStyle), 'normal');
  console.log('PASS highlighted completion details');

  // Ctrl-N and Ctrl-P move through completions, wrapping, as in vim; Ctrl-P
  // isn't quick open (Mod-p off macOS) while they're open.
  const selectedLabel = () => completions.locator('li[aria-selected] .cm-completionLabel').textContent();
  const first = await selectedLabel();
  await page.keyboard.press('Control+n');
  await until('Ctrl-N moved', async () => (await selectedLabel()) !== first);
  await page.keyboard.press('Control+n');
  await until('Ctrl-N wrapped', async () => (await selectedLabel()) === first);
  await page.keyboard.press('Control+p');
  await until('Ctrl-P wrapped', async () => (await selectedLabel()) !== first);
  assert.equal(await pane.getByRole('combobox', { name: 'Go to file' }).count(), 0);
  // Ctrl-J and Ctrl-K too, as in fzf; Ctrl-K doesn't start a Mod-k chord off macOS.
  await page.keyboard.press('Control+j');
  await until('Ctrl-J moved', async () => (await selectedLabel()) === first);
  await page.keyboard.press('Control+k');
  await until('Ctrl-K moved', async () => (await selectedLabel()) !== first);
  await page.keyboard.press('Control+j');
  await until('Ctrl-J after Ctrl-K', async () => (await selectedLabel()) === first);
  // Each kind has its own Nerd Font icon.
  const fieldIcon = completions.locator('li', { hasText: 'fakeField' }).locator('.cm-completionIcon-field');
  assert.equal(await fieldIcon.evaluate((el) => getComputedStyle(el, '::after').content), JSON.stringify('\ueb5f'));
  await page.keyboard.press('Escape');
  console.log('PASS Ctrl-N/P and Ctrl-J/K move through completions');

  // They move as soon as a list shows (before autocomplete's interaction
  // delay) and with Caps Lock; a list waiting for refreshed results keeps
  // them, so Ctrl-P neither opens quick open nor moves the cursor.
  const incomplete = join(dir, 'incomplete.fake');
  writeFileSync(incomplete, '\n');
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: incomplete, line: 1, editor: 'native' });
  await until('incomplete.fake', async () => (await lines(page, paneId)) === '\n');
  await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
  // Sent from the page as the list appears, well inside the 75 ms delay.
  const pressedOnOpen = page.evaluate(
    (id) =>
      new Promise<void>((resolve) => {
        const content = document.querySelector(`[data-native-editor][data-pane-id="${id}"]`)!;
        const editor = content.closest('.cm-editor')!;
        const observer = new MutationObserver(() => {
          if (!editor.querySelector('.cm-tooltip-autocomplete li')) return;
          observer.disconnect();
          const init = { key: 'n', code: 'KeyN', ctrlKey: true, bubbles: true, cancelable: true };
          content.dispatchEvent(new KeyboardEvent('keydown', init));
          resolve();
        });
        observer.observe(editor, { childList: true, subtree: true });
      }),
    paneId,
  );
  await page.keyboard.type('i');
  await pressedOnOpen;
  const selectedIndex = () =>
    completions.locator('li').evaluateAll((items) => items.findIndex((li) => li.hasAttribute('aria-selected')));
  assert.equal(await selectedIndex(), 1);
  // Uppercase without Shift, as Caps Lock sends.
  await page.keyboard.press('Control+K');
  await until('Ctrl-K moved with Caps Lock', async () => (await selectedIndex()) === 0);
  await page.keyboard.type('nco');
  // Only "inco" is answered late; earlier queries disable the list briefly too.
  await until('inco', async () => (await lines(page, paneId)) === '\ninco');
  await page.locator('.cm-tooltip-autocomplete-disabled').waitFor();
  await page.keyboard.press('Control+p');
  await page.locator('.cm-tooltip-autocomplete:not(.cm-tooltip-autocomplete-disabled)').waitFor();
  await completions.locator('.cm-completionLabel', { hasText: 'incOne' }).waitFor();
  assert.equal(await pane.getByRole('combobox', { name: 'Go to file' }).count(), 0);
  assert.equal(await lines(page, paneId), '\ninco');
  console.log('PASS completion keys during the interaction delay, with Caps Lock, and while refreshing');

  // The text kind's icon is as large as the others (CodeMirror halves its `abc`).
  await page.keyboard.press('Escape');
  await page.keyboard.press('Enter');
  await page.keyboard.type('i');
  await completions.locator('.cm-completionLabel', { hasText: 'incText' }).waitFor();
  const iconSize = (label: string) =>
    completions
      .locator('li', { hasText: label })
      .locator('.cm-completionIcon')
      .evaluate((el) => getComputedStyle(el, '::after').fontSize);
  assert.equal(await iconSize('incText'), await iconSize('incOne'));
  await page.keyboard.press('Escape');
  console.log('PASS text completion icons are full size');

  // File operations: a file created and edited, the shown one moved with its tab.
  const moving = join(dir, 'move.fake');
  const moved = join(dir, 'mod', 'moved.fake');
  const extra = join(dir, 'mod', 'extra.fake');
  writeFileSync(moving, 'move me\n');
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: moving, line: 1, editor: 'native' });
  await until('move.fake', async () => (await lines(page, paneId)) === 'move me\n');
  await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
  await page.keyboard.press('ControlOrMeta+.');
  await pane.getByRole('option', { name: /Move into a module/ }).click();
  await pane.getByRole('tab', { name: 'moved.fake' }).waitFor();
  await until('the moved file', async () => (await lines(page, paneId)) === 'move me\n');
  assert.equal(await pane.getByRole('tab', { name: 'move.fake' }).count(), 0);
  assert(!existsSync(moving) && existsSync(moved) && existsSync(extra));
  await until('the moved file in the server', () => log().includes(`open ${moved} fake`));
  assert.match(log(), /^workspace\/didCreateFiles .*extra\.fake/m);
  assert.match(log(), /^workspace\/didRenameFiles .*"oldUri":"[^"]*\/move\.fake"/m);
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: extra, line: 1, editor: 'native' });
  await until('the created file, unsaved', async () => (await lines(page, paneId)) === 'created\n');
  console.log('PASS file operations');

  // The file browser's renames update references first, and open files follow.
  const files = (type: string, payload: object) =>
    page.evaluate(
      async ({ type, payload }) => {
        const ws = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws/files`);
        await new Promise((resolve, reject) => {
          ws.onopen = resolve;
          ws.onerror = reject;
        });
        const reply = await new Promise<{ type: string }>((resolve) => {
          ws.onmessage = (e) => {
            const msg = JSON.parse(e.data);
            if (msg.id === 'test') resolve(msg);
          };
          ws.send(JSON.stringify({ id: 'test', type, payload }));
        });
        ws.close();
        return reply;
      },
      { type, payload },
    );
  const lib = join(dir, 'lib.fake');
  const util = join(dir, 'util.fake');
  const user = join(dir, 'user.fake');
  writeFileSync(lib, 'helper\n');
  writeFileSync(user, 'use lib.fake;\n');
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: lib, line: 1, editor: 'native' });
  await until('lib.fake', async () => (await lines(page, paneId)) === 'helper\n');
  assert.equal((await files('rename_file', { from: lib, to: util })).type, 'rename_file_result');
  await pane.getByRole('tab', { name: 'util.fake' }).waitFor();
  assert.equal(await pane.getByRole('tab', { name: 'lib.fake' }).count(), 0);
  await until('the moved file', async () => (await lines(page, paneId)) === 'helper\n');
  await until('didRenameFiles', () => /^workspace\/didRenameFiles .*util\.fake/m.test(log()));
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: user, line: 1, editor: 'native' });
  await until('the updated reference', async () => (await lines(page, paneId)) === 'use util.fake;\n');
  assert.equal(readFileSync(user, 'utf8'), 'use lib.fake;\n');
  // Deleting a file closes its tab.
  assert.equal((await files('delete_file', { root: dir, path: util })).type, 'delete_file_result');
  await until('the closed tab', async () => (await pane.getByRole('tab', { name: 'util.fake' }).count()) === 0);
  await until('didDeleteFiles', () => /^workspace\/didDeleteFiles .*util\.fake/m.test(log()));
  // Creating, copying, and moving too, under free names. A create that
  // would fail doesn't ask servers for edits first.
  const willCreates = () => (log().match(/^workspace\/willCreateFiles$/gm) ?? []).length;
  assert.equal((await files('create_entry', { root: dir, name: 'user.fake' })).type, 'error');
  assert.equal(willCreates(), 0);
  assert.equal((await files('create_entry', { root: dir, name: 'made.fake' })).type, 'create_entry_result');
  await until('didCreateFiles', () => /^workspace\/didCreateFiles .*made\.fake/m.test(log()));
  assert.equal(willCreates(), 1);
  await files('copy_entries', { paths: [user], dest: join(dir, 'mod') });
  await until('the copy', () => /^workspace\/didCreateFiles .*mod\/user\.fake/m.test(log()));
  await files('move_entries', { paths: [join(dir, 'mod', 'user.fake'), join(dir, 'made.fake')], dest: dir });
  await until('the move', () =>
    /^workspace\/didRenameFiles .*"newUri":"[^"]*\/user_copy\.fake","oldUri":"[^"]*\/mod\/user\.fake"/m.test(log()),
  );
  assert(existsSync(join(dir, 'user_copy.fake')) && existsSync(join(dir, 'made_copy.fake')));
  console.log('PASS file browser operations');

  // Diagnostic tags fade unused code and strike deprecated code; codes link to their docs.
  const tags = join(dir, 'tags.fake');
  writeFileSync(tags, 'let unused = old;\n');
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: tags, line: 1, editor: 'native' });
  const unused = pane.locator('.cm-lsp-unnecessary', { hasText: 'unused' });
  await unused.waitFor();
  const faded = await unused.evaluate((el) => [getComputedStyle(el).opacity, getComputedStyle(el).backgroundImage]);
  assert.deepEqual(faded, ['0.55', 'none']);
  const old = pane.locator('.cm-lsp-deprecated', { hasText: 'old' });
  assert.match(await old.evaluate((el) => getComputedStyle(el).textDecorationLine), /line-through/);
  // Hints and lenses move text as they arrive; hover the mark itself.
  await old.hover();
  const code = page.locator('.cm-tooltip a.cm-lsp-diagnostic-code', { hasText: '(D1)' });
  await code.waitFor();
  assert.equal(await code.getAttribute('href'), 'https://example.com/D1');
  console.log('PASS diagnostic tags and codes');

  // Related information links to its location.
  await page.locator('.cm-tooltip .cm-lsp-related a', { hasText: 'other.fake:1' }).click();
  await until('the related location', async () => (await lines(page, paneId)) === 'thing defined here\n');
  console.log('PASS related information');

  // Implementations, with Mod-F12, and the palette's declaration and type definition commands.
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: tags, line: 1, editor: 'native' });
  await until('tags.fake', async () => (await lines(page, paneId)) === 'let unused = old;\n');
  await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
  await page.keyboard.press('ControlOrMeta+F12');
  await until('the implementation', async () => (await lines(page, paneId)) === 'thing defined here\n');
  console.log('PASS go to implementation');

  // Edits re-pull other files' diagnostics when they depend on each other.
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: tags, line: 1, editor: 'native' });
  await until('tags.fake', async () => (await lines(page, paneId)) === 'let unused = old;\n');
  const pulls = () =>
    log()
      .split('\n')
      .filter((l) => l === `pull ${main}`).length;
  const before = pulls();
  await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.type('withdraw');
  await until('main.fake pulled again', () => pulls() > before);
  console.log('PASS dependent diagnostics');

  // A question the server withdraws goes away.
  await page.keyboard.press('ControlOrMeta+.');
  await pane.getByRole('option', { name: /Ask, then withdraw/ }).click();
  await page.getByText('Withdrawn soon?').first().waitFor();
  await until('the withdrawn question', async () => (await page.getByText('Withdrawn soon?').count()) === 0);
  await until('its cancellation', () => log().includes('answer window/showMessageRequest undefined'));
  console.log('PASS withdrawn prompts');

  // Declining a code action's edit skips its command too.
  const runs = () =>
    log()
      .split('\n')
      .filter((l) => l === 'execute fake.hint ["after"]').length;
  await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
  await page.keyboard.press('ControlOrMeta+.');
  await pane.getByRole('option', { name: /Confirm, then run/ }).click();
  await page.getByText('Confirm, then run includes changes that need confirmation: Comment out').waitFor();
  await page.keyboard.press('Escape');
  await until('the dismissed confirmation', async () => (await page.getByText('Apply all changes').count()) === 0);
  await page.waitForTimeout(500);
  assert.equal(runs(), 0);
  await page.keyboard.press('ControlOrMeta+.');
  await pane.getByRole('option', { name: /Confirm, then run/ }).click();
  await page.getByText('Apply all changes', { exact: true }).click();
  await until('the command after the edit', () => runs() === 1);
  assert((await lines(page, paneId))?.startsWith('// let unused'));
  console.log('PASS declined code actions skip their command');

  // Inlay hints resolve: a tooltip on hover, a command and a location in their
  // label, and edits on a double click.
  const hinted = join(dir, 'hints.fake');
  writeFileSync(hinted, 'let x = 1;\n');
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: hinted, line: 1, editor: 'native' });
  const hint = pane.locator('.cm-lsp-inlay-hint', { hasText: ': hint' });
  await hint.waitFor();
  await pane.locator('.cm-lsp-code-lens').first().waitFor();
  const [colon, type] = [
    hint.locator('.cm-lsp-inlay-hint-part').nth(0),
    hint.locator('.cm-lsp-inlay-hint-part').nth(1),
  ];
  await type.hover();
  await page.locator('.cm-tooltip').getByText('Type of').waitFor();
  assert.equal(await page.getByText('Fake hover for').count(), 0);
  await until('the resolved command', async () => (await colon.getAttribute('class'))?.includes('command') ?? false);
  await colon.click();
  await until('the command', () => log().includes('execute fake.hint ["x"]'));
  await hint.dblclick();
  await until('the inserted hint', async () => (await lines(page, paneId)) === 'let x: hint = 1;\n');
  await type.click({ modifiers: ['ControlOrMeta'] });
  await until('the location', async () => (await lines(page, paneId)) === 'thing defined here\n');
  console.log('PASS inlay hint resolve');

  // Inline completions: ghost text to type through, cycle, accept, or dismiss.
  const inline = join(dir, 'inline.fake');
  writeFileSync(inline, 'x\n');
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: inline, line: 1, editor: 'native' });
  await until('inline.fake', async () => (await lines(page, paneId)) === 'x\n');
  const ghost = pane.locator('.cm-inline-completion');
  const ghostText = async () => ((await ghost.count()) ? await ghost.textContent() : null);
  await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.type('fn ');
  await until('the ghost text', async () => (await ghostText()) === 'main() {}');
  assert.match(log(), /^inline 2$/m);
  // As macOS sends Option chords: the text Option types, and the physical key.
  const optionKey = (code: string, key: string) =>
    page.evaluate(
      ({ code, key }) =>
        document.activeElement?.dispatchEvent(
          new KeyboardEvent('keydown', { key, code, altKey: true, bubbles: true, cancelable: true }),
        ),
      { code, key },
    );
  await optionKey('BracketRight', '‘');
  await until('the next suggestion', async () => (await ghostText()) === 'other() {}');
  await page.keyboard.press('Alt+[');
  await until('the first again', async () => (await ghostText()) === 'main() {}');
  await page.keyboard.type('m');
  await until('typed through', async () => (await ghostText()) === 'ain() {}');
  await page.keyboard.press('ControlOrMeta+ArrowRight');
  await until('a word accepted', async () => (await ghostText()) === '() {}');
  await page.keyboard.press('Tab');
  await until('accepted', async () => (await lines(page, paneId)) === 'x\nfn main() {}');
  await until('its command', () => log().includes('execute fake.hint ["inline"]'));
  await page.keyboard.press('Enter');
  await page.keyboard.type('fn ');
  await until('the ghost text again', async () => (await ghostText()) === 'main() {}');
  await page.keyboard.press('Escape');
  await until('dismissed', async () => (await ghostText()) === null);
  await optionKey('Backslash', '«');
  await until('asked for', async () => (await ghostText()) === 'main() {}');
  assert.match(log(), /^inline 1$/m);
  console.log('PASS inline completions');

  // Esc drops a suggestion still on its way, too.
  await page.keyboard.press('Escape');
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.press('Enter');
  await page.keyboard.type('lazy ');
  await optionKey('Backslash', '«');
  await page.waitForTimeout(200);
  await page.keyboard.press('Escape');
  await page.waitForTimeout(1500);
  assert.equal(await ghostText(), null);
  assert.match(log(), /^\$\/cancelRequest$/m);
  console.log('PASS dismissed inline completions stay dismissed');

  // A restarted server's watches work too.
  const count = (re: RegExp) => (log().match(re) ?? []).length;
  writeFileSync(join(dir, 'crash.fake'), 'x\n');
  await until('the restart', () => count(/^answer client\/registerCapability/gm) === 2);
  writeFileSync(join(dir, 'after.fake'), 'x\n');
  await until('the watched file after the restart', () => /watched .*after\.fake/.test(log()));
  console.log('PASS watched files after a restart');

  // Two servers for one file: each one's diagnostics, hovers, and code
  // actions, and formatting from the only one that can.
  const duo = join(dir, 'main.duo');
  writeFileSync(duo, 'bad lint fmt\n');
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: duo, line: 1, editor: 'native' });
  await pane.locator('[aria-label^="Language server first"][data-lsp-state="ready"]').waitFor();
  await pane.locator('[aria-label^="Language server second"][data-lsp-state="ready"]').waitFor();
  await until('the file', async () => (await lines(page, paneId)) === 'bad lint fmt\n');
  await pane.getByText('1 error, 1 warning').waitFor();
  console.log('PASS diagnostics from two servers');

  // Lenses move text as they arrive; hover the mark itself.
  await pane.locator('.cm-lsp-code-lens').first().waitFor();
  const lintMark = pane.locator('.cm-lintRange-warning').first();
  await lintMark.hover();
  const hover = page.locator('.cm-lsp-hover-tooltip');
  await hover.getByText('Fake hover for lint').waitFor();
  await hover.getByText('Second hover for lint').waitFor();
  console.log('PASS hovers from two servers');

  await lintMark.click();
  await page.keyboard.press('ControlOrMeta+.');
  const fix = pane.getByRole('option', { name: /Fix lint/ });
  await fix.waitFor();
  assert.match((await fix.textContent()) ?? '', /second/);
  await fix.click();
  await until('the fix', async () => (await lines(page, paneId)) === 'bad clean fmt\n');
  await pane.getByText('1 error, 0 warnings').waitFor();
  console.log('PASS code actions from the second server');

  await page.keyboard.press('Shift+Alt+f');
  await until('the format', async () => (await lines(page, paneId)) === 'bad clean FMT\n');
  console.log('PASS formatting from the server that can');

  // A line added above: the second server's warning on it comes after the
  // editor synced (1 s), the first server's update 2 s late. Meanwhile its
  // error stays on its text. Its late update for formatting comes first.
  await page.waitForTimeout(2500);
  await page.keyboard.press('ControlOrMeta+Home');
  await page.keyboard.type('lint\n');
  await until('the new warning', async () => (await pane.locator('.cm-lintRange-warning').count()) === 1);
  const error = pane.locator('.cm-lintRange-error');
  assert.equal(await error.textContent(), 'bad');
  await until('the first server', () => /version \d+ "lint\\nbad clean FMT\\n"/.test(log()));
  await page.waitForTimeout(2500);
  assert.equal(await error.textContent(), 'bad');
  console.log("PASS a server's diagnostics stay on their text while another publishes");

  // The problems list: every file's, most severe first, whatever the position.
  await page.keyboard.press('ControlOrMeta+Shift+M');
  const problems = pane.locator('.cm-btmux-problems');
  const duoRows = problems.locator('li[role=option]', { hasText: 'main.duo' });
  await until('both problems', async () => (await duoRows.count()) === 2);
  assert.match((await duoRows.nth(0).textContent()) ?? '', /^E.*main\.duo:2:1bad word/);
  assert.match((await duoRows.nth(1).textContent()) ?? '', /^W.*main\.duo:1:1second lint/);
  const severities = await problems
    .locator('li[role=option]')
    .evaluateAll((rows) => rows.map((r) => (r.className.includes('error') ? 1 : r.className.includes('warning') ? 2 : 3)));
  assert.deepEqual(severities, [...severities].sort((a, b) => a - b));
  const inList = () => page.waitForFunction(() => document.activeElement?.closest('.cm-btmux-problems'));
  const inText = () => page.waitForFunction(() => document.activeElement?.matches('.cm-content[data-native-editor]'));
  await inList();
  await duoRows.nth(0).click();
  await pane.getByText('Ln 2, Col 1', { exact: true }).waitFor();
  await inText();
  // It stays open after a jump; a right click doesn't jump.
  await duoRows.nth(1).click({ button: 'right' });
  await page.waitForTimeout(300);
  await pane.getByText('Ln 2, Col 1', { exact: true }).waitFor();

  // Edits move rows at once, before the slow server publishes again.
  await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).focus();
  const edited = Date.now();
  await page.keyboard.press('ControlOrMeta+Home');
  await page.keyboard.press('Enter');
  await duoRows.filter({ hasText: 'main.duo:3:1bad word' }).waitFor();
  assert(Date.now() - edited < 1500, 'the row waited for the server');
  await page.keyboard.press('ControlOrMeta+z');
  await until('the undo', async () => (await lines(page, paneId)) === 'lint\nbad clean FMT\n');

  // Mod-Shift-M closes it from the text and from the list.
  await page.keyboard.press('ControlOrMeta+Shift+M');
  await problems.waitFor({ state: 'detached' });
  await page.keyboard.press('ControlOrMeta+Shift+M');
  await inList();
  await page.keyboard.press('ControlOrMeta+Shift+M');
  await problems.waitFor({ state: 'detached' });
  await inText();

  // Every row is reachable, and closing a file in the background drops its server's.
  const many = join(dir, 'many.fake');
  const solo = join(dir, 'x.solo');
  writeFileSync(many, 'bad\n'.repeat(600));
  writeFileSync(solo, 'lint\n');
  for (const path of [many, solo])
    await api(`/api/panes/${paneId}/open-editor`, 'POST', { path, line: 1, editor: 'native' });
  await pane.locator('[aria-label^="Language server solo"][data-lsp-state="ready"]').waitFor();
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: duo, line: 1, editor: 'native' });
  await until('main.duo again', async () => (await lines(page, paneId)) === 'lint\nbad clean FMT\n');
  await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).focus();
  await page.keyboard.press('ControlOrMeta+Shift+M');
  await inList();
  const counts = async () => {
    const m = /(\d+) errors?, (\d+) warnings?/.exec((await problems.locator('.cm-btmux-problems-counts').textContent()) ?? '');
    return m ? { errors: Number(m[1]), warnings: Number(m[2]) } : { errors: 0, warnings: 0 };
  };
  await until('600 errors', async () => (await counts()).errors >= 601);
  assert((await problems.locator('li[role=option]').count()) < 100, 'rows not in view were drawn');
  const warnings = (await counts()).warnings;
  await page.keyboard.press('End');
  const selectedRow = problems.locator('li[aria-selected=true]');
  await until('the last row', async () => /^W/.test((await selectedRow.textContent()) ?? ''));
  await page.keyboard.press('Home');
  await until('the first row', async () => /^E/.test((await selectedRow.textContent()) ?? ''));
  // f lists only the shown file's, as Vim's location list, and back.
  const scope = (name: string) => problems.getByRole('button', { name });
  await page.keyboard.press('f');
  await until("the shown file's", async () => (await counts()).errors === 1);
  assert.equal(await scope('This file').getAttribute('aria-pressed'), 'true');
  assert.equal(await problems.locator('li[role=option]').count(), await duoRows.count());
  await page.keyboard.press('f');
  await until('every file again', async () => (await counts()).errors >= 601);
  assert.equal(await scope('Workspace').getAttribute('aria-pressed'), 'true');
  // A hidden file's rows are its server's latest.
  writeFileSync(solo, 'lint lint\n');
  await until("the hidden file's new warning", async () => (await counts()).warnings === warnings + 1);
  await pane.locator('[title="Close x.solo (Delete)"]').dispatchEvent('click');
  await until("the closed file's warning", async () => (await counts()).warnings === warnings - 1);
  // Its server serves other open files, so this one's stay.
  await pane.locator('[title="Close many.fake (Delete)"]').dispatchEvent('click');
  // The editor's chords that don't need the text work from the list.
  await inList();
  await page.keyboard.press('ControlOrMeta+p');
  const picker = pane.locator('.cm-btmux-picker');
  await picker.waitFor();
  await page.keyboard.press('Escape');
  await picker.waitFor({ state: 'detached' });
  await problems.locator('ul').focus();
  await page.keyboard.press('Escape');
  await problems.waitFor({ state: 'detached' });
  await inText();
  console.log('PASS the problems list');

  // One server's failed completion leaves the other's suggestions.
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.type(' fa');
  const suggestion = completions.locator('.cm-completionLabel', { hasText: 'fakeCompletion' });
  await suggestion.waitFor();
  await page.waitForTimeout(1000);
  assert(await suggestion.isVisible(), 'the failed request closed completion');
  await page.keyboard.press('Escape');
  console.log("PASS one server's failed completion keeps the other's");

  // A slow server doesn't hold back the other's hover: the first sends
  // nothing for 6 s on "stall".
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.press('Enter');
  await page.keyboard.type('stall');
  await page.keyboard.press('Escape');
  await page.mouse.move(0, 0);
  const stall = await wordRect(page, paneId, 3, 'stall');
  const stalledAt = Date.now();
  await page.mouse.move(stall.x, stall.y);
  await page.locator('.cm-lsp-hover-tooltip').getByText('Second hover for stall').waitFor({ timeout: 4000 });
  assert(Date.now() - stalledAt < 4000);
  await page.mouse.move(0, 0);
  console.log("PASS a slow server doesn't hold back another's hover");

  // A command run while the server starts waits for it.
  const late = join(dir, 'start.late');
  writeFileSync(late, 'fmt\n');
  await api(`/api/panes/${paneId}/open-editor`, 'POST', { path: late, line: 1, editor: 'native' });
  await until('the late file', async () => (await lines(page, paneId)) === 'fmt\n');
  await pane.locator('[aria-label^="Language server late"][data-lsp-state="starting"]').waitFor();
  await page.locator(`[data-native-editor][data-pane-id="${paneId}"]`).click();
  await page.keyboard.press('Shift+Alt+f');
  await until('the format', async () => (await lines(page, paneId)) === 'FMT\n');
  console.log('PASS a command run while the server starts waits for it');
} catch (error) {
  const shot = process.env.TEST_SHOT;
  if (shot) for (const context of browser.contexts()) await context.pages()[0]?.screenshot({ path: shot });
  throw error;
} finally {
  await browser.close();
  await api(`/api/sessions/${session.id}`, 'DELETE').catch(() => {});
  rmSync(dir, { recursive: true, force: true });
}
