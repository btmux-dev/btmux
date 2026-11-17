import { readFileSync } from "node:fs";
import { defineConfig } from "astro/config";
import { unified } from "@astrojs/markdown-remark";
import starlight from "@astrojs/starlight";
import starlightLinksValidator from "starlight-links-validator";
import starlightLlmsTxt from "starlight-llms-txt";
import { remarkRepoLinks } from "./src/remark-repo-links.mjs";
import { generateChangelogPage } from "./scripts/changelog-page.mjs";

const repo = "https://github.com/buntec/btmux";

// Render the repo-root CHANGELOG.md as the Project > Changelog page.
const released = generateChangelogPage("https://github.com/btmux-dev/btmux");

// The version beside the site title (src/components/SiteTitle.astro), linking
// to its changelog entry: heading anchors drop the dots, `## 0.0.206` is `#00206`.
const version = readFileSync(new URL("../Cargo.toml", import.meta.url), "utf8")
  .match(/^version = "([^"]+)"/m)[1];
const versionHref = `/project/changelog/#${
  released.includes(version) ? version.replaceAll(".", "") : "unreleased"
}`;

// Sidebar groups, one per content directory; pages order themselves with
// `sidebar.order` in their frontmatter.
const groups = {
  "Start here": "start",
  Features: "features",
  "Coding agents": "agents",
  Automation: "automation",
  Configuration: "configuration",
  Internals: "internals",
  Project: "project",
};

export default defineConfig({
  site: "https://btmux.dev",
  markdown: {
    processor: unified({ remarkPlugins: [[remarkRepoLinks, { repo }]] }),
  },
  vite: {
    define: {
      __BTMUX_VERSION__: JSON.stringify(version),
      __BTMUX_VERSION_HREF__: JSON.stringify(versionHref),
    },
  },
  integrations: [
    starlight({
      plugins: [
        starlightLinksValidator(),
        // /llms.txt, /llms-full.txt, and /llms-small.txt (https://llmstxt.org).
        starlightLlmsTxt({
          projectName: "btmux",
          description:
            "btmux is a terminal emulator and tmux-style session manager that runs in the browser, a desktop app, and on phones. A Rust server owns the shells; clients attach over WebSockets. It tracks coding agents, and has a REST API, an MCP server, and a tmux-style CLI.",
        }),
      ],
      title: "btmux",
      description:
        "A browser-based tmux: terminal emulator and session manager.",
      customCss: [
        "@fontsource-variable/geist",
        "@fontsource-variable/geist-mono",
        "@fontsource-variable/geist-pixel/elsh.css",
        "./src/styles/custom.css",
      ],
      components: { SiteTitle: "./src/components/SiteTitle.astro" },
      social: [{ icon: "github", label: "GitHub", href: repo }],
      editLink: { baseUrl: `${repo}/edit/main/docs/` },
      sidebar: Object.entries(groups).map(([label, directory]) => ({
        label,
        items: [{ autogenerate: { directory } }],
      })),
    }),
  ],
});
