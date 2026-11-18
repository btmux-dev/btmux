/**
 * The demo playground shared by the docs' screenshots (docs-screenshots.ts)
 * and demos (docs-demos.ts): an `acme-api` repository with history and work
 * in progress, a `router` crate, and a `website` folder, under a fresh
 * directory with a short path, since the UI shows it.
 */
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';

export function createPlayground() {
  const temp = process.platform === 'win32' ? tmpdir() : '/tmp';
  const home = mkdtempSync(path.join(temp, 'btmux-demo-'));
  process.once('exit', () => rmSync(home, { recursive: true, force: true }));

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
    `import { health } from './routes/health';\nimport { users } from './routes/users';\n\nconst routes = [health, users];\n\nexport function handle(request: Request): Response {\n  const { pathname } = new URL(request.url);\n  for (const route of routes) {\n    const match = route.pattern.exec(pathname);\n    if (match) return route.handler(request, match);\n  }\n  return new Response('Not found', { status: 404 });\n}\n\nBun.serve({ port: 3000, fetch: handle });\n`,
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
    `import { health } from './routes/health';\nimport { orders } from './routes/orders';\nimport { users } from './routes/users';\n\nconst routes = [health, users, orders];\n\nexport function handle(request: Request): Response {\n  const { pathname } = new URL(request.url);\n  for (const route of routes) {\n    const match = route.pattern.exec(pathname);\n    if (match) return route.handler(request, match);\n  }\n  return Response.json({ error: 'not found' }, { status: 404 });\n}\n\nBun.serve({ port: Number(process.env.PORT ?? 3000), fetch: handle });\n`,
  );
  git('add', 'src/server.ts');
  write(
    'README.md',
    '# acme-api\n\nThe Acme HTTP API.\n\n## Develop\n\n```sh\nbun install\nbun dev\n```\n\nSet `PORT` to listen elsewhere.\n\n## Routes\n\n- `GET /health`\n- `GET /users/:id`\n- `POST /orders`\n',
  );
  write('tests/orders.test.ts', `import { expect, test } from 'bun:test';\n\ntest.todo('creates an order');\n`);
  // A small Rust server: the program the server window runs (a listening port
  // for the process viewer) and the file the editor shots and demo edit.
  const router = path.join(home, 'router');
  mkdirSync(path.join(router, 'src'), { recursive: true });
  writeFileSync(path.join(router, 'Cargo.toml'), '[package]\nname = "router"\nversion = "0.1.0"\nedition = "2024"\n');
  writeFileSync(
    path.join(router, 'src', 'main.rs'),
    String.raw`use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

/// A parsed HTTP request.
struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
}

impl Request {
    /// Reads the request line and headers from a connection.
    fn read(stream: &TcpStream) -> Option<Request> {
        let mut lines = BufReader::new(stream).lines();
        let first = lines.next()?.ok()?;
        let mut parts = first.split_whitespace();
        let method = parts.next()?.to_string();
        let path = parts.next()?.to_string();
        let mut headers = HashMap::new();
        for line in lines {
            let line = line.ok()?;
            let Some((name, value)) = line.split_once(": ") else {
                break;
            };
            headers.insert(name.to_lowercase(), value.to_string());
        }
        Some(Request { method, path, headers })
    }

    /// Returns the value of a header.
    ///
    /// Names are matched in lowercase.\
    /// The last value wins if repeated.
    ///
    /// ${'```'}
    /// request.header("host")
    /// ${'```'}
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

/// The status and body to answer a request with.
fn respond(request: &Request) -> (u16, String) {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => (200, "ok".to_string()),
        ("GET", "/agent") => {
            let agent = request.header("user-agent").unwrap_or("unknown");
            (200, agent.to_string())
        }
        _ => (404, "not found".to_string()),
    }
}

fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:8089")?;
    for stream in listener.incoming() {
        let mut stream = stream?;
        let Some(request) = Request::read(&stream) else {
            continue;
        };
        let (status, body) = respond(&request);
        write!(stream, "HTTP/1.1 {status} OK\r\ncontent-length: {}\r\n\r\n{body}", body.len())?;
    }
    Ok(())
}
`,
  );
  // More files for quick open to find, and a repository, so the native editor
  // shows Git signs for its edits.
  for (const [file, text] of [
    ['src/headers.rs', '//! Header names and parsing.\n'],
    ['src/health.rs', '//! The health check.\n'],
    ['tests/health.rs', '//! GET /health answers 200.\n'],
    ['README.md', '# router\n\nA small HTTP server.\n'],
  ]) {
    mkdirSync(path.dirname(path.join(router, file)), { recursive: true });
    writeFileSync(path.join(router, file), text);
  }
  writeFileSync(path.join(router, '.gitignore'), 'target\nCargo.lock\n');
  for (const args of [
    ['init', '--initial-branch=main'],
    ['add', '.'],
    ['commit', '-m', 'Add the router'],
  ])
    execFileSync('git', ['-c', 'user.name=Ada', '-c', 'user.email=ada@example.invalid', ...args], {
      cwd: router,
      env: { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1' },
    });

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
  return { home, repo, router, site };
}

/** The btmux CLI against the stack's server, found through its state directory. */
export function stackCli(root: string, configHome: string, token: string) {
  return (...args: string[]) =>
    execFileSync(path.join(root, 'target', 'debug', 'btmux'), args, {
      encoding: 'utf8',
      env: {
        // Not this terminal's own server, if it runs in a btmux pane.
        ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('BTMUX_'))),
        XDG_STATE_HOME: path.join(path.dirname(configHome), 'state'),
        BTMUX_AUTH_TOKEN: token,
      },
    }).trim();
}
