# Introduction

`fm` is a cross-platform dual-pane file manager in the style of
[fman](https://fman.io/) and [Total Commander](https://www.ghisler.com/).
It is written in Rust on top of the [iced](https://iced.rs/) GUI toolkit.

Two panes sit side by side, each showing one directory. The active pane is
outlined; nearly every action — open, copy, delete, sort, filter — runs against
it. Type-to-filter, range marking with `Shift`, and a single-line modal for
the path-entry / copy / delete prompts keep the keyboard the primary input
device.

The rest of this book covers what you need to use it and what you need to
hack on it:

- [Build & run](./build-and-run.md) — getting a binary on disk.
- [Keybindings](./keybindings.md) — every key the app responds to.
- [Configuration](./configuration.md) — `~/.fm.yaml` reference.
- [Session state](./session-state.md) — `~/.fm-state.yaml` (the pane-restore
  file).
- [Architecture](./architecture.md) — module layout and the load-bearing
  invariants you should know before changing the code.
