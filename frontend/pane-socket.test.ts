import { expect, test } from 'bun:test';
import { constants, createDeflateRaw } from 'node:zlib';
import { paneMessageHandler, PaneInflater } from './src/lib/paneSocket';

/** Frames as `connect::PaneDeflater` writes them: u32 length, then one sync-flushed stream. */
function deflater() {
  const stream = createDeflateRaw({ level: 6 });
  const chunks: Buffer[] = [];
  stream.on('data', (chunk: Buffer) => chunks.push(chunk));
  return async (data: Uint8Array): Promise<ArrayBuffer> => {
    chunks.length = 0;
    stream.write(data);
    await new Promise<void>((resolve) => stream.flush(constants.Z_SYNC_FLUSH, () => resolve()));
    await new Promise((resolve) => setImmediate(resolve));
    const body = Buffer.concat(chunks);
    const frame = new Uint8Array(4 + body.length);
    new DataView(frame.buffer).setUint32(0, data.length);
    frame.set(body, 4);
    return frame.buffer;
  };
}

const encoder = new TextEncoder();
const prompt = encoder.encode('\x1b[32m~/repos/btmux\x1b[0m main ❯ ');
const big = Uint8Array.from({ length: 300_000 }, (_, i) => (i * 7919) % 251);

test('inflates each frame from one shared stream', async () => {
  const frame = deflater();
  const inflater = new PaneInflater();
  for (const message of [prompt, prompt, big, encoder.encode('x'), prompt]) {
    const out = await inflater.inflate(new Uint8Array(await frame(message)));
    expect(out).toEqual(message);
  }
  inflater.dispose();
});

test('keeps text frames in order with compressed output', async () => {
  const frame = deflater();
  const seen: string[] = [];
  const handler = paneMessageHandler({
    binary: (bytes) => seen.push(`binary ${bytes.length}`),
    text: (data) => seen.push(JSON.parse(data).type),
  });
  const send = (data: unknown) => handler.onMessage({ data } as MessageEvent);
  send('{"type":"owner","owner":true}');
  expect(seen).toEqual(['owner']); // synchronous until compression is announced

  send('{"type":"compression","format":"deflate-raw"}');
  send(await frame(big));
  send('{"type":"size","cols":80,"rows":24}');
  send(await frame(prompt));
  send('{"type":"ready"}');
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(seen).toEqual(['owner', `binary ${big.length}`, 'size', `binary ${prompt.length}`, 'ready']);
  handler.dispose();
});
