/**
 * Records one of the docs' demos with rrweb from a fresh stack, in dark or
 * light: DOCS_DEMO (panes, files, editor, agents, latex) and DOCS_DEMO_SCHEME.
 * Run all with `just docs-demos`; writes docs/public/demos/<demo>-<scheme>.json.gz
 * and the fonts and images it uses as asset-<hash>.<ext>, which the recording
 * names as `@demos@/asset-…` for the docs' player to resolve.
 *
 * A recording carries the whole DOM, not only what is visible, so text naming
 * the user's home is rewritten, and it fails if the home, the user's name, or
 * the token remain.
 */
import assert from 'node:assert/strict';
import { chromium, type Page } from 'playwright';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, realpathSync, symlinkSync, writeFileSync } from 'node:fs';
import { homedir, userInfo } from 'node:os';
import path from 'node:path';
import { gzipSync } from 'node:zlib';
import { waitForAsync } from './browser-test-utils';
import { createPlayground, stackCli } from './demo-playground';

const url = process.env.BTMUX_TEST_URL!;
const token = process.env.BTMUX_AUTH_TOKEN!;
const configHome = process.env.BTMUX_TEST_CONFIG_HOME!;
assert(url && token && configHome, 'run through browser-test-stack.ts');
const demos = ['panes', 'files', 'editor', 'agents', 'latex'] as const;
type Demo = (typeof demos)[number];
const demo = process.env.DOCS_DEMO as Demo;
const scheme = process.env.DOCS_DEMO_SCHEME ?? 'dark';
assert(demos.includes(demo), `DOCS_DEMO must be one of ${demos.join(', ')}`);
assert(scheme === 'dark' || scheme === 'light', 'DOCS_DEMO_SCHEME must be dark or light');
const root = path.dirname(import.meta.dir);
const out = path.join(root, 'docs', 'public', 'demos');
mkdirSync(out, { recursive: true });
if (demo === 'editor')
  assert(process.env.RUST_SRC_PATH, 'RUST_SRC_PATH is required (the Rust standard library sources)');

const { repo, router, site, ...playground } = createPlayground();
// Resolved, as shells report it: /tmp is a link on macOS.
const home = realpathSync(playground.home);
const btmux = stackCli(root, configHome, token);
const headers = { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' };

// Shells, including those the demos open, read this: a short prompt, the
// playground as HOME (new windows start in the real one), and the demo agents
// and btmux on PATH.
const work = path.join(path.dirname(configHome), 'demo');
const bin = path.join(work, 'bin');
const sync = path.join(work, 'sync');
mkdirSync(bin, { recursive: true });
mkdirSync(sync, { recursive: true });
symlinkSync(path.join(root, 'target', 'debug', 'btmux'), path.join(bin, 'btmux'));
for (const agent of ['claude', 'codex', 'gemini']) {
  writeFileSync(
    path.join(bin, agent),
    `#!/bin/bash\nexec -a ${agent} /bin/bash ${JSON.stringify(path.join(import.meta.dir, 'demo-agent.sh'))} ${agent} "$@"\n`,
    { mode: 0o755 },
  );
}
writeFileSync(
  path.join(configHome, 'shrc'),
  [
    `HOME=${JSON.stringify(home)}`,
    `PATH=${JSON.stringify(bin)}:$PATH`,
    `BTMUX_DEMO=${demo}`,
    `BTMUX_DEMO_SYNC=${JSON.stringify(sync)}`,
    'export HOME PATH BTMUX_DEMO BTMUX_DEMO_SYNC',
    `[ "$PWD" = ${JSON.stringify(homedir())} ] && cd "$HOME"`,
    `PS1='~\${PWD#"$HOME"} $ '`,
    '',
  ].join('\n'),
);

// The demo's sessions; the stack's default one goes.
const first = demo === 'editor' ? router : repo;
const firstPane = btmux('new-session', '-s', path.basename(first), '-n', 'main', '-c', first);
if (demo === 'panes') btmux('new-session', '-s', 'website', '-n', 'main', '-c', site);
btmux('kill-session', '-t', '0');
const startedAt = Date.now();
await Bun.sleep(800);

const browser = await chromium.launch({ headless: true, channel: process.env.BTMUX_TEST_BROWSER });
const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, colorScheme: scheme });
const page = await context.newPage();
// The fonts and images the page loads; the replay needs no others.
const loaded = new Set<string>();
page.on('requestfinished', (request) => {
  if (['font', 'image'].includes(request.resourceType())) loaded.add(request.url());
});
const recorder = readFileSync(path.join(path.dirname(require.resolve('@rrweb/record')), 'record.umd.min.cjs'), 'utf8');
// As a global, whatever module system the page has.
await page.addInitScript(`(function(){var module,exports,define;${recorder}\n})()`);
await page.goto(`${url}/s/${path.basename(first)}`);
await page.getByLabel('Access token').fill(token);
await page.getByRole('button', { name: 'Connect', exact: true }).click();
await waitForAsync(page, async () => {
  const { useStore } = await import('/src/state/store.tsx' as string);
  return !!useStore.getState().config;
});
await page.waitForTimeout(2500);
await page.getByLabel('Terminal').first().focus();
await page.keyboard.type('clear\n');
if (demo === 'editor') await warmEditor(page);
await page.waitForTimeout(500);

await page.evaluate(
  ({ home, user }) => {
    const w = window as any;
    w.demoEvents = [];
    const scrub = (text: string) => text.replaceAll(home, '~').replaceAll(`/${user}/`, '/ada/');
    w.stopDemo = w.rrwebRecord.record({
      emit: (event: unknown) => w.demoEvents.push(event),
      inlineStylesheet: true,
      recordCanvas: false,
      collectFonts: true,
      slimDOMOptions: 'all',
      maskTextSelector: '*',
      maskTextFn: scrub,
      sampling: { mousemove: 40, scroll: 100, input: 'last' },
    });
  },
  { home: homedir(), user: userInfo().username },
);
await pace(1);

const prefix = async (key: string) => {
  await page.keyboard.press('Control+a');
  await page.keyboard.press(key);
};
const type = (text: string) => page.keyboard.type(text, { delay: 45 });
const line = async (text: string) => {
  await type(text);
  await page.keyboard.press('Enter');
};
const press = async (key: string, times = 1, pause = 0.4) => {
  for (let i = 0; i < times; i++) {
    await page.keyboard.press(key);
    await pace(pause);
  }
};
function pace(seconds: number) {
  return page.waitForTimeout(seconds * 1000);
}
async function waitFor(marker: string, seconds: number) {
  const deadline = Date.now() + seconds * 1000;
  while (!existsSync(path.join(sync, marker))) {
    assert(Date.now() < deadline, `timed out waiting for ${marker}`);
    await Bun.sleep(200);
  }
}

// Opens main.rs through the API and waits for rust-analyzer, so the demo's
// completions come at once; then hides the editor again.
async function warmEditor(page: Page) {
  const opened = await fetch(`${url}/api/panes/${firstPane}/open-editor`, {
    method: 'POST',
    headers,
    body: JSON.stringify({ path: path.join(router, 'src', 'main.rs'), editor: 'native' }),
  });
  assert(opened.ok, `open-editor: ${opened.status}`);
  await page.locator('.cm-content').last().waitFor();
  // rust-analyzer loads the crate and the standard library.
  await page.waitForTimeout(Math.max(0, 25_000 - (Date.now() - startedAt)));
  await page.locator('.cm-content').last().focus();
  await page.keyboard.press('Escape');
  await page.waitForTimeout(100);
  await page.keyboard.press('Escape');
  await page.waitForTimeout(800);
  await page.getByLabel('Terminal').first().focus();
}

const scripts: Record<Demo, () => Promise<void>> = {
  // Splits, resizing, zoom, layouts, windows, the window grid, the session
  // picker, and the key bindings.
  async panes() {
    await line('echo "hello from btmux"');
    await pace(1);
    await prefix('%');
    await pace(1.2);
    await line('ls');
    await pace(1);
    await prefix('"');
    await pace(1.2);
    await line('git log --oneline -8');
    await pace(1.5);
    await prefix('ArrowLeft');
    await pace(0.8);
    for (let i = 0; i < 2; i++) {
      await prefix('Alt+ArrowRight');
      await pace(0.5);
    }
    await prefix('z');
    await pace(1.2);
    await prefix('z');
    await pace(1);
    for (let i = 0; i < 5; i++) {
      await prefix('Space');
      await pace(1);
    }
    // Rename this window, then open another.
    await prefix(',');
    await pace(0.8);
    await press('Backspace', 4, 0.05);
    await type('code');
    await pace(0.5);
    await page.keyboard.press('Enter');
    await pace(1);
    await prefix('c');
    await pace(1.2);
    await line('ls');
    await pace(1);
    await line('cd acme-api');
    await pace(0.6);
    await line('git status --short');
    await pace(1.5);
    await prefix('p');
    await pace(1.2);
    await prefix('n');
    await pace(1.2);
    // The window grid, back to the first window.
    await prefix('w');
    await pace(2);
    await press('l', 1, 1);
    await page.keyboard.press('Enter');
    await pace(1.5);
    // The session picker, over to the website session.
    await prefix('s');
    await pace(1.5);
    await press('j', 1, 1);
    await page.keyboard.press('Enter');
    await pace(1.2);
    await line('cat notes.md');
    await pace(1.5);
    await prefix('L');
    await pace(1.5);
    // The key bindings, down and back up.
    await prefix('?');
    await pace(1.5);
    await press('Control+d', 4, 0.7);
    await press('Control+u', 4, 0.5);
    await pace(0.5);
    await page.keyboard.press('Escape');
    await pace(1.5);
  },

  // The file browser's filter, file search, and content search, then Git mode.
  async files() {
    await prefix('f');
    await pace(1.5);
    await page.keyboard.press('/');
    await pace(0.4);
    await type('read');
    await pace(1.5);
    await page.keyboard.press('Escape');
    await pace(0.8);
    await page.keyboard.press('f');
    await pace(0.6);
    await type('routes');
    await pace(1.5);
    await press('Control+n', 3, 1);
    await page.keyboard.press('Escape');
    await pace(0.6);
    await page.keyboard.press('s');
    await pace(0.6);
    await type('Response');
    await pace(1.5);
    await press('Control+n', 4, 1);
    await page.keyboard.press('Escape');
    await pace(0.3);
    await page.keyboard.press('Escape');
    await pace(0.8);
    // Git mode: the first Escape leaves it, the second closes.
    await prefix('g');
    await pace(2);
    await press('j', 3, 0.8);
    await pace(1.5);
    await page.keyboard.press('Escape');
    await pace(0.3);
    await page.keyboard.press('Escape');
    await pace(1);
  },

  // The native editor over a pane: quick open, a completion from
  // rust-analyzer, Git signs, saving, and the change in the shell.
  async editor() {
    await line('btmux open-editor src/main.rs');
    await pace(1.5);
    await page.keyboard.press('ControlOrMeta+p');
    await pace(0.6);
    await type('heal');
    await pace(1.2);
    await page.keyboard.press('Escape');
    await pace(0.6);
    // Below the /health arm in respond().
    await page.keyboard.press('ControlOrMeta+f');
    await pace(0.4);
    await type('"/health")');
    await page.keyboard.press('Enter');
    await page.keyboard.press('Escape');
    await pace(0.6);
    await page.keyboard.press('End');
    await page.keyboard.press('Enter');
    await type('("GET", "/path") => (200, request.');
    await pickCompletion(page, 'path');
    await type('.clone()),');
    await pace(1.5);
    await page.keyboard.press('ControlOrMeta+s');
    await pace(1.5);
    await page.keyboard.press('Escape');
    await page.waitForTimeout(100);
    await page.keyboard.press('Escape');
    await pace(1);
    await line('git diff');
    await pace(3);
  },

  // Three agents in split panes, then the agent view once Claude waits for
  // permission and Codex is done.
  async agents() {
    await line('claude');
    await pace(1.5);
    await line('add pagination to GET /orders');
    await pace(1.5);
    await prefix('%');
    await pace(1.2);
    await line('codex');
    await pace(1.5);
    await line('why do the server tests flake?');
    await pace(1.5);
    await prefix('"');
    await pace(1.2);
    await line('gemini');
    await pace(1.5);
    await line('explain the routing');
    await waitFor('claude-blocked', 30);
    await waitFor('codex-done', 30);
    await pace(2.5);
    // Jump to Claude and approve.
    await prefix('a');
    await pace(2.5);
    await press('1', 1, 1);
    await page.keyboard.press('Enter');
    await pace(1.5);
    await page.keyboard.press('y');
    await waitFor('claude-done', 30);
    await pace(1.5);
    await waitFor('gemini-done', 60);
    await pace(2.5);
    // Everyone is done; jump to Gemini's answer.
    await prefix('a');
    await pace(2.5);
    await press('3', 1, 1);
    await page.keyboard.press('Enter');
    await pace(2.5);
  },

  // LaTeX in agent output, rendered in the side panel; hovering a formula
  // highlights its source.
  async latex() {
    await line('codex');
    await pace(1.5);
    await line('why is the Gaussian integral sqrt(pi)?');
    await waitFor('codex-done', 30);
    await pace(1.5);
    await prefix('m');
    await pace(2);
    const formulas = page.locator('div[title]').filter({ has: page.locator('.katex') });
    for (const formula of await formulas.all()) {
      const box = (await formula.boundingBox())!;
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2, { steps: 15 });
      await pace(1);
    }
    await page.mouse.move(1000, 620, { steps: 15 });
    await pace(1);
    await prefix('m');
    await pace(1.5);
  },
};

// Picks the completion whose label starts with `label`, asking again until
// rust-analyzer answers.
async function pickCompletion(page: Page, label: string) {
  const menu = page.locator('.cm-tooltip-autocomplete');
  const selected = menu.locator('[aria-selected="true"]');
  for (let step = 0; step < 30; step++) {
    if ((await selected.count()) === 0) {
      await page.keyboard.press('Control+Space');
      await pace(1.5);
      continue;
    }
    const current = await selected.textContent({ timeout: 1000 }).catch(() => null);
    if (current?.startsWith(label)) {
      await pace(0.8);
      await page.keyboard.press('Enter');
      await pace(0.5);
      return;
    }
    await page.keyboard.press('ArrowDown');
    await pace(0.35);
  }
  assert.fail(`no ${label} completion from rust-analyzer`);
}

await scripts[demo]();

const events: any[] = await page.evaluate(() => {
  const w = window as any;
  w.stopDemo();
  return w.demoEvents;
});
await browser.close();
// Vite tags its dev styles with their source paths.
const strip = (value: any) => {
  if (!value || typeof value !== 'object') return;
  if (value.attributes && typeof value.attributes === 'object') delete value.attributes['data-vite-dev-id'];
  for (const child of Object.values(value)) strip(child);
};
strip(events);
let json = JSON.stringify(events);

// Fonts and images the page loaded from the stack become files beside the
// recording; other stack URLs go nowhere, so the docs never ask a viewer's
// localhost.
const port = new URL(url).port;
const stackUrl = new RegExp(`https?://(?:localhost|127\\.0\\.0\\.1):${port}(/[^"'()\\s\\\\]*)?`, 'g');
const assetTypes = /\.(woff2?|ttf|otf|png|jpe?g|gif|svg|webp|avif)$/;
const assets = new Map<string, string>();
for (const [found, pathname = '/'] of json.matchAll(stackUrl)) {
  if (assets.has(found)) continue;
  const ext = new URL(pathname, url).pathname.match(assetTypes)?.[1];
  if (!ext || !loaded.has(new URL(pathname, url).href)) {
    assets.set(found, 'about:invalid');
    continue;
  }
  const response = await fetch(new URL(pathname, url), { headers });
  assert(response.ok, `asset ${pathname}: ${response.status}`);
  const bytes = new Uint8Array(await response.arrayBuffer());
  const name = `asset-${createHash('sha256').update(bytes).digest('hex').slice(0, 16)}.${ext}`;
  writeFileSync(path.join(out, name), bytes);
  assets.set(found, `@demos@/${name}`);
}
json = json.replace(stackUrl, (found) => assets.get(found)!);
// Vite's inline source maps.
json = json.replace(/\/\*# sourceMappingURL=data:[^*]*\*\//g, '');

for (const [secret, what] of [
  [token, 'the access token'],
  [homedir(), 'the home directory'],
  [new RegExp(`\\b${userInfo().username}\\b`), "the user's name"],
] as const) {
  const at = typeof secret === 'string' ? json.indexOf(secret) : json.search(secret);
  assert(at < 0, `the recording contains ${what}: …${json.slice(Math.max(0, at - 300), at)}…`);
}

const file = path.join(out, `${demo}-${scheme}.json.gz`);
writeFileSync(file, gzipSync(json, { level: 9 }));
const seconds = (events.at(-1).timestamp - events[0].timestamp) / 1000;
console.log(
  `${path.relative(root, file)}: ${seconds.toFixed(1)} s, ${events.length} events, ` +
    `${(json.length / 1024).toFixed(0)} KiB, ${(readFileSync(file).length / 1024).toFixed(0)} KiB gzipped, ` +
    `${[...assets.values()].filter((v) => v.startsWith('@')).length} assets`,
);
