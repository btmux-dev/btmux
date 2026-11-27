/**
 * `just dev-tailscale`: `just dev` served to the tailnet over HTTPS (Tailscale
 * Serve), for testing on a phone. It signs in with a random token, never the
 * fixed dev one or printed, and prints a one-time pairing link and its QR code;
 * Enter prints a new one. It refuses a port Serve uses for anything else, or exposes to the
 * internet (Funnel), and removes only a route it made, if it's still there.
 *
 * Usage: bun dev-tailscale.ts FRONTEND_PORT BACKEND_PORT
 */
import { randomBytes } from 'node:crypto';
import { renderUnicodeCompact } from 'uqr';

async function tailscale(...args: string[]): Promise<string> {
  const child = Bun.spawn(['tailscale', ...args], { stdout: 'pipe', stderr: 'pipe' });
  const [out, err, code] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  if (code !== 0) throw new Error(`tailscale ${args.join(' ')} failed: ${err.trim() || `exit ${code}`}`);
  return out;
}

interface ServeConfig {
  TCP?: Record<string, unknown>;
  Web?: Record<string, { Handlers?: Record<string, { Proxy?: string }> }>;
  AllowFunnel?: Record<string, boolean>;
}

/** Serve's configuration; a node without one prints `null` or nothing. */
export function parseServeStatus(output: string): ServeConfig {
  const text = output.trim();
  if (!text) return {};
  const config = JSON.parse(text) as ServeConfig | null;
  return config ?? {};
}

/** How the port is used: free, already this route (tailnet only), or anything else. */
export function routeState(config: ServeConfig, hostPort: string, port: number, target: string) {
  if (config.AllowFunnel?.[hostPort]) return 'funnel';
  const web = config.Web?.[hostPort];
  if (!web && !config.TCP?.[String(port)]) return 'free';
  const handlers = Object.entries(web?.Handlers ?? {});
  return handlers.length === 1 && handlers[0][0] === '/' && handlers[0][1].Proxy === target ? 'ours' : 'taken';
}

async function main() {
  const [frontendPort, backendPort] = process.argv.slice(2).map(Number);
  if (!frontendPort || !backendPort) throw new Error('usage: bun dev-tailscale.ts FRONTEND_PORT BACKEND_PORT');
  const target = `http://127.0.0.1:${frontendPort}`;
  const status = JSON.parse(await tailscale('status', '--json')) as {
    BackendState?: string;
    Self?: { DNSName?: string };
  };
  const host = status.Self?.DNSName?.replace(/\.$/, '');
  if (status.BackendState !== 'Running' || !host) throw new Error('Tailscale is not running');
  const hostPort = `${host}:${frontendPort}`;
  const serveStatus = async () => parseServeStatus(await tailscale('serve', 'status', '--json'));

  const state = routeState(await serveStatus(), hostPort, frontendPort, target);
  if (state === 'funnel') throw new Error(`Funnel exposes ${hostPort} to the internet; turn it off first`);
  if (state === 'taken') throw new Error(`Tailscale already serves port ${frontendPort}; see 'tailscale serve status'`);
  const owned = state === 'free';
  if (owned) await tailscale('serve', '--bg', '--yes', `--https=${frontendPort}`, target);

  const token = randomBytes(24).toString('base64url');
  const env = {
    ...process.env,
    BTMUX_AUTH_TOKEN: token,
    BTMUX_CONSOLE_LOG: process.env.BTMUX_CONSOLE_LOG || 'debug',
    BTMUX_FILE_LOG: process.env.BTMUX_FILE_LOG || 'debug',
    __VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS: host,
  };
  const origins = [`http://localhost:${frontendPort}`, `http://127.0.0.1:${frontendPort}`, `https://${hostPort}`]
    .map((origin) => `--public-url ${origin}`)
    .join(' ');
  const stack = Bun.spawn(
    [
      'bunx',
      'concurrently',
      '--kill-others',
      '--success',
      'first',
      '--names',
      'backend,frontend',
      '--prefix-colors',
      'blue,green',
      `cargo run -- --no-browser --profile dev ${origins} --port ${backendPort}`,
      `bunx vite --host 127.0.0.1 --port ${frontendPort}`,
    ],
    { cwd: import.meta.dirname, env, stdio: ['ignore', 'inherit', 'inherit'] },
  );

  // Ctrl+C reaches the stack too; wait for it to stop, then clean up.
  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP'] as const) process.on(signal, () => stack.kill(signal));

  const printPairing = async () => {
    const response = await fetch(`http://127.0.0.1:${backendPort}/api/pair`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${token}` },
    });
    if (!response.ok) throw new Error(`pairing failed: ${response.status}`);
    const { code, expires_in } = (await response.json()) as { code: string; expires_in: number };
    const link = `https://${hostPort}/pair?code=${code}`;
    console.log(`\n${renderUnicodeCompact(link, { border: 2 })}`);
    console.log(`Scan or open to sign in (once, within ${Math.round(expires_in / 60)} min):\n${link}`);
    console.log('Press Enter for a new code.\n');
  };

  // The backend compiles first; Vite is up long before it.
  void (async () => {
    for (;;) {
      if (stack.exitCode !== null) return;
      try {
        const info = await fetch(`http://127.0.0.1:${backendPort}/api/info`, {
          headers: { Authorization: `Bearer ${token}` },
        });
        if (info.ok) break;
      } catch {}
      await Bun.sleep(500);
    }
    await printPairing().catch((error) => console.error(String(error)));
    for await (const _ of console) await printPairing().catch((error) => console.error(String(error)));
  })();

  const code = await stack.exited;
  if (owned) {
    // Remove the route only if it's still the one made here.
    const now = await serveStatus().catch(() => null);
    if (now && routeState(now, hostPort, frontendPort, target) === 'ours') {
      await tailscale('serve', `--https=${frontendPort}`, 'off').catch((error) => console.error(String(error)));
    }
  }
  process.exit(code);
}

if (import.meta.main) {
  main().catch((error) => {
    console.error(`error: ${error instanceof Error ? error.message : error}`);
    process.exit(1);
  });
}
