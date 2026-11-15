// btmux connect sign-up and passkey sign-in, end to end in Chromium with a
// virtual authenticator. Starts its own backend and a fake sign-up service,
// which registers `localhost` (no relay; the hidden --connect-listen listener
// stands in for the tunnel).
import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { chromium, type CDPSession, type Page } from 'playwright';

const root = path.dirname(import.meta.dir);
const binary = path.resolve(root, process.env.CARGO_TARGET_DIR ?? 'target', 'debug', 'btmux');

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      server.close(() => (typeof address === 'object' && address ? resolve(address.port) : reject(new Error('no port'))));
    });
  });
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const dir = mkdtempSync(path.join(tmpdir(), 'btmux-passkey-test-'));
const env: NodeJS.ProcessEnv = { ...process.env };
for (const key of Object.keys(env)) if (key.startsWith('BTMUX_')) delete env[key];
for (const name of ['CONFIG', 'STATE', 'DATA', 'CACHE']) {
  const home = path.join(dir, name.toLowerCase());
  mkdirSync(home, { recursive: true });
  env[`XDG_${name}_HOME`] = home;
}
const token = randomBytes(18).toString('base64url');
const [port, relayedPort] = [await freePort(), await freePort()];
const origin = `http://localhost:${relayedPort}`;

function cli(...args: string[]) {
  const result = spawnSync(binary, args, { env: { ...env, BTMUX_CONNECT_TOKEN: 'unused' }, encoding: 'utf8' });
  assert(result.status === 0, `btmux ${args.join(' ')}: ${result.stderr}`);
  return result.stdout.trim();
}

// The sign-up service, with one invite. No relay is reached: the relayed
// listener below takes its place. Its CA is a certificate of no use.
const relayCa = spawnSync(
  'openssl',
  ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', '/dev/null', '-subj', '/CN=relay.test', '-days', '1', '-outform', 'DER'],
  { encoding: 'buffer' },
);
assert(relayCa.status === 0, `openssl: ${relayCa.stderr}`);
const invites = new Set(['btmi_test']);
const service = Bun.serve({
  hostname: '127.0.0.1',
  port: 0,
  async fetch(request) {
    const { invite } = (await request.json()) as { invite: string };
    if (!invites.delete(invite)) return Response.json({ error: "This invite code isn't valid." }, { status: 403 });
    return Response.json(
      {
        hostname: 'localhost',
        token: 'btmr_test',
        relay: '127.0.0.1:9',
        relay_name: 'relay.test',
        relay_ca: relayCa.stdout.toString('base64'),
        public_url: origin,
      },
      { status: 201 },
    );
  },
});

let backend: ChildProcess | undefined;
const browser = await chromium.launch();
try {
  backend = spawn(
    binary,
    ['--no-browser', '--host', '127.0.0.1', '--port', String(port), '--connect-listen', `127.0.0.1:${relayedPort}`, '--shell', '/bin/sh'],
    {
      env: {
        ...env,
        BTMUX_AUTH_TOKEN: token,
        BTMUX_CONNECT_SERVICE: `http://127.0.0.1:${service.port}`,
        BTMUX_CONNECT_ACME: 'none',
      },
      stdio: ['ignore', 'ignore', 'inherit'],
    },
  );
  for (let i = 0; ; i++) {
    const ready = await fetch(`http://127.0.0.1:${port}/`).then(
      () => true,
      () => false,
    );
    if (ready) break;
    assert(i < 100, 'the backend did not start');
    await Bun.sleep(100);
  }

  const context = await browser.newContext();
  const page = await context.newPage();
  const cdp = await context.newCDPSession(page);
  await cdp.send('WebAuthn.enable');
  // A new authenticator stands for another device: one passkey each.
  const newDevice = async (session: CDPSession = cdp) => {
    const { authenticatorId } = await session.send('WebAuthn.addVirtualAuthenticator', {
      options: {
        protocol: 'ctap2',
        transport: 'internal',
        hasResidentKey: true,
        hasUserVerification: true,
        isUserVerified: true,
        automaticPresenceSimulation: true,
      },
    });
    return authenticatorId;
  };
  let device = await newDevice();
  const signIn = (page: Page) => page.getByRole('button', { name: 'Sign in with a passkey' });
  const enroll = async (page: Page) => {
    await page.goto(cli('connect-enroll'));
    await page.getByRole('button', { name: 'Create a passkey' }).click();
    await page.waitForURL(`${origin}/`);
  };
  /** What the pane shows, read on the machine rather than through the relay. */
  const paneText = () => cli('capture-pane', '-t', '0', '-S', '-');

  // Sign up from Settings on the machine itself, where the token works.
  const localOrigin = `http://127.0.0.1:${port}`;
  const localContext = await browser.newContext({
    // The browser's own prompt: any user name, the token as the password.
    httpCredentials: { username: 'btmux', password: token, origin: localOrigin },
  });
  const local = await localContext.newPage();
  await local.goto(`${localOrigin}/`);
  await local.getByRole('button', { name: 'Settings', exact: true }).first().click();
  await local.getByRole('tab', { name: 'Connect', exact: true }).click();
  const setup = local.getByRole('tabpanel', { name: 'Connect' });
  await setup.getByLabel('Invite code').fill('btmi_wrong');
  await setup.getByLabel(/Subscriber Agreement/).check();
  await setup.getByRole('button', { name: 'Connect', exact: true }).click();
  await setup.getByText("This invite code isn't valid.").waitFor();
  await setup.getByLabel('Invite code').fill('btmi_test');
  await setup.getByRole('button', { name: 'Connect', exact: true }).click();
  // Connected without a restart: the status replaces the form.
  await setup.getByText(origin, { exact: true }).waitFor();
  const again = spawnSync(binary, ['connect', 'btmi_other', '--accept-acme-terms'], { env, encoding: 'utf8' });
  assert(again.status !== 0 && again.stderr.includes('already connected'), `a second sign-up: ${again.stderr}`);
  await localContext.close();

  // Before any passkey: a page load gets the sign-in page, which points to enrolling instead.
  const first = await page.goto(`${origin}/`);
  assert(first?.status() === 401, `expected the sign-in page, got ${first?.status()}`);
  await page.getByText('No passkey is enrolled on this machine yet.', { exact: false }).waitFor();
  assert((await signIn(page).count()) === 0, 'the sign-in page offered a sign-in with no passkey');

  // The token never works over the relay, in any form.
  const bearer = await fetch(`${origin}/api/sessions`, { headers: { Authorization: `Bearer ${token}` } });
  assert(bearer.status === 401, `Bearer over the relay: ${bearer.status}`);
  assert(bearer.headers.get('x-btmux-login') === 'passkey', 'the 401 should ask for a passkey');

  // Enroll through a one-time link; the app opens signed in.
  const link = cli('connect-enroll');
  assert(link.startsWith(`${origin}/connect/enroll#code=`), `unexpected link ${link}`);
  await page.goto(link);
  assert(!page.url().includes('code='), 'the code should leave the address bar');
  await page.getByRole('button', { name: 'Create a passkey' }).click();
  await page.waitForURL(`${origin}/`);
  await page.getByLabel('Terminal').first().waitFor({ state: 'attached', timeout: 15_000 });
  await page.locator('.wterm').first().click();
  await page.keyboard.type('echo relayed-$((40 + 2))\n');
  await page.waitForFunction(() => document.body.innerText.includes('relayed-42'));
  const cookies = await context.cookies();
  assert(
    cookies.some((c) => c.name === '__Host-btmux_session' && c.httpOnly && c.secure && c.sameSite === 'Strict'),
    'expected a Strict, HttpOnly, Secure __Host- session cookie',
  );

  // Settings → Connect: this machine's status, and its passkeys managed from the app.
  await page.getByRole('button', { name: 'Settings', exact: true }).first().click();
  const settings = page.getByRole('dialog', { name: 'Settings' });
  await settings.getByRole('tab', { name: 'Connect' }).click();
  const panel = settings.getByRole('tabpanel', { name: 'Connect' });
  await panel.getByText(origin, { exact: true }).waitFor();
  // No relay listens, so the tunnel keeps trying.
  await panel.getByText(/^(Connecting to relay\.test…|Can't reach relay\.test; retrying)$/).waitFor();
  await panel.getByText('Self-signed').waitFor();
  const listed = panel.getByRole('listitem');
  const awaitListed = async (count: number, when: string) => {
    for (let i = 0; (await listed.count()) !== count; i++) {
      assert(i < 100, `${when}: ${await listed.count()} passkeys listed, not ${count}`);
      await Bun.sleep(100);
    }
  };
  await awaitListed(1, 'after enrolling');
  // A link from the app enrolls another device, here a browser of its own.
  await panel.getByRole('button', { name: 'Add a passkey' }).click();
  const fromApp = (await panel.getByText('/connect/enroll#code=').innerText()).trim();
  assert(fromApp.startsWith(`${origin}/connect/enroll#code=`), `unexpected link ${fromApp}`);
  const other = await browser.newContext();
  const otherPage = await other.newPage();
  const otherCdp = await other.newCDPSession(otherPage);
  await otherCdp.send('WebAuthn.enable');
  await newDevice(otherCdp);
  await otherPage.goto(fromApp);
  await otherPage.getByRole('button', { name: 'Create a passkey' }).click();
  await otherPage.waitForURL(`${origin}/`);
  // Pushed to the open Settings, which drops the used link.
  await awaitListed(2, 'after enrolling from the app');
  assert(!(await panel.getByText('/connect/enroll#code=').isVisible()), 'the used link stayed up');
  // Removing the new passkey signs that browser out, not this one.
  await listed.nth(1).getByRole('button', { name: /^Remove / }).click();
  await page.getByRole('alertdialog').getByRole('button', { name: 'Remove passkey' }).click();
  await awaitListed(1, 'after removing');
  const removedStatus = await otherPage.evaluate(() => fetch('/api/sessions', { cache: 'no-store' }).then((r) => r.status));
  assert(removedStatus === 401, `a removed passkey's browser answered ${removedStatus}`);
  await other.close();
  // A stale sign-in is asked to sign in again (the server's answer, stubbed).
  const signOutRoute = '**/api/connect/sign-out-everywhere';
  await page.route(signOutRoute, (route) =>
    route.fulfill({ status: 403, body: 'Sign in again at /connect/login?reauth, then try again.' }),
  );
  const signOutEverywhere = async () => {
    await panel.getByRole('button', { name: 'Sign out everywhere' }).click();
    await page.getByRole('alertdialog').getByRole('button', { name: 'Sign out everywhere' }).click();
  };
  await signOutEverywhere();
  await panel.getByText('Sign in again to change passkeys').waitFor();
  await page.unroute(signOutRoute);
  // Signing out everywhere from the app ends this browser's session too.
  await signOutEverywhere();
  await signIn(page).waitFor({ timeout: 10_000 });
  const signedOut = await page.evaluate(() => fetch('/api/sessions', { cache: 'no-store' }).then((r) => r.status));
  assert(signedOut === 401, `signed out everywhere, a request answered ${signedOut}`);
  await signIn(page).click();
  await page.getByLabel('Terminal').first().waitFor({ state: 'attached', timeout: 15_000 });

  // The link worked once.
  const code = new URL(link).hash.slice('#code='.length);
  const reuse = await page.evaluate(
    (code) =>
      fetch('/connect/passkeys/register/start', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ code }),
      }).then((r) => r.status),
    code,
  );
  assert(reuse === 401, `a used enrollment code answered ${reuse}`);

  // The sign-in page's own address, signed in: home, not a reload loop.
  await page.goto(`${origin}/connect/login`);
  await page.waitForURL(`${origin}/`);
  await page.getByLabel('Terminal').first().waitFor({ state: 'attached', timeout: 15_000 });
  // `?reauth` signs in again despite the session, as relayed changes require.
  await page.goto(`${origin}/connect/login?reauth`);
  await signIn(page).click();
  await page.waitForURL(`${origin}/`);

  // Signed out, a deep link shows the sign-in page, then opens what it asked for.
  await context.clearCookies();
  const deep = `${origin}/s/0`;
  await page.goto(deep);
  await signIn(page).click();
  await page.getByLabel('Terminal').first().waitFor({ state: 'attached', timeout: 15_000 });
  // The app then adds the active window to the address.
  assert(page.url().startsWith(`${deep}/`), `signed in at ${page.url()}, not under ${deep}`);

  // Ending a session cuts the terminal it has open: what's typed after never runs.
  const noPasskey = (page: Page) => page.getByText('No passkey is enrolled on this machine yet.', { exact: false });
  const revoked = async (revoke: () => void, marker: string, landing = signIn) => {
    await page.locator('.wterm').first().click();
    revoke();
    await page.keyboard.type(`echo ${marker}-$((40 + 2))\n`);
    await landing(page).waitFor({ timeout: 10_000 });
    assert(!paneText().includes(`${marker}-42`), `${marker}: a revoked session's terminal ran a command`);
    const status = await page.evaluate(() => fetch('/api/sessions', { cache: 'no-store' }).then((r) => r.status));
    assert(status === 401, `${marker}: a revoked session's request answered ${status}`);
  };
  await revoked(() => cli('connect-passkeys', '--sign-out-everywhere'), 'after-sign-out');
  await signIn(page).click();
  await page.getByLabel('Terminal').first().waitFor({ state: 'attached', timeout: 15_000 });
  const [passkey] = JSON.parse(cli('connect-passkeys')) as { id: string }[];
  assert(passkey, 'expected one enrolled passkey');
  // IDs are base64url and may start with `-`.
  // The last one: the sign-in page then points to enrolling.
  await revoked(() => cli('connect-passkeys', '--remove', passkey.id), 'after-removal', noPasskey);

  // Up to ten passkeys, and signing in with the last still fits the request limit.
  for (let enrolled = 0; enrolled < 10; enrolled++) {
    await cdp.send('WebAuthn.removeVirtualAuthenticator', { authenticatorId: device });
    device = await newDevice();
    await enroll(page);
  }
  assert(JSON.parse(cli('connect-passkeys')).length === 10, 'expected ten passkeys');
  const eleventh = new URL(cli('connect-enroll')).hash.slice('#code='.length);
  const refused = await page.evaluate(
    (code) =>
      fetch('/connect/passkeys/register/start', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ code }),
      }).then((r) => r.status),
    eleventh,
  );
  assert(refused === 409, `an eleventh passkey answered ${refused}`);
  await context.clearCookies();
  await page.goto(`${origin}/`);
  await signIn(page).click();
  await page.getByLabel('Terminal').first().waitFor({ state: 'attached', timeout: 15_000 });

  console.log(
    'PASS sign-up from Settings, passkey enrollment, sign-in, Settings management, reauth, single-use links, token refusal, revocation of live terminals, and the passkey cap over the relay',
  );
} catch (error) {
  console.error(`FAIL ${error instanceof Error ? error.stack : error}`);
  process.exitCode = 1;
} finally {
  await browser.close();
  service.stop(true);
  backend?.kill('SIGTERM');
  if (!process.exitCode) rmSync(dir, { recursive: true, force: true });
  else console.error(`logs and state in ${dir}`);
}
