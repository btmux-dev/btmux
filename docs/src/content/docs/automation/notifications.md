---
title: "Notifications"
sidebar:
  order: 6
---

`btmux send-notification` shows a notification in every connected browser:

```sh
btmux send-notification --level attention --body "All tests passed" "Build done"
```

`--level` is `info` (default), `success`, `attention`, or `error`. Every
client shows an in-app toast; OS notifications follow the `[notifications]`
preferences. Inside a btmux pane it targets that pane's server; elsewhere, the
running server of `--profile` (or the default profile). It wraps
`POST /api/notify`, which takes `title`, optional `body`, and `level`.
