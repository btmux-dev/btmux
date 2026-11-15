---
title: "Process and port viewer"
sidebar:
  order: 3
---

Open the viewer with `<prefix> + t`. Its keys work without the prefix:

| Key | Action |
| --- | --- |
| `p` | Switch between processes and ports |
| `j` / `k`, arrows | Move between rows |
| `g` / `G` | Move to the first / last row |
| `h` / `l`, left / right | Collapse / expand a process tree or port's socket details |
| `Tab`, `Enter`, `Space` | Toggle expansion |
| `/` | Filter by socket fields or process details; `Enter` / `Esc` clears the filter |
| `s` | Cycle sort modes; column headers select a sort directly |
| `F` | Follow the selected process or port group as rows reorder |
| `V` | Switch between flat and tree process views |
| `x` / `X` | Request termination / force kill, then confirm with `y` or cancel with `n` / `Esc` |
| `q` / `Esc` | Close the viewer (`Esc` first clears a filter or cancels confirmation) |

Ports initially sort with TCP listeners first, then by ascending port within
each category. All connections and UDP sockets remain visible. Choose the PORT
header for numeric sorting, or **Listeners first** to restore the initial order.
UDP `BOUND` means a socket has a local port; it does not identify a listening
service. While filtering, `p` is filter text rather than a mode switch.

Known owners group by local port and PID, with individual sockets and remote
endpoints available on expansion. Unknown owners group by protocol and local
and remote endpoints; matching endpoints do not establish a shared process
owner. Rows without process information cannot be signaled.

The viewer samples once per second. Port enumeration runs independently for
each viewer only while its ports view is open. Permissions can limit visibility:
on macOS, sockets belonging to inaccessible processes may be omitted entirely.
