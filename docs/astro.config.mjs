import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import { remarkRepoLinks } from "./src/remark-repo-links.mjs";

const repo = "https://github.com/buntec/btmux";

export default defineConfig({
  site: "https://btmux.dev",
  markdown: { remarkPlugins: [[remarkRepoLinks, { repo }]] },
  integrations: [
    starlight({
      title: "btmux",
      description:
        "A browser-based tmux: terminal emulator and session manager.",
      logo: { src: "./src/assets/logo.svg" },
      social: [
        {
          icon: "github",
          label: "GitHub",
          href: repo,
        },
      ],
      editLink: { baseUrl: `${repo}/edit/main/docs/` },
      sidebar: [
        {
          label: "Getting started",
          items: ["installation", "phones-and-tailscale", "tmux", "upgrade"],
        },
        {
          label: "Guides",
          items: [
            "configuration",
            "shaders",
            "editors",
            "neovim",
            "vscode",
            "browser",
            "sandbox",
          ],
        },
        { label: "Automation", items: ["automation"] },
        { label: "Project", items: ["desktop", "competitors", "development"] },
      ],
    }),
  ],
});
