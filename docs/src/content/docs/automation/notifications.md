---
title: "Notifications"
sidebar:
  order: 7
---

`btmux send-notification` shows a notification in every connected client:

```sh
btmux send-notification --level attention --body "All tests passed" "Build done"
```

`--level` is `info` (default), `success`, `attention`, or `error`. Every
client shows a toast, and OS notifications follow the
[`[notifications]`](../agents/index.md#notifications) settings.

Inside a pane the command targets that pane's server; elsewhere, the running
server of `--profile`. It wraps `POST /api/notify`, which takes `title`,
optional `body`, and `level`.
