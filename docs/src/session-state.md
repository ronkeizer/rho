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
right: /Users/you/Downloads
active: left
recent:
  - /Users/you/projects
  - /Users/you/Downloads
  - /etc
```

| Key | Type | Notes |
|---|---|---|
| `left` | path | Folder shown in the left pane. |
| `right` | path | Folder shown in the right pane. |
| `active` | `left` \| `right` | Which pane has focus. Defaults to `left` if missing (older state files written before this field existed will still load). |
| `recent` | list of paths | Recently-navigated directories, most-recent first. Used by the `⌘P` "Go to folder" modal as filterable suggestions. Capped at 50 entries; the same path is only ever listed once (move-to-front on revisit). Defaults to an empty list. |

## Recovery behaviour

Anything that could go wrong with this file falls back to your home
directory rather than blocking startup:

- File missing → both panes open at `$HOME`.
- File present but malformed → both panes open at `$HOME`.
- File parses but one or both paths no longer exist → those panes open at
  `$HOME`; the existing paths still load.

You can safely delete the file to reset; the next launch will re-create it.
