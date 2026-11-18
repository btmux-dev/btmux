/** The native editor's key sequences: vim mappings with a leader, which-key, the jump list, and VS Code's chords. Run through browser-test-stack.ts. */
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

const dir = realpathSync(mkdtempSync(join(tmpdir(), 'btmux-keys-')));
const main = join(dir, 'main.fake');
const MAIN = 'let thing = bad;   \nuse thing;\nend\n';
writeFileSync(main, MAIN);
writeFileSync(join(dir, 'other.fake'), 'thing defined here\n');
const long = join(dir, 'long.txt');
writeFileSync(long, Array.from({ length: 300 }, (_, i) => `line ${i + 1}`).join('\n'));
const wrap = join(dir, 'wrap.txt');
writeFileSync(wrap, Array.from({ length: 30 }, () => 'word').join(' ') + '\n');
mkdirSync(lspDir, { recursive: true });
mkdirSync(join(configHome, 'btmux'), { recursive: true });
const config = join(configHome, 'btmux', 'config.toml');
const lsp = '[language-server.fake]\ncommand = "fake-lsp"\n[[language]]\nname = "fake"\nfile-types = ["fake"]\nlanguage-servers = ["fake"]\n';
writeFileSync(config, `[native-editor]\nvim = true\n${lsp}`);
const log = () => {
  try {
    return readFileSync(join(lspDir, 'log'), 'utf8');
  } catch {
    return '';
  }
};

const session: { id: string; name: string } = await api('/api/sessions', 'POST', { name: `keys-${Date.now()}` });
const browser = await chromium.launch({ headless: true, channel: process.env.BTMUX_TEST_BROWSER });

const lines = (page: Page, paneId: string) =>
  page.evaluate((id) => {
    const content = document.querySelector(`[data-native-editor][data-pane-id="${id}"]`);
    const text = (line: Element) => {
      const copy = line.cloneNode(true) as Element;
      for (const hint of copy.querySelectorAll('.cm-lsp-inlay-hint')) hint.remove();
      return copy.textContent;
    };
    return content ? Array.from(content.querySelectorAll(':scope > .cm-line'), text).join('\n') : null;
  }, paneId);

const until = async (what: string, check: () => boolean | Promise<boolean>) => {
  for (let i = 0; i < 200; i++) {
    if (await check()) return;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`timed out waiting for ${what}`);
};

/** Waits for the config to have vim on or off, and `keys` mapped if given. */
const configured = (page: Page, vim: boolean, keys?: string) =>
  waitForAsync(
    page,
    async ({ vim, keys }) => {
      const { useStore } = await import('/src/state/store.tsx' as string);
      const config = useStore.getState().config;
      return (
        config?.vim_mode === vim &&
        (!keys || config.vim_binds.some((b: { keys: string[] }) => b.keys.join('') === keys))
      );
    },
    { vim, keys },
  );

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
  await configured(page, true);
  const open = (path: string) => api(`/api/panes/${paneId}/open-editor`, 'POST', { path, line: 1, editor: 'native' });
  await open(main);
  const pane = page.locator(`[data-editor-pane="${paneId}"]`);
  const content = page.locator(`[data-native-editor][data-pane-id="${paneId}"]`);
  const mode = (label: string) => pane.locator('[aria-label="Vim mode"]').getByText(label, { exact: true }).waitFor();
  const at = (text: string) => pane.getByText(text, { exact: true }).waitFor();
  const popup = pane.getByRole('status', { name: /^Keys after/ });
  const row = (key: string) => popup.locator(`[data-which-key="${key}"]`);
  await pane.getByLabel('Language server fake').waitFor();
  await until('the file', async () => (await lines(page, paneId)) === MAIN);
  await content.click();
  await mode('NORMAL');

  // Space is the leader: which-key lists what follows, Esc drops it.
  await page.keyboard.type('gg');
  await page.keyboard.press(' ');
  await pane.getByLabel('Pending keys').getByText('␣').waitFor();
  await row('h').getByText('+hunk').waitFor();
  await row('f').getByText('Format').waitFor();
  await page.keyboard.press('Escape');
  await popup.waitFor({ state: 'detached' });
  await mode('NORMAL');
  assert.equal(await pane.getByLabel('Pending keys').count(), 0);
  console.log('PASS the leader shows which-key, and Esc drops it');

  // <leader>f formats the file; gqq formats a line through the server.
  await page.keyboard.type(' f');
  await until('formatted', async () => (await lines(page, paneId)) === 'let thing = bad;\nuse thing;\nend\n');
  await page.keyboard.type('A  ');
  await page.keyboard.press('Escape');
  await until('trailing spaces', async () => (await lines(page, paneId))!.startsWith('let thing = bad;  \n'));
  await page.keyboard.type('gqq');
  await until('range formatted', async () => (await lines(page, paneId))!.startsWith('let thing = bad;\n'));
  assert(log().includes('textDocument/rangeFormatting'));
  console.log('PASS <leader>f and gq format with the language server');

  // Neovim's gr* mappings and K.
  await page.keyboard.type('ggw');
  await at('Ln 1, Col 5');
  await page.keyboard.type('K');
  await page.locator('.cm-tooltip').getByText('Fake hover for thing').waitFor();
  await page.keyboard.type('grn');
  await until('the name', async () => (await pane.getByLabel('New name').inputValue().catch(() => '')) === 'thing');
  await page.keyboard.press('Escape');
  await content.click();
  await page.keyboard.type('ggw');
  await page.keyboard.type('grr');
  await pane.locator('.cm-lsp-reference-panel').waitFor();
  await page.keyboard.press('Escape');
  await content.click();
  await page.keyboard.type('gg$h');
  await page.keyboard.type('gra');
  await pane.getByRole('option', { name: /Replace bad with good/ }).waitFor();
  await page.keyboard.press('Escape');
  console.log('PASS K, grn, grr, and gra use the language server');

  // Keys held for a mapping that turn out not to be one reach vim: gg, viw.
  await content.click();
  await page.keyboard.type('Gggw');
  await at('Ln 1, Col 5');
  await page.keyboard.type('viwd');
  await until('the word deleted', async () => (await lines(page, paneId))!.startsWith('let  = bad;'));
  await page.keyboard.type('u');
  await until('undone', async () => (await lines(page, paneId))!.startsWith('let thing = bad;'));
  console.log('PASS held keys that are no mapping reach vim');

  // gd jumps to the other file; CTRL-O comes back, CTRL-I goes again.
  await page.keyboard.type('ggw');
  await page.keyboard.type('gd');
  await until('the definition', async () => (await lines(page, paneId)) === 'thing defined here\n');
  await page.keyboard.press('Control+o');
  await until('back', async () => (await lines(page, paneId))?.startsWith('let thing') ?? false);
  await at('Ln 1, Col 5');
  await page.keyboard.press('Control+i');
  await until('forward', async () => (await lines(page, paneId)) === 'thing defined here\n');
  // A move off the screen is a jump too.
  await open(long);
  await until('the long file', async () => (await lines(page, paneId))?.startsWith('line 1\n') ?? false);
  await content.click();
  await page.keyboard.type('gg');
  await at('Ln 1, Col 1');
  await page.keyboard.type('G');
  await at('Ln 300, Col 1');
  await page.keyboard.press('Control+o');
  await at('Ln 1, Col 1');
  console.log('PASS CTRL-O and CTRL-I follow jumps across files');

  // Without a server, gq hard wraps.
  await open(wrap);
  await until('the long line', async () => (await lines(page, paneId))?.startsWith('word word') ?? false);
  await content.click();
  await page.keyboard.type('gggqq');
  await until('wrapped', async () => (await lines(page, paneId))!.split('\n').filter(Boolean).length > 1);
  console.log('PASS gq hard wraps without a language server');

  // The config moves the leader and adds, replaces, and removes mappings.
  writeFileSync(
    config,
    [
      '[native-editor]',
      'vim = true',
      'leader = ","',
      'timeout = 300',
      '[native-editor.vim-keys]',
      '"<leader>x" = "editor-problems"',
      '"<leader>h" = "editor-hover"',
      '"<leader>hs" = "none"',
      '"<leader>p" = { action = "command-palette", desc = "Palette" }',
      '"jk" = { keys = "<Esc>", modes = ["insert"] }',
      '[native-editor.vim-groups]',
      '"<leader>h" = "git"',
      lsp,
    ].join('\n'),
  );
  await configured(page, true, ',x');
  await open(main);
  await until('main again', async () => (await lines(page, paneId))?.startsWith('let thing') ?? false);
  await content.click();
  await page.keyboard.type('ggw');
  await page.keyboard.press(',');
  await row('x').getByText('Problems').waitFor();
  await row('h').getByText('Hover (+git)').waitFor();
  assert.equal(await row('s').count(), 0);
  await page.keyboard.press('x');
  await pane.locator('.cm-panel-lint').waitFor();
  await content.click();
  // ,h is a mapping and a prefix: it runs once no more keys come.
  await page.keyboard.type('ggw,h');
  await page.locator('.cm-tooltip').getByText('Fake hover for thing').waitFor();
  await page.keyboard.press('Escape');
  // Space isn't the leader now.
  await page.keyboard.press(' ');
  await at('Ln 1, Col 6');
  assert.equal(await popup.count(), 0);
  // Insert-mode keys and a btmux action.
  await page.keyboard.type('ijk');
  await mode('NORMAL');
  assert.equal((await lines(page, paneId))?.includes('jk'), false);
  await page.keyboard.type(',p');
  await page.getByRole('dialog').getByRole('combobox').first().waitFor();
  await page.keyboard.press('Escape');
  console.log('PASS the config moves the leader and changes mappings');

  // Without vim: VS Code's Mod-k chords, F8, and Go Back.
  writeFileSync(config, lsp);
  await configured(page, false);
  await content.click();
  await page.keyboard.press('ControlOrMeta+Home');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ControlOrMeta+k');
  await row(process.platform === 'darwin' ? '<M-i>' : '<C-i>').getByText('Hover').waitFor();
  await page.keyboard.press('ControlOrMeta+i');
  await page.locator('.cm-tooltip').getByText('Fake hover for thing').waitFor();
  await page.keyboard.press('F12');
  await until('the definition', async () => (await lines(page, paneId)) === 'thing defined here\n');
  await page.keyboard.press(process.platform === 'darwin' ? 'Control+Minus' : 'Control+Alt+Minus');
  await until('back', async () => (await lines(page, paneId))?.startsWith('let thing') ?? false);
  await at('Ln 1, Col 5');
  await page.keyboard.press('Shift+F8');
  await at('Ln 1, Col 13');
  console.log('PASS VS Code chords, F8, and Go Back without vim');
} finally {
  await browser.close();
  await api(`/api/sessions/${session.id}`, 'DELETE').catch(() => {});
  rmSync(dir, { recursive: true, force: true });
}
