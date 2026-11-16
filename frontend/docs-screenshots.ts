/**
 * Takes the docs' screenshots from a fresh stack with demo sessions, each in
 * dark and light (`<name>-dark.png`, `<name>-light.png`; docs CSS shows the
 * one matching the site's theme). Run with `just docs-screenshots`; writes
 * docs/src/assets/screenshots.
 */
import assert from 'node:assert/strict';
import { chromium, type Page } from 'playwright';
import { execFileSync } from 'node:child_process';
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { waitForAsync } from './browser-test-utils';

const url = process.env.BTMUX_TEST_URL!;
const token = process.env.BTMUX_AUTH_TOKEN!;
const configHome = process.env.BTMUX_TEST_CONFIG_HOME!;
assert(url && token && configHome, 'run through browser-test-stack.ts');
const root = path.dirname(import.meta.dir);
const out = path.join(root, 'docs', 'src', 'assets', 'screenshots');
mkdirSync(out, { recursive: true });
// DOCS_SHOTS=latex,git-mode retakes only those.
const only = (process.env.DOCS_SHOTS ?? '').split(',').filter(Boolean);

// A short path, since the UI shows it.
const home = process.platform === 'win32' ? path.join(tmpdir(), 'btmux-demo') : '/tmp/btmux-demo';
rmSync(home, { recursive: true, force: true });
const repo = path.join(home, 'acme-api');
mkdirSync(path.join(repo, 'src', 'routes'), { recursive: true });
mkdirSync(path.join(repo, 'tests'), { recursive: true });
const write = (file: string, text: string) => writeFileSync(path.join(repo, file), text);
const git = (...args: string[]) =>
  execFileSync('git', ['-c', 'user.name=Ada', '-c', 'user.email=ada@example.invalid', ...args], {
    cwd: repo,
    env: { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1' },
  });

git('init', '--initial-branch=main');
write(
  'README.md',
  '# acme-api\n\nThe Acme HTTP API.\n\n## Develop\n\n```sh\nbun install\nbun dev\n```\n\n## Routes\n\n- `GET /health`\n- `GET /users/:id`\n- `POST /orders`\n',
);
write(
  'package.json',
  '{\n  "name": "acme-api",\n  "scripts": { "dev": "bun --watch src/server.ts", "test": "bun test" }\n}\n',
);
write('.gitignore', 'node_modules\n.env\n');
write(
  'src/server.ts',
  `import { health } from './routes/health';\nimport { users } from './routes/users';\n\nconst routes = [health, users];\n\nexport function handle(request: Request): Response | Promise<Response> {\n  const { pathname } = new URL(request.url);\n  for (const route of routes) {\n    const match = route.pattern.exec(pathname);\n    if (match) return route.handler(request, match);\n  }\n  return new Response('Not found', { status: 404 });\n}\n\nBun.serve({ port: 3000, fetch: handle });\n`,
);
write(
  'src/routes/health.ts',
  `export const health = {\n  pattern: /^\\/health$/,\n  handler: () => Response.json({ ok: true }),\n};\n`,
);
write(
  'src/routes/users.ts',
  `const users = new Map([[1, { id: 1, name: 'Ada' }]]);\n\nexport const users_route = {\n  pattern: /^\\/users\\/(\\d+)$/,\n  handler: (_: Request, match: RegExpExecArray) => {\n    const user = users.get(Number(match[1]));\n    return user ? Response.json(user) : new Response('Not found', { status: 404 });\n  },\n};\n\nexport { users_route as users };\n`,
);
write(
  'tests/server.test.ts',
  `import { expect, test } from 'bun:test';\nimport { handle } from '../src/server';\n\ntest('health', async () => {\n  const response = handle(new Request('http://x/health'));\n  expect(await response.json()).toEqual({ ok: true });\n});\n`,
);
git('add', '.');
git('commit', '-m', 'Add the health and users routes');
git('switch', '-c', 'orders');
write(
  'src/routes/orders.ts',
  `export const orders = {\n  pattern: /^\\/orders$/,\n  handler: async (request: Request) => {\n    const order = await request.json();\n    return Response.json({ id: crypto.randomUUID(), ...order }, { status: 201 });\n  },\n};\n`,
);
git('add', '.');
git('commit', '-m', 'Add POST /orders');
git('switch', 'main');
write(
  'src/routes/health.ts',
  `export const health = {\n  pattern: /^\\/health$/,\n  handler: () => Response.json({ ok: true, uptime: process.uptime() }),\n};\n`,
);
git('commit', '-am', 'Report uptime in /health');
git('merge', '--no-ff', '-m', 'Merge orders', 'orders');
// Work in progress: one staged, one unstaged, one untracked.
write(
  'src/server.ts',
  `import { health } from './routes/health';\nimport { orders } from './routes/orders';\nimport { users } from './routes/users';\n\nconst routes = [health, users, orders];\n\nexport function handle(request: Request): Response | Promise<Response> {\n  const { pathname } = new URL(request.url);\n  for (const route of routes) {\n    const match = route.pattern.exec(pathname);\n    if (match) return route.handler(request, match);\n  }\n  return Response.json({ error: 'not found' }, { status: 404 });\n}\n\nBun.serve({ port: Number(process.env.PORT ?? 3000), fetch: handle });\n`,
);
git('add', 'src/server.ts');
write(
  'README.md',
  '# acme-api\n\nThe Acme HTTP API.\n\n## Develop\n\n```sh\nbun install\nbun dev\n```\n\nSet `PORT` to listen elsewhere.\n\n## Routes\n\n- `GET /health`\n- `GET /users/:id`\n- `POST /orders`\n',
);
write('tests/orders.test.ts', `import { expect, test } from 'bun:test';\n\ntest.todo('creates an order');\n`);
// Types for the runtime globals, so the language server reports no errors.
write(
  'src/globals.d.ts',
  'declare const Bun: { serve(options: { port: number; fetch: (request: Request) => Response | Promise<Response> }): void };\ndeclare const process: { env: Record<string, string | undefined>; uptime(): number };\n',
);
write(
  'tsconfig.json',
  '{\n  "compilerOptions": { "target": "esnext", "module": "esnext", "moduleResolution": "bundler" },\n  "include": ["src"]\n}\n',
);
git('add', 'src/globals.d.ts', 'tsconfig.json');
git('commit', '-m', 'Declare runtime globals');

// A language server for the Neovim shots, outside the demo repo, and a Neovim
// config that uses it with native completion, as the Neovim page suggests.
const tools = path.join(home, 'tools');
mkdirSync(tools);
writeFileSync(path.join(tools, 'package.json'), '{}\n');
execFileSync('bun', ['add', 'typescript-language-server', 'typescript@5'], { cwd: tools, stdio: 'ignore' });
mkdirSync(path.join(configHome, 'nvim'), { recursive: true });
writeFileSync(
  path.join(configHome, 'nvim', 'init.lua'),
  `vim.o.number = true
vim.o.signcolumn = 'no'
vim.o.autocomplete = true
vim.o.completeopt = 'menuone,noselect,fuzzy,popup'
vim.lsp.config('ts', {
  cmd = { ${JSON.stringify(path.join(tools, 'node_modules', '.bin', 'typescript-language-server'))}, '--stdio' },
  filetypes = { 'typescript' },
  root_markers = { 'package.json' },
})
vim.lsp.enable('ts')
vim.api.nvim_create_autocmd('LspAttach', {
  callback = function(ev)
    vim.lsp.completion.enable(true, ev.data.client_id, ev.buf, { autotrigger = true })
  end,
})
`,
);

const site = path.join(home, 'website');
mkdirSync(site);
writeFileSync(path.join(site, 'index.html'), '<h1>Acme</h1>\n');
writeFileSync(
  path.join(site, 'notes.md'),
  String.raw`Retry delay with exponential backoff and jitter:

$$d_n = \min\left(d_{\max},\; d_0 \cdot 2^n\right) \cdot (1 + \epsilon)$$

where $\epsilon \sim U(0, 0.25)$. The expected total wait is

\[ E[T] = \sum_{n=0}^{N} d_0 \, 2^n \left(1 + \tfrac{1}{8}\right) \]
`,
);

// The CLI finds the stack's server through its state directory.
const btmux = (...args: string[]) =>
  execFileSync(path.join(root, 'target', 'debug', 'btmux'), args, {
    encoding: 'utf8',
    env: {
      // Not this terminal's own server, if it runs in a btmux pane.
      ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('BTMUX_'))),
      XDG_STATE_HOME: path.join(path.dirname(configHome), 'state'),
      BTMUX_AUTH_TOKEN: token,
    },
  }).trim();
const run = (pane: string, command: string) => btmux('send-keys', '-t', pane, command, 'Enter');
const prompt = (pane: string) => run(pane, "PS1='$ '; clear");

const dev = btmux('new-session', '-s', 'acme-api', '-n', 'dev', '-c', repo);
const side = btmux('split-window', '-t', dev, '-h', '-d', '-c', repo);
const below = btmux('split-window', '-t', side, '-v', '-d', '-c', repo);
const server = btmux('new-window', '-t', 'acme-api', '-n', 'server', '-d', '-c', repo);
const web = btmux('new-session', '-s', 'website', '-n', 'site', '-c', site);
const agents = [
  ['orders-api', 'claude', 'working', 'Add pagination to GET /orders'],
  ['flaky-tests', 'codex', 'blocked', 'Fix the race in tests/server.test.ts'],
  ['docs', 'claude', 'done', 'Document the PORT variable'],
].map(([name, agent, state, text]) => ({
  name,
  agent,
  state,
  text,
  pane: btmux('new-session', '-s', name, '-n', agent, '-c', repo),
}));
// The stack's default session.
btmux('kill-session', '-t', '0');
for (const pane of [dev, side, below, server, web, ...agents.map((a) => a.pane)]) prompt(pane);
await Bun.sleep(800);
// One at a time: a shell still starting a command drops typed characters.
for (const [pane, command] of [
  [dev, 'git log --oneline --graph'],
  [side, 'cat src/routes/health.ts'],
  [below, 'ls src tests'],
  [server, 'cat package.json'],
  [web, 'cat notes.md'],
  [dev, 'git status --short'],
]) {
  run(pane, command);
  await Bun.sleep(400);
}
const headers = { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' };
// Stand-ins for agents: a prompt on screen and a reported state.
for (const agent of agents) {
  run(agent.pane, `clear; echo '> ${agent.text}'; sleep 99999`);
  await Bun.sleep(300);
  const report = await fetch(`${url}/api/panes/${agent.pane}/agent-status`, {
    method: 'POST',
    headers,
    body: JSON.stringify({ state: agent.state, agent: agent.agent, source: 'hook' }),
  });
  assert(report.ok, `agent status: ${report.status}`);
}

const overlay = (mode: string, focus?: string) =>
  fetch(`${url}/api/panes/${dev}/open-file-browser`, {
    method: 'POST',
    headers,
    body: JSON.stringify({ path: repo, mode, focus_file: focus }),
  }).then((response) => assert(response.ok, `open ${mode}: ${response.status}`));

const browser = await chromium.launch({ headless: true, channel: process.env.BTMUX_TEST_BROWSER });
try {
  // `appearance = "auto"` follows the browser's color scheme.
  for (const scheme of ['dark', 'light'] as const) {
    const context = await browser.newContext({
      viewport: { width: 1360, height: 850 },
      deviceScaleFactor: 2,
      colorScheme: scheme,
    });
    const page = await context.newPage();
    const store = (call: string) =>
      page.evaluate(async (call) => {
        const { useStore } = await import('/src/state/store.tsx' as string);
        new Function('s', `s.${call}`)(useStore.getState());
      }, call);
    const shot = async (name: string, open: () => Promise<unknown>, close: () => Promise<unknown>, settle = 1200) => {
      if (only.length && !only.includes(name)) return;
      try {
        await open();
        await page.waitForTimeout(settle);
        await page.screenshot({ path: path.join(out, `${name}-${scheme}.png`) });
        console.log(`shot ${name}-${scheme}`);
      } catch (error) {
        console.error(`FAILED ${name}-${scheme}: ${error}`);
        process.exitCode = 1;
      }
      await close();
      await page.waitForTimeout(400);
    };
    const escape = () => page.keyboard.press('Escape');

    const signIn = async (page: Page, session: string) => {
      await page.goto(`${url}/s/${session}`);
      await page.getByLabel('Access token').fill(token);
      await page.getByRole('button', { name: 'Connect', exact: true }).click();
      await waitForAsync(page, async () => {
        const { useStore } = await import('/src/state/store.tsx' as string);
        return !!useStore.getState().config;
      });
      await page.waitForTimeout(2500);
    };
    await signIn(page, 'acme-api');

    await shot(
      'panes',
      async () => {},
      async () => {},
    );
    await shot(
      'command-palette',
      async () => {
        await page.getByLabel('Terminal').first().focus();
        await page.keyboard.press('Control+a');
        await page.keyboard.press(':');
      },
      escape,
    );
    await shot(
      'session-switcher',
      async () => {
        await store('setSwitcherOpen(true)');
        await page.waitForTimeout(800);
        await page.keyboard.press('ArrowRight');
      },
      () => store('setSwitcherOpen(false)'),
      2000,
    );
    await shot(
      'window-grid',
      () => store('setWindowGridOpen(true)'),
      () => store('setWindowGridOpen(false)'),
      2500,
    );
    await shot(
      'file-browser',
      () => overlay('files', 'README.md'),
      () => store('setFileBrowserOpen(false)'),
      1800,
    );
    await shot(
      'git-mode',
      async () => {
        await overlay('git');
        await page.waitForTimeout(1200);
        await page.keyboard.press('j');
      },
      () => store('setFileBrowserOpen(false)'),
      1800,
    );
    // No process viewer: it lists the host's own processes, users, and paths.
    await shot(
      'agent-view',
      () => store('setAgentGridOpen(true)'),
      () => store('setAgentGridOpen(false)'),
      3000,
    );
    await shot(
      'settings',
      () => store('setSettingsOpen(true)'),
      () => store('setSettingsOpen(false)'),
    );
    await context.close();

    // Close-ups: a small window, so text is large on the page.
    const closeUp = async (
      session: string,
      names: string[],
      take: (page: Page, save: (name: string) => Promise<void>) => Promise<void>,
    ) => {
      if (only.length && !names.some((name) => only.includes(name))) return;
      const small = await browser.newContext({
        viewport: { width: 960, height: 600 },
        deviceScaleFactor: 2,
        colorScheme: scheme,
      });
      const page = await small.newPage();
      try {
        await signIn(page, session);
        await take(page, async (name) => {
          await page.waitForTimeout(1200);
          await page.screenshot({ path: path.join(out, `${name}-${scheme}.png`) });
          console.log(`shot ${name}-${scheme}`);
        });
      } catch (error) {
        console.error(`FAILED ${names}-${scheme}: ${error}`);
        process.exitCode = 1;
      }
      await small.close();
    };

    // Neovim over the single pane of the server window, with the config above.
    btmux('select-window', '-t', 'acme-api:server');
    await closeUp('acme-api', ['neovim-completion', 'neovim-hover', 'neovim-cmdline'], async (page, save) => {
      await page.evaluate(async (pane) => {
        const { useStore } = await import('/src/state/store.tsx' as string);
        useStore.getState().setNvimOpen(true, pane);
      }, server);
      await page.locator('[data-nvim-input]').waitFor({ state: 'attached' });
      await page.waitForTimeout(2500);
      await page.locator('[data-nvim-input]').focus();
      const command = async (text: string) => {
        await page.keyboard.type(`:${text}`);
        await page.keyboard.press('Enter');
        await page.waitForTimeout(300);
      };
      // One Escape at a time: a second within 300 ms hides the surface.
      const escape = async () => {
        await page.keyboard.press('Escape');
        await page.waitForTimeout(600);
      };
      await command('e! src/server.ts');
      // The language server starts and loads the project.
      await page.waitForTimeout(scheme === 'dark' ? 12_000 : 3000);
      // LSP completion of a route's members, inside the loop of handle().
      await page.keyboard.type('9Goroute.');
      await page.waitForTimeout(2500);
      await page.keyboard.press('Control+n');
      await save('neovim-completion');
      await escape();
      await command('e!');
      // LSP hover on handle.
      await page.keyboard.type('7G2wK');
      await page.waitForTimeout(1500);
      await save('neovim-hover');
      await page.keyboard.type('j');
      await page.waitForTimeout(400);
      await page.keyboard.type(':set fold');
      await page.keyboard.press('Tab');
      await save('neovim-cmdline');
      await escape();
      await page.evaluate(async () => {
        const { useStore } = await import('/src/state/store.tsx' as string);
        useStore.getState().setNvimOpen(false);
      });
    });
    btmux('select-window', '-t', 'acme-api:dev');

    await closeUp('website', ['latex'], async (page, save) => {
      await page.evaluate(async (pane) => {
        const { useStore } = await import('/src/state/store.tsx' as string);
        useStore.getState().toggleLatex(pane);
      }, web);
      await page.waitForTimeout(2500);
      await save('latex');
    });
  }
} finally {
  await browser.close();
  rmSync(home, { recursive: true, force: true });
}
