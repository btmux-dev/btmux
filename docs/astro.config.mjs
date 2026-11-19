import { readFileSync } from "node:fs";
import { defineConfig } from "astro/config";
import { unified } from "@astrojs/markdown-remark";
import starlight from "@astrojs/starlight";
import starlightLinksValidator from "starlight-links-validator";
import starlightLlmsTxt from "starlight-llms-txt";
import starlightOpenAPI, { createOpenAPISidebarGroup } from "starlight-openapi";
import { remarkApiEndpoints } from "./src/remark-api-endpoints.mjs";
import { remarkRepoLinks } from "./src/remark-repo-links.mjs";
import { generateChangelogPage } from "./scripts/changelog-page.mjs";

const repo = "https://github.com/btmux-dev/btmux";

// Served as a GitHub Pages project site, under /btmux.
const site = "https://btmux-dev.github.io";
const base = "/btmux";

// Render the repo-root CHANGELOG.md as the Project > Changelog page.
const released = generateChangelogPage("https://github.com/btmux-dev/btmux");

// The version beside the site title (src/components/SiteTitle.astro), linking
// to its changelog entry: heading anchors drop the dots, `## 0.0.206` is `#00206`.
const version = readFileSync(new URL("../Cargo.toml", import.meta.url), "utf8")
  .match(/^version = "([^"]+)"/m)[1];
const versionHref = `${base}/project/changelog/#${
  released.includes(version) ? version.replaceAll(".", "") : "unreleased"
}`;

// The REST API reference, generated from public/openapi.json (`just
// protocol`) and listed under Automation.
const apiReference = createOpenAPISidebarGroup();

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
  site,
  base,
  markdown: {
    processor: unified({
      remarkPlugins: [remarkApiEndpoints, [remarkRepoLinks, { repo, base }]],
    }),
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
        starlightOpenAPI([
          {
            base: "automation/api",
            schema: "./public/openapi.json",
            // Fetch samples parse every reply as JSON, which breaks on 204s
            // and on text and byte replies.
            snippets: { operation: { clients: { shell: ["curl"] } } },
            sidebar: {
              label: "API reference",
              group: apiReference,
              operations: { badges: true },
            },
          },
        ]),
        // It can't see starlight-openapi's routes.
        starlightLinksValidator({ exclude: [`${base}/automation/api/**`] }),
        // /llms.txt, /llms-full.txt, and /llms-small.txt (https://llmstxt.org).
        starlightLlmsTxt({
          projectName: "btmux",
          description:
            "btmux is a persistent terminal workspace and tmux-style session manager that runs in the browser, a desktop app, and on phones. A Rust server owns the shells; clients attach over WebSockets. It tracks coding agents, and has a REST API, an MCP server, and a tmux-style CLI.",
        }),
      ],
      title: "btmux",
      description:
        "A persistent terminal workspace and session manager for desktop, browser, and mobile.",
      customCss: [
        "@fontsource-variable/geist",
        "@fontsource-variable/geist-mono",
        "@fontsource-variable/geist-pixel/elsh.css",
        "./src/styles/custom.css",
      ],
      components: {
        Head: "./src/components/Head.astro",
        SiteTitle: "./src/components/SiteTitle.astro",
      },
      social: [{ icon: "github", label: "GitHub", href: repo }],
      editLink: { baseUrl: `${repo}/edit/main/docs/` },
      sidebar: Object.entries(groups).map(([label, directory]) => ({
        label,
        items: [
          { autogenerate: { directory } },
          ...(directory === "automation" ? [apiReference] : []),
        ],
      })),
    }),
  ],
});
