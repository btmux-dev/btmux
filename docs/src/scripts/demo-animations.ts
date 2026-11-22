// Recordings carry Web Animations (pane motion) as `animate` custom events,
// with `animate-cancel` when one is interrupted (see frontend/docs-demos.ts).
// This plays them on the replayed nodes: live as they are cast, and, after
// every start, seek, or pause, those spanning the replay's time, at their
// progress then, paused with the replay. Shared with frontend/readme-video.ts.

interface Span {
  key: number;
  id: number;
  keyframes: Keyframe[] | PropertyIndexedKeyframes;
  options: KeyframeAnimationOptions;
  /** Replay time at which the animation's own time is 0. */
  origin: number;
  /** Replay time at which it ends or is canceled. */
  end: number;
}

interface Replayer {
  config: { speed: number };
  getCurrentTime(): number;
  getMetaData(): { startTime: number };
  getMirror(): { getNode(id: number): Node | null };
  on(event: string, handler: (event?: any) => void): unknown;
}

export function replayAnimations(replayer: Replayer, events: any[]) {
  const start = replayer.getMetaData().startTime;
  const spans = new Map<number, Span>();
  for (const event of events) {
    if (event.type !== 5) continue;
    const { tag, payload } = event.data;
    const at = event.timestamp - start;
    if (tag === "animate") {
      const { duration = 0, delay = 0 } = payload.options ?? {};
      const origin = at - payload.elapsed;
      spans.set(payload.key, { ...payload, origin, end: origin + delay + duration });
    } else if (tag === "animate-cancel") {
      const span = spans.get(payload.key);
      if (span) span.end = Math.min(span.end, at);
    }
  }
  const running = new Map<number, Animation>();
  const stop = (key: number) => {
    running.get(key)?.cancel();
    running.delete(key);
  };
  const run = (span: Span, time: number, playing: boolean) => {
    const node = replayer.getMirror().getNode(span.id) as Element | null;
    if (typeof node?.animate !== "function") return;
    const animation = node.animate(span.keyframes, span.options);
    animation.playbackRate = replayer.config.speed;
    animation.currentTime = time - span.origin;
    if (!playing) animation.pause();
    animation.onfinish = () => running.get(span.key) === animation && running.delete(span.key);
    running.set(span.key, animation);
  };
  // Seeks cast earlier events before their nodes reach the DOM, so spanning
  // animations start once the replay has flushed it.
  const sync = (playing: boolean) => {
    const time = replayer.getCurrentTime();
    for (const key of [...running.keys()]) stop(key);
    for (const span of spans.values())
      if (span.origin <= time && time < span.end) run(span, time, playing);
  };
  replayer.on("custom-event", (event) => {
    const { tag, payload } = event.data;
    if (tag === "animate-cancel") stop(payload.key);
    const span = tag === "animate" && spans.get(payload.key);
    const time = replayer.getCurrentTime();
    if (span && !running.has(span.key) && time < span.end) run(span, time, true);
  });
  replayer.on("start", () => sync(true));
  replayer.on("pause", () => sync(false));
}
