/**
 * Run each browser test file against its own fresh btmux stack: a debug
 * backend and Vite on free ports, with a random token, `/bin/sh`, and
 * temporary XDG and Git config, so neither the user's config, profiles, shell,
 * nor a running `just dev` affects it. Shells read `shrc` in the config
 * directory (`ENV`) if a test writes one. A fake `multipass` (`test-bin`) logs to
 * the stack's `multipass` directory, and a fake `gh` answers from its `gh`
 * directory.
 *
 * Usage: bun browser-test-stack.ts [--jobs N] [--shard I/N] <test.ts>...
 * `--jobs` runs that many files at once (default `BTMUX_TEST_JOBS`, else 1 in
 * CI and up to 4 by core count elsewhere), printing each file's output when it
 * ends. `--shard I/N` runs every Nth file, starting with the Ith.
 */
import { spawn, type ChildProcess } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { mkdirSync, mkdtempSync, openSync, readFileSync, rmSync } from 'node:fs';
import { createServer } from 'node:net';
import { availableParallelism, tmpdir } from 'node:os';
import path from 'node:path';

const frontend = import.meta.dir;
const root = path.dirname(frontend);

function usage(): never {
  console.error('usage: bun browser-test-stack.ts [--jobs N] [--shard I/N] <test.ts>...');
  process.exit(2);
}

let jobs =
  Number(process.env.BTMUX_TEST_JOBS) ||
  (process.env.CI ? 1 : Math.min(4, Math.max(1, Math.floor(availableParallelism() / 4))));
let shard = { index: 1, count: 1 };
let tests: string[] = [];
const args = process.argv.slice(2);
for (let i = 0; i < args.length; i++) {
  if (args[i] === '--jobs') jobs = Number(args[++i]);
  else if (args[i] === '--shard') {
    const match = /^(\d+)\/(\d+)$/.exec(args[++i] ?? '');
    if (!match) usage();
    shard = { index: Number(match[1]), count: Number(match[2]) };
  } else tests.push(args[i]);
}
if (!tests.length || !Number.isInteger(jobs) || jobs < 1 || shard.index < 1 || shard.index > shard.count) usage();
tests = tests.filter((_, i) => i % shard.count === shard.index - 1);

let interrupted = false;
const live = new Set<ChildProcess>();
for (const signal of ['SIGINT', 'SIGTERM'] as const) {
  process.on(signal, () => {
    interrupted = true;
    for (const child of live) kill(child, 'SIGTERM');
  });
}

function kill(child: ChildProcess, signal: NodeJS.Signals) {
  try {
    // Detached children lead their own process group (Vite runs under bunx).
    process.kill(-child.pid!, signal);
  } catch {
    child.kill(signal);
  }
}

function exited(child: ChildProcess) {
  return new Promise<number>((resolve) => {
    if (child.exitCode !== null || child.signalCode !== null) resolve(child.exitCode ?? 1);
    else child.once('exit', (code) => resolve(code ?? 1));
  });
}

async function stop(child: ChildProcess) {
  kill(child, 'SIGTERM');
  const timeout = Bun.sleep(5000).then(() => 'timeout' as const);
  if ((await Promise.race([exited(child), timeout])) === 'timeout') kill(child, 'SIGKILL');
  await exited(child);
  live.delete(child);
}

function start(command: string, args: string[], options: { cwd: string; env: NodeJS.ProcessEnv; log?: string }) {
  const output = options.log ? openSync(options.log, 'a') : 'inherit';
  const child = spawn(command, args, {
    cwd: options.cwd,
    env: options.env,
    detached: !!options.log,
    stdio: ['ignore', output, output],
  });
  live.add(child);
  return child;
}

function freePort() {
  return new Promise<number>((resolve, reject) => {
    const server = createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address() as { port: number };
      server.close(() => resolve(port));
    });
  });
}

/** The caller's environment without btmux pane variables or shell/editor config. */
function baseEnv() {
  const env = { ...process.env };
  for (const key of Object.keys(env)) if (key.startsWith('BTMUX_') && key !== 'BTMUX_TEST_BROWSER') delete env[key];
  delete env.ENV;
  delete env.BASH_ENV;
  // `test-editor` (`test-bin`) is the server's `$EDITOR`.
  delete env.VISUAL;
  env.EDITOR = 'test-editor';
  return env;
}

async function withStack(
  dir: string,
  run: (url: string, backendUrl: string, token: string, configHome: string) => Promise<number>,
) {
  const [backendPort, vitePort] = [await freePort(), await freePort()];
  const token = randomBytes(18).toString('base64url');
  const url = `http://localhost:${vitePort}`;
  const xdg = Object.fromEntries(
    ['CONFIG', 'STATE', 'DATA', 'CACHE'].map((name) => {
      const home = path.join(dir, name.toLowerCase());
      mkdirSync(home, { recursive: true });
      return [`XDG_${name}_HOME`, home];
    }),
  );
  const backend = start(
    binary,
    [
      '--no-browser',
      '--host',
      '127.0.0.1',
      '--port',
      String(backendPort),
      '--shell',
      '/bin/sh',
      '--public-url',
      url,
      '--public-url',
      `http://127.0.0.1:${vitePort}`,
    ],
    {
      cwd: dir,
      env: {
        ...baseEnv(),
        ...xdg,
        PATH: `${path.join(frontend, 'test-bin')}${path.delimiter}${process.env.PATH}`,
        BTMUX_FAKE_MULTIPASS: path.join(dir, 'multipass'),
        BTMUX_FAKE_GH: path.join(dir, 'gh'),
        BTMUX_FAKE_AGY: path.join(dir, 'agy'),
        BTMUX_FAKE_LSP: path.join(dir, 'lsp'),
        BTMUX_AUTH_TOKEN: token,
        ENV: path.join(xdg.XDG_CONFIG_HOME, 'shrc'),
        GIT_CONFIG_GLOBAL: '/dev/null',
        GIT_CONFIG_NOSYSTEM: '1',
      },
      log: path.join(dir, 'backend.log'),
    },
  );
  const vite = start('bunx', ['vite', '--port', String(vitePort), '--strictPort'], {
    cwd: frontend,
    env: { ...baseEnv(), BTMUX_BACKEND_PORT: String(backendPort) },
    log: path.join(dir, 'vite.log'),
  });
  try {
    const deadline = Date.now() + 60_000;
    for (;;) {
      if (backend.exitCode !== null || vite.exitCode !== null) throw new Error('stack exited during startup');
      try {
        const response = await fetch(`${url}/api/sessions`, { headers: { Authorization: `Bearer ${token}` } });
        if (response.ok) break;
      } catch {}
      if (Date.now() > deadline) throw new Error('stack did not become ready within 60s');
      if (interrupted) return 130;
      await Bun.sleep(200);
    }
    return await run(url, `http://127.0.0.1:${backendPort}`, token, xdg.XDG_CONFIG_HOME);
  } finally {
    await Promise.all([stop(vite), stop(backend)]);
  }
}

const temp = mkdtempSync(path.join(tmpdir(), 'btmux-browser-test-'));
// Vite's dependency cache is shared, so it's built once before stacks start.
const optimizeLog = path.join(temp, 'vite-optimize.log');
const optimize = start('bunx', ['vite', 'optimize'], { cwd: frontend, env: baseEnv(), log: optimizeLog });
const build = start('cargo', ['build'], { cwd: root, env: process.env });
const [built, optimized] = await Promise.all([exited(build), exited(optimize)]);
live.delete(build);
live.delete(optimize);
if (built !== 0) process.exit(1);
if (optimized !== 0) {
  console.error(readFileSync(optimizeLog, 'utf8'));
  process.exit(1);
}
const binary = path.resolve(root, process.env.CARGO_TARGET_DIR ?? 'target', 'debug', 'btmux');

const failed: string[] = [];

async function runFile(index: number, test: string) {
  const dir = path.join(temp, `${index + 1}-${path.basename(test, '.ts')}`);
  mkdirSync(dir);
  // Run alone, output streams; otherwise it's printed whole at the end.
  const log = jobs > 1 ? path.join(dir, 'test.log') : undefined;
  if (!log) console.log(`\n=== ${test}`);
  const startedAt = Date.now();
  let code: number;
  let error = '';
  try {
    code = await withStack(dir, async (url, backendUrl, token, configHome) => {
      const child = start('bun', [test], {
        cwd: frontend,
        env: {
          ...process.env,
          BTMUX_TEST_URL: url,
          BTMUX_TEST_BACKEND_URL: backendUrl,
          BTMUX_AUTH_TOKEN: token,
          BTMUX_TEST_CONFIG_HOME: configHome,
          BTMUX_TEST_MULTIPASS: path.join(dir, 'multipass'),
          BTMUX_TEST_GH: path.join(dir, 'gh'),
          BTMUX_TEST_AGY: path.join(dir, 'agy'),
          BTMUX_TEST_LSP: path.join(dir, 'lsp'),
        },
        log,
      });
      const code = await exited(child);
      live.delete(child);
      return code;
    });
  } catch (caught) {
    error = String(caught);
    code = 1;
  }
  if (log) {
    console.log(`\n=== ${test}`);
    try {
      process.stdout.write(readFileSync(log, 'utf8'));
    } catch {}
  }
  if (error) console.error(error);
  const seconds = Math.round((Date.now() - startedAt) / 1000);
  if (code !== 0) {
    failed.push(test);
    console.error(`FAIL ${test} (${seconds}s); stack logs in ${dir}`);
  } else {
    console.log(`ok ${test} (${seconds}s)`);
    rmSync(dir, { recursive: true, force: true });
  }
}

let next = 0;
await Promise.all(
  Array.from({ length: Math.min(jobs, tests.length) }, async () => {
    while (!interrupted && next < tests.length) {
      const index = next++;
      await runFile(index, tests[index]);
    }
  }),
);

if (failed.length || interrupted) {
  if (failed.length) console.error(`\n${failed.length} of ${tests.length} failed: ${failed.join(', ')}`);
  process.exit(1);
}
rmSync(temp, { recursive: true, force: true });
console.log(`\nAll ${tests.length} browser test files passed.`);
