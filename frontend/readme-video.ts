/**
 * Joins the docs' demo recordings (docs/public/demos, `just docs-demos`) into
 * one MP4 for the README at twice their speed: `just readme-video` writes
 * target/readme-video.mp4.
 * Each demo plays its dark and light recordings at once, stacked, and wipes
 * from one to the other partway through; demos crossfade into the next.
 *
 * It plays in real time, so CSS animations keep their speed, and captures with
 * the screencast; ffmpeg turns the frames into constant frame rate H.264.
 */
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { chromium } from 'playwright';

const root = path.dirname(import.meta.dir);
const demos = path.join(root, 'docs', 'public', 'demos');
const replay = path.join(root, 'docs', 'node_modules', '@rrweb', 'replay', 'dist');
const output = path.join(root, 'target', 'readme-video.mp4');
assert(existsSync(replay), 'run `cd docs && bun install` first');

// Demo and its first theme. The wipe to the other starts at the recording's
// `calm` mark, a still moment `docs-demos.ts` picks.
const segments = [
  { name: 'panes', from: 'dark' },
  { name: 'files', from: 'light' },
  { name: 'editor', from: 'dark' },
  { name: 'agents', from: 'light' },
  { name: 'latex', from: 'dark' },
];
for (const { name } of segments)
  for (const scheme of ['dark', 'light'])
    assert(existsSync(path.join(demos, `${name}-${scheme}.json.gz`)), `missing ${name}-${scheme}; run just docs-demos`);

const origin = 'http://readme-video.test';
const width = 1280;
const height = 800;
const scale = 2;
const fps = 30;
// Playback speed of the recordings; fades and wipes keep their own.
const speed = 2;

const pageHtml = `<!doctype html>
<html><head><meta charset="utf-8">
<link rel="stylesheet" href="/style.css">
<style>
  html, body { margin: 0; width: ${width}px; height: ${height}px; overflow: hidden; background: #000; }
  .segment, .stage { position: absolute; inset: 0; }
  .segment { opacity: 0; }
  .replayer-wrapper iframe { display: block; width: ${width}px; height: ${height}px; border: 0; }
  .segment:not(.mouse) .replayer-mouse { display: none; }
  .edge { position: absolute; top: 0; bottom: 0; width: 2px; margin-left: -1px; display: none;
    background: rgb(255 255 255 / 85%); box-shadow: 0 0 12px 2px rgb(120 160 255 / 60%); }
</style>
<script src="/replay.js"></script>
</head><body></body></html>`;

const browser = await chromium.launch({ headless: true });
const frames = mkdtempSync(path.join(tmpdir(), 'btmux-readme-video-'));
try {
  const context = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: scale });
  const page = await context.newPage();
  page.on('pageerror', (error) => console.error('page error:', error.message));
  await page.route(`${origin}/**`, async (route) => {
    const { pathname } = new URL(route.request().url());
    if (pathname === '/') return route.fulfill({ contentType: 'text/html', body: pageHtml });
    if (pathname === '/replay.js')
      return route.fulfill({ contentType: 'text/javascript', path: path.join(replay, 'replay.umd.min.cjs') });
    if (pathname === '/style.css')
      return route.fulfill({ contentType: 'text/css', path: path.join(replay, 'style.css') });
    const file = path.join(demos, path.basename(pathname));
    if (pathname.startsWith('/demos/') && existsSync(file)) return route.fulfill({ path: file });
    return route.fulfill({ status: 404 });
  });
  await page.goto(`${origin}/`);

  // Build every segment up front, paused at its start with its fonts loaded.
  const durations = await page.evaluate(
    async ({ segments, origin, speed }) => {
      const { Replayer } = (window as any).rrwebReplay;
      const load = async (file: string) => {
        const response = await fetch(`${origin}/demos/${file}`);
        const stream = response.body!.pipeThrough(new DecompressionStream('gzip'));
        const text = await new Response(stream).text();
        return JSON.parse(text.replaceAll('@demos@', `${origin}/demos`));
      };
      const built = [];
      for (const { name, from } of segments) {
        const to = from === 'dark' ? 'light' : 'dark';
        const segment = document.createElement('div');
        segment.className = 'segment';
        document.body.append(segment);
        const players = [];
        let wipeAt = 0;
        for (const scheme of [from, to]) {
          const events = await load(`${name}-${scheme}.json.gz`);
          if (scheme === from) {
            const mark = events.find((e: any) => e.type === 5 && e.data.tag === 'calm');
            if (!mark) throw new Error(`${name}-${scheme} has no calm mark; run just docs-demos`);
            wipeAt = mark.timestamp - events[0].timestamp;
          }
          if (events.some((e: any) => e.type === 3 && e.data.source === 1)) segment.classList.add('mouse');
          const stage = document.createElement('div');
          stage.className = 'stage';
          segment.append(stage);
          const replayer = new Replayer(events, {
            root: stage,
            mouseTail: false,
            pauseAnimation: false,
            triggerFocus: false,
            showWarning: false,
            speed,
          });
          replayer.pause(0);
          players.push({ replayer, stage });
        }
        players[1].stage.style.clipPath = 'inset(0 100% 0 0)';
        const edge = document.createElement('div');
        edge.className = 'edge';
        segment.append(edge);
        built.push({ segment, players, edge, wipeAt });
      }
      await Promise.all(built.flatMap((b) => b.players.map((p) => p.replayer.iframe.contentDocument?.fonts.ready)));
      await new Promise((resolve) => setTimeout(resolve, 500));
      (window as any).built = built;
      return built.map((b) => Math.max(...b.players.map((p) => p.replayer.getMetaData().totalTime)) / speed);
    },
    { segments, origin, speed },
  );
  console.log('segments (s):', durations.map((d) => (d / 1000).toFixed(1)).join(', '));

  // Capture.
  const cdp = await context.newCDPSession(page);
  const captured: { file: string; time: number }[] = [];
  const writes: Promise<void>[] = [];
  cdp.on('Page.screencastFrame', ({ data, metadata, sessionId }) => {
    void cdp.send('Page.screencastFrameAck', { sessionId }).catch(() => {});
    const file = path.join(frames, `${String(captured.length).padStart(6, '0')}.jpg`);
    captured.push({ file, time: metadata.timestamp ?? Date.now() / 1000 });
    writes.push(writeFile(file, Buffer.from(data, 'base64')));
  });
  await cdp.send('Page.startScreencast', {
    format: 'jpeg',
    quality: 92,
    maxWidth: width * scale,
    maxHeight: height * scale,
  });

  await page.evaluate(
    async ({ speed }) => {
      const built = (window as any).built as any[];
      const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
      const ease = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2);
      const animate = (ms: number, step: (t: number) => void) =>
        new Promise<void>((resolve) => {
          const start = performance.now();
          const frame = (now: number) => {
            const t = Math.min(1, (now - start) / ms);
            step(t);
            if (t < 1) requestAnimationFrame(frame);
            else resolve();
          };
          requestAnimationFrame(frame);
        });
      const FADE = 600;
      const WIPE = 1200;
      for (let i = 0; i < built.length; i++) {
        const { segment, players, edge } = built[i];
        const total = Math.max(...players.map((p: any) => p.replayer.getMetaData().totalTime)) / speed;
        for (const p of players) p.replayer.play(0);
        segment.style.zIndex = String(i + 1);
        await animate(i === 0 ? 1 : FADE, (t) => (segment.style.opacity = String(t)));
        // Hide the previous segment once covered.
        if (i > 0) built[i - 1].segment.style.visibility = 'hidden';
        // Just after the mark, once the last key's effect has settled.
        const wipeAt = (built[i].wipeAt + 300) / speed;
        await sleep(Math.max(0, wipeAt - (i === 0 ? 0 : FADE)));
        edge.style.display = 'block';
        await animate(WIPE, (t) => {
          const p = ease(t) * 100;
          players[1].stage.style.clipPath = `inset(0 ${100 - p}% 0 0)`;
          edge.style.left = `${p}%`;
        });
        edge.style.display = 'none';
        players[0].stage.style.visibility = 'hidden';
        await sleep(Math.max(0, total - wipeAt - WIPE) + 800);
      }
      await sleep(1200);
    },
    { speed },
  );
  await cdp.send('Page.stopScreencast');
  await Promise.all(writes);
  assert(captured.length > 0, 'no frames captured');

  // Each frame lasts until the next; the last one a frame.
  const list = captured
    .map(({ file, time }, i) => {
      const next = captured[i + 1]?.time ?? time + 1 / fps;
      return `file '${file}'\nduration ${(next - time).toFixed(6)}`;
    })
    .join('\n');
  writeFileSync(path.join(frames, 'frames.txt'), `${list}\nfile '${captured[captured.length - 1].file}'\n`);
  mkdirSync(path.dirname(output), { recursive: true });
  const ffmpeg = spawnSync(
    'ffmpeg',
    [
      '-y',
      '-loglevel',
      'error',
      '-f',
      'concat',
      '-safe',
      '0',
      '-i',
      path.join(frames, 'frames.txt'),
      '-vf',
      `fps=${fps},scale=${(width * 3) / 2}:${(height * 3) / 2}:flags=lanczos,format=yuv420p`,
      '-c:v',
      'libx264',
      '-preset',
      'slow',
      '-crf',
      '22',
      '-tune',
      'animation',
      '-movflags',
      '+faststart',
      output,
    ],
    { stdio: 'inherit' },
  );
  assert.equal(ffmpeg.status, 0, 'ffmpeg failed');
  const seconds = captured[captured.length - 1].time - captured[0].time;
  const megabytes = readFileSync(output).length / 1e6;
  console.log(`${output}: ${captured.length} frames over ${seconds.toFixed(1)} s, ${megabytes.toFixed(1)} MB`);
} finally {
  await browser.close();
  rmSync(frames, { recursive: true, force: true });
}
