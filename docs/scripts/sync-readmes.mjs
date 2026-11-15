// Generates the home and desktop pages from README.md and desktop/README.md.
import { existsSync } from "node:fs";
import { readFile, writeFile } from "node:fs/promises";
import { dirname, join, posix } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const content = "docs/src/content/docs";
const repo = "https://github.com/buntec/btmux";

// Re-bases relative links onto the content directory; remark-repo-links
// turns them into routes or GitHub URLs from there.
const rebase = (body, from, to) =>
  body.replace(/\]\(([^)\s]+)\)/g, (m, target) => {
    if (/^[a-z]+:|^#/.test(target)) return m;
    const resolved = posix.join(posix.dirname(from), target);
    return `](${posix.relative(posix.dirname(to), resolved)})`;
  });

// GitHub serves README videos (user-attachments) only to signed-in viewers,
// so each becomes the site's own recording (just docs-demos-fetch), picked by
// the heading above it, in the theme's mode.
const demoNames = {
  "Sessions, windows, and panes": "panes",
  "File browser, Git mode, and process viewer": "files",
  "Neovim GUI": "nvim",
  "Coding agents": "agents",
  "LaTeX overlay": "latex",
  "Shader wallpapers": "shaders",
};
const video = (name, mode, hide) =>
  `<video${hide === "none" ? "" : ` class="${hide}:sl-hidden"`} poster="/demos/${name}-${mode}.jpg" controls muted loop playsinline preload="none">` +
  `<source src="/demos/${name}-${mode}.webm" type="video/webm"><source src="/demos/${name}-${mode}.mp4" type="video/mp4"></video>`;
const videos = (body) => {
  let heading = "";
  return body.replace(
    /^### (.+)$|^https:\/\/github\.com\/user-attachments\/assets\/[\w-]+$/gm,
    (line, h) => {
      if (h) return ((heading = h), line);
      const name = demoNames[heading];
      if (!name)
        throw new Error(`README: no demo for the video under "${heading}"`);
      const has = (mode) =>
        existsSync(
          join(root, "docs", "public", "demos", `${name}-${mode}.mp4`),
        );
      if (has("dark") && has("light"))
        return (
          video(name, "dark", "light") + "\n" + video(name, "light", "dark")
        );
      // One mode, or none yet (the player then shows nothing): no theme switch.
      return video(name, has("light") ? "light" : "dark", "none");
    },
  );
};

const frontmatter = (fm) =>
  `---\n${Object.entries(fm)
    .map(([k, v]) => `${k}: ${JSON.stringify(v)}`)
    .join("\n")}\n---\n\n`;

async function page(from, name, extra = {}) {
  const src = await readFile(join(root, from), "utf8");
  const match = src.match(/^# (.+)\n+/);
  if (!match) throw new Error(`${from}: expected a leading "# Title"`);
  let body = src.slice(match[0].length);
  let fm = { title: match[1], editUrl: `${repo}/edit/main/${from}`, ...extra };
  if (name === "index") {
    // The intro becomes the hero; the sidebar replaces the link list.
    const intro = body.match(/^([\s\S]+?)\n\n/)[1];
    body = body
      .slice(intro.length + 2)
      .replace(/\n## Learn more\n[\s\S]*?(?=\n## )/, "\n");
    fm = {
      ...fm,
      template: "splash",
      hero: {
        tagline: intro
          .replace(/\n/g, " ")
          .replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2">$1</a>'),
        actions: [
          {
            text: "Install",
            link: "/start/installation/",
            icon: "right-arrow",
          },
          { text: "GitHub", link: repo, icon: "github", variant: "minimal" },
        ],
      },
    };
  }
  const to = `${content}/${name}.md`;
  await writeFile(
    join(root, to),
    frontmatter(fm) + videos(rebase(body, from, to)),
  );
}

await page("README.md", "index");
await page("desktop/README.md", "project/desktop", { sidebar: { order: 1 } });
