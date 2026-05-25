# Build & run

Rho is a single binary crate. No system services or daemons.

## Requirements

- A Rust toolchain (stable). The project tracks `edition = "2021"`.
- On Linux you'll also need iced's graphics deps. On Debian/Ubuntu:

  ```sh
  sudo apt-get install libxkbcommon-dev libwayland-dev
  ```

  Other distributions need the equivalent `libxkbcommon` and Wayland
  development headers.

- macOS and Windows have no extra prerequisites beyond Rust.

## Common commands

```sh
cargo run              # launch the app
cargo check            # fast type-check during edits
cargo build --release  # optimized binary at target/release/rho
cargo test             # run the unit tests
```

`cargo check` is the right command while iterating — `cargo build` from a
cold cache takes ~15s because iced pulls in a wide tree of crates.

## App icon

The window icon is embedded in the binary (`main.rs::APP_ICON`, decoded
from `assets/icon.png` via iced's `image` feature), so it shows up in the
title bar and taskbar on Linux and Windows out of the box.

On macOS the Dock icon of an unbundled `cargo run` binary is *not* taken
from the window icon. To get a real `Rho.app` with its own Dock icon:

```sh
./scripts/make-macos-app.sh   # → target/macos/Rho.app
open target/macos/Rho.app
```

The script (macOS-only; uses the stock `sips` + `iconutil`) builds the
release binary, renders an `.iconset` from `assets/icon.png`, packs it into
`Rho.icns`, and assembles the bundle. The artwork itself is generated from
primitives by `scripts/gen-icon.py` (pure stdlib, no design tool), which
writes `assets/icon.png` — re-run it after changing the script to
regenerate the master image.

## First launch

On first run Rho creates `~/.rho.yaml` with a starter template (see
[Configuration](./configuration.md)) and opens with both panes pointed at
`$HOME`. Subsequent launches restore the last-used folders from
`~/.rho-state.yaml` (see [Session state](./session-state.md)).
