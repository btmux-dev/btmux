---
title: "LaTeX overlay"
sidebar:
  order: 12
---

btmux scans the visible part of each pane for LaTeX: `$$…$$`, `\[…\]`,
`\(…\)`, `\begin{equation}`-style environments, `$…$`, and the bare `[` / `]`
blocks and `( … )` spans some coding agents print after their Markdown renderer
drops the backslashes. When it finds any, a `∑ N` chip appears in the pane's
title bar (or its top-right corner when title bars are off).

Click the chip, press `<prefix> + m`, or run `latex: toggle overlay` from the
command palette to open a side panel with the formulas rendered by KaTeX.

![A pane printing LaTeX source, with the overlay showing three rendered formulas](../../../assets/screenshots/latex-dark.png)
![A pane printing LaTeX source, with the overlay showing three rendered formulas](../../../assets/screenshots/latex-light.png)

Hovering a formula highlights its source in the terminal. Rebind the key with
`toggle-latex` under `[keys]`.

Detection is heuristic. Shell text such as `$HOME/bin:$PATH` is filtered out,
and anything KaTeX cannot parse is dropped silently.
