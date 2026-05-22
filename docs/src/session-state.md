# Session state

`~/.rho-state.yaml` holds the last-used pane folders so the app reopens
where you left it. It is written eagerly on any path change or active-pane
switch.

This file is separate from [`~/.rho.yaml`](./configuration.md) on purpose:
settings are user-edited and may be hand-tuned, but session state is
machine-managed and you generally shouldn't touch it.

## Format

```yaml
left: /Users/you/projects
right: alice.dev:/var/log
active: left
recent:
  - /Users/you/projects
  - /Users/you/Downloads
  - /etc
```

| Key | Type | Notes |
|---|---|---|
| `left` | location | Folder shown in the left pane. See "Location format" below. |
| `right` | location | Folder shown in the right pane. |
| `active` | `left` \| `right` | Which pane has focus. Defaults to `left` if missing (older state files written before this field existed will still load). |
| `recent` | list of paths | Recently-navigated **local** directories, most-recent first. Used by the `⌘P` "Go to folder" modal as filterable suggestions. Capped at 50 entries; the same path is only ever listed once (move-to-front on revisit). Defaults to an empty list. Remote panes don't (yet) feed this list. |

## Location format

A `left` / `right` entry is one of:

- **Local path** — a plain filesystem path: `/Users/you/projects`,
  `C:\Users\you`, etc. This is the only form older state files use, and
  it still round-trips unchanged.
- **Remote path** — `<backend>:<path>`, where `<backend>` is an alias
  from `~/.ssh/config` and `<path>` starts with `/` or `~`. Example:
  `alice.dev:/var/log`. Written when the user clicks **Open** on an
  entry in the SSH-server picker (`⌘P` → "Connect to SSH server").

The parser disambiguates Windows drive letters (`C:\foo`) and
colon-containing local filenames (`file:notes.txt`) from the remote
form by requiring the remote path to start with `/` or `~`.

## Recovery behaviour

Anything that could go wrong with this file falls back to your home
directory rather than blocking startup:

- File missing → both panes open at `$HOME`.
- File present but malformed → both panes open at `$HOME`.
- Saved **local** path no longer exists → that pane opens at `$HOME`;
  the other pane still restores normally.
- Saved **remote** path always restores — we can't cheaply pre-check the
  host, so the pane shows up empty if the host is unreachable and the
  user can navigate away or close it.

You can safely delete the file to reset; the next launch will re-create it.
