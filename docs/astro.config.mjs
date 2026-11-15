import { defineConfig } from "astro/config";
import { unified } from "@astrojs/markdown-remark";
import starlight from "@astrojs/starlight";
import starlightLinksValidator from "starlight-links-validator";
import { remarkRepoLinks } from "./src/remark-repo-links.mjs";

const repo = "https://github.com/buntec/btmux";

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
  integrations: [
    starlight({
      plugins: [starlightLinksValidator()],
      title: "btmux",
      description:
        "A browser-based tmux: terminal emulator and session manager.",
      logo: { src: "./src/assets/logo.svg" },
      social: [{ icon: "github", label: "GitHub", href: repo }],
      editLink: { baseUrl: `${repo}/edit/main/docs/` },
      sidebar: Object.entries(groups).map(([label, directory]) => ({
        label,
        items: [{ autogenerate: { directory } }],
      })),
    }),
  ],
});
