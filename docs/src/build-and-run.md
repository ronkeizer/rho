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

## First launch

On first run Rho creates `~/.rho.yaml` with a starter template (see
[Configuration](./configuration.md)) and opens with both panes pointed at
`$HOME`. Subsequent launches restore the last-used folders from
`~/.rho-state.yaml` (see [Session state](./session-state.md)).
