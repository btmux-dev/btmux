// <btmux-demo name="panes" label="…">: plays the rrweb recording
// public/demos/<name>-<theme>.json.gz (made with `just docs-demos`) in the
// site's theme, falling back to the other theme's, and hides itself if
// neither exists. It loads once near the viewport, loops, pauses out of view,
// and doesn't autoplay for reduced motion.
import "@rrweb/replay/dist/style.css";

const demos = `${new URL(`${import.meta.env.BASE_URL.replace(/\/?$/, "/")}demos`, location.origin)}`;
const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");

type Replayer = import("@rrweb/replay").Replayer;

const theme = () =>
  document.documentElement.dataset.theme === "light" ? "light" : "dark";

async function fetchRecording(
  name: string,
  scheme: string,
): Promise<any[] | null> {
  const response = await fetch(`${demos}/${name}-${scheme}.json.gz`);
  if (!response.ok) return null;
  let bytes = new Uint8Array(await response.arrayBuffer());
  // Servers that send Content-Encoding have already inflated it.
  if (bytes[0] === 0x1f && bytes[1] === 0x8b) {
    const stream = new Blob([bytes])
      .stream()
      .pipeThrough(new DecompressionStream("gzip"));
    bytes = new Uint8Array(await new Response(stream).arrayBuffer());
  }
  return JSON.parse(
    new TextDecoder().decode(bytes).replaceAll("@demos@", demos),
  );
}

const format = (ms: number) => {
  const s = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};

class BtmuxDemo extends HTMLElement {
  private replayer: Replayer | null = null;
  private scheme = "";
  private visible = false;
  private playing = false;
  private restart = 0;
  private frame!: HTMLDivElement;
  private stage!: HTMLDivElement;
  private button!: HTMLButtonElement;
  private seek!: HTMLInputElement;
  private time!: HTMLSpanElement;
  private observers: { disconnect(): void }[] = [];

  connectedCallback() {
    const label =
      this.getAttribute("label") ?? `Demo: ${this.getAttribute("name")}`;
    // Starlight spaces content's descendants apart, except in .not-content.
    this.classList.add("not-content");
    this.setAttribute("role", "group");
    this.setAttribute("aria-label", label);
    this.innerHTML = `
      <div class="demo-frame"><div class="demo-stage" inert aria-hidden="true"></div></div>
      <div class="demo-controls">
        <button type="button" class="demo-play" aria-label="Play"></button>
        <input type="range" class="demo-seek" min="0" max="0" value="0" step="100" aria-label="Position" />
        <span class="demo-time">0:00</span>
      </div>`;
    this.frame = this.querySelector(".demo-frame")!;
    this.stage = this.querySelector(".demo-stage")!;
    this.button = this.querySelector(".demo-play")!;
    this.seek = this.querySelector(".demo-seek")!;
    this.time = this.querySelector(".demo-time")!;
    this.button.addEventListener("click", () =>
      this.playing ? this.pause() : this.play(),
    );
    this.seek.addEventListener("input", () => {
      clearTimeout(this.restart);
      this.replayer?.pause(Number(this.seek.value));
      this.showTime();
    });
    this.seek.addEventListener(
      "change",
      () => this.playing && this.replayer?.play(Number(this.seek.value)),
    );

    const intersection = new IntersectionObserver(
      ([entry]) => {
        this.visible = entry.isIntersecting;
        if (this.visible && !this.replayer) void this.load();
        else if (!this.visible && this.playing) this.replayer?.pause();
        else if (this.visible && this.playing)
          this.replayer?.play(this.replayer.getCurrentTime());
      },
      { rootMargin: "200px" },
    );
    intersection.observe(this);
    const resize = new ResizeObserver(() => this.fit());
    resize.observe(this.frame);
    const themeChange = new MutationObserver(() => {
      if (this.replayer && theme() !== this.scheme)
        void this.load(this.replayer.getCurrentTime());
    });
    themeChange.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    this.observers = [intersection, resize, themeChange];
  }

  disconnectedCallback() {
    for (const observer of this.observers) observer.disconnect();
    clearTimeout(this.restart);
    this.replayer?.destroy();
    this.replayer = null;
  }

  private async load(at = 0) {
    const name = this.getAttribute("name")!;
    const wanted = theme();
    const other = wanted === "dark" ? "light" : "dark";
    let scheme = wanted;
    let events = await fetchRecording(name, wanted).catch(() => null);
    if (!events) {
      scheme = other;
      events = await fetchRecording(name, other).catch(() => null);
    }
    if (!events) {
      this.hidden = true;
      return;
    }
    const { Replayer } = await import("@rrweb/replay");
    this.replayer?.destroy();
    clearTimeout(this.restart);
    this.stage.replaceChildren();
    this.scheme = scheme;
    const meta = events.find((event) => event.type === 4)?.data ?? {
      width: 1024,
      height: 640,
    };
    this.style.setProperty("--demo-width", String(meta.width));
    this.style.setProperty("--demo-height", String(meta.height));
    // Recordings without mouse moves hide the replayed pointer.
    this.classList.toggle(
      "demo-mouse",
      events.some((event) => event.type === 3 && event.data.source === 1),
    );
    const replayer = new Replayer(events, {
      root: this.stage,
      mouseTail: false,
      pauseAnimation: false,
      // Focusing replayed elements would scroll the page to them.
      triggerFocus: false,
      showWarning: false,
    });
    this.replayer = replayer;
    const total = replayer.getMetaData().totalTime;
    this.seek.max = String(total);
    replayer.on("finish", () => {
      if (!this.playing) return;
      this.restart = window.setTimeout(
        () => this.playing && this.visible && replayer.play(0),
        2000,
      );
    });
    this.fit();
    if (this.playing || (at === 0 && !reducedMotion.matches))
      this.play(Math.min(at, total));
    else replayer.pause(Math.min(at, total));
    this.showTime();
  }

  private play(at = this.replayer?.getCurrentTime() ?? 0) {
    if (!this.replayer) return;
    const total = this.replayer.getMetaData().totalTime;
    this.playing = true;
    this.replayer.play(at >= total ? 0 : at);
    this.button.setAttribute("aria-label", "Pause");
    this.classList.add("demo-playing");
    this.tick();
  }

  private pause() {
    this.playing = false;
    clearTimeout(this.restart);
    this.replayer?.pause();
    this.button.setAttribute("aria-label", "Play");
    this.classList.remove("demo-playing");
  }

  private tick() {
    if (!this.playing || !this.replayer) return;
    this.showTime();
    requestAnimationFrame(() => this.tick());
  }

  private showTime() {
    if (!this.replayer) return;
    const now = Math.min(
      this.replayer.getCurrentTime(),
      this.replayer.getMetaData().totalTime,
    );
    this.seek.value = String(now);
    this.time.textContent = format(now);
  }

  // The recording's viewport, scaled to the frame's width.
  private fit() {
    const width = Number(this.style.getPropertyValue("--demo-width")) || 1024;
    this.style.setProperty(
      "--demo-scale",
      String(this.frame.clientWidth / width),
    );
  }
}

customElements.define("btmux-demo", BtmuxDemo);
