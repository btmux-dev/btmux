/**
 * Message handling for `/ws/pane` sockets. A relayed socket (btmux connect)
 * announces compressed output in its first frame: each binary frame is then a
 * big-endian u32 uncompressed length followed by the next piece of one raw
 * deflate stream, sync-flushed (`connect::PaneDeflater`). Inflating is async,
 * so from that frame on every message waits its turn, keeping `size` and
 * `ready` frames in order with the output around them. Other sockets are
 * handled synchronously, as before.
 */
export function paneMessageHandler(handlers: { binary: (bytes: Uint8Array) => void; text: (data: string) => void }): {
  onMessage: (ev: MessageEvent) => void;
  dispose: () => void;
} {
  let inflater: PaneInflater | null = null;
  let queue = Promise.resolve();
  let disposed = false;
  const handle = (data: unknown) => {
    if (data instanceof ArrayBuffer) handlers.binary(new Uint8Array(data));
    else if (typeof data === 'string') handlers.text(data);
  };
  return {
    onMessage: (ev) => {
      if (disposed) return;
      if (!inflater) {
        if (typeof ev.data === 'string' && isCompressionFrame(ev.data)) inflater = new PaneInflater();
        else handle(ev.data);
        return;
      }
      const active = inflater;
      queue = queue.then(async () => {
        if (disposed) return;
        if (ev.data instanceof ArrayBuffer) {
          const bytes = await active.inflate(new Uint8Array(ev.data));
          if (!disposed) handlers.binary(bytes);
        } else {
          handle(ev.data);
        }
      });
      // A corrupt stream can't recover; the socket's reconnect starts a new one.
      queue = queue.catch((error) => {
        console.warn('btmux: pane output failed to inflate', error);
        disposed = true;
      });
    },
    dispose: () => {
      disposed = true;
      inflater?.dispose();
    },
  };
}

function isCompressionFrame(data: string): boolean {
  if (!data.includes('"compression"')) return false;
  try {
    const msg = JSON.parse(data);
    return msg.type === 'compression' && msg.format === 'deflate-raw';
  } catch {
    return false;
  }
}

/** One raw deflate stream; each frame's output is complete after its sync flush. */
export class PaneInflater {
  private readonly writer: WritableStreamDefaultWriter<BufferSource>;
  private readonly reader: ReadableStreamDefaultReader<Uint8Array>;
  private buffered: Uint8Array[] = [];
  private bufferedBytes = 0;

  constructor() {
    const stream = new DecompressionStream('deflate-raw');
    this.writer = stream.writable.getWriter();
    this.reader = stream.readable.getReader();
  }

  async inflate(frame: Uint8Array<ArrayBuffer>): Promise<Uint8Array> {
    if (frame.length < 4) throw new Error('short frame');
    const size = new DataView(frame.buffer, frame.byteOffset, 4).getUint32(0);
    // Not awaited: the write resolves only as output is read, below.
    this.writer.write(frame.subarray(4)).catch(() => {});
    while (this.bufferedBytes < size) {
      const { value, done } = await this.reader.read();
      if (done) throw new Error('deflate stream ended');
      this.buffered.push(value);
      this.bufferedBytes += value.length;
    }
    return this.take(size);
  }

  private take(size: number): Uint8Array {
    const first = this.buffered[0];
    if (first && first.length === size) {
      this.buffered.shift();
      this.bufferedBytes -= size;
      return first;
    }
    const out = new Uint8Array(size);
    let offset = 0;
    while (offset < size) {
      const chunk = this.buffered[0];
      const n = Math.min(chunk.length, size - offset);
      out.set(chunk.subarray(0, n), offset);
      offset += n;
      if (n === chunk.length) this.buffered.shift();
      else this.buffered[0] = chunk.subarray(n);
    }
    this.bufferedBytes -= size;
    return out;
  }

  dispose(): void {
    this.reader.cancel().catch(() => {});
    this.writer.abort().catch(() => {});
  }
}
