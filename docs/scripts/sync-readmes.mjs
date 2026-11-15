// Generates the home and desktop pages from README.md and desktop/README.md.
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

// GitHub renders bare user-attachment URLs as players; do the same here.
const videos = (body) =>
  body.replace(
    /^(https:\/\/github\.com\/user-attachments\/assets\/[\w-]+)$/gm,
    '<video src="$1" controls muted playsinline style="width:100%"></video>',
  );

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
