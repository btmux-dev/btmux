// Generates the docs' Changelog page from the repo-root CHANGELOG.md, the
// single source. Runs at config load, so `astro dev` picks up changelog edits
// on restart. The output is gitignored.
//
// Drops the `# Changelog` heading (Starlight renders the title), turns
// `## [x.y.z] - date` into `## x.y.z` for clean anchors, and adds a line under
// each release with its date and links to the GitHub release and the diff from
// the release before it.
import { readFileSync, writeFileSync } from "node:fs";

export function generateChangelogPage(repo) {
  const source = readFileSync(
    new URL("../../CHANGELOG.md", import.meta.url),
    "utf8",
  );
  const versions = [...source.matchAll(/^## \[(\d[^\]]*)\]/gm)].map((m) => m[1]);
  const body = source
    .replace(/^# Changelog\s*\n/, "")
    .replace(/^## \[([^\]]+)\](?: - (\S+))?[ \t]*$/gm, (_, version, date) => {
      if (version === "Unreleased") {
        const last = versions[0];
        return last
          ? `## Unreleased\n\n[Changes since ${last}](${repo}/compare/v${last}...main)`
          : "## Unreleased";
      }
      const previous = versions[versions.indexOf(version) + 1];
      const line = [
        date,
        `[GitHub release](${repo}/releases/tag/v${version})`,
        previous &&
          `[Changes since ${previous}](${repo}/compare/v${previous}...v${version})`,
      ].filter(Boolean);
      return `## ${version}\n\n${line.join(" · ")}`;
    });
  const page = `---
title: "Changelog"
description: "Notable changes in each btmux release."
editUrl: "${repo}/edit/main/CHANGELOG.md"
sidebar:
  order: 3
---

<!-- Generated from the repo-root CHANGELOG.md by docs/scripts/changelog-page.mjs. Do not edit. -->

${body}`;
  writeFileSync(
    new URL("../src/content/docs/project/changelog.md", import.meta.url),
    page,
  );
}
