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

## Distributing a `.dmg` (macOS)

`scripts/make-dmg.sh` wraps the app bundle into a drag-to-Applications disk
image (Apple Silicon / arm64, unsigned):

```sh
./scripts/make-dmg.sh        # → target/macos/Rho-<version>-arm64.dmg
```

It calls `make-macos-app.sh`, stages `Rho.app` next to an `/Applications`
symlink, and runs `hdiutil create` to produce a compressed (`UDZO`) image.
The version in the filename comes from `Cargo.toml`.

### Gatekeeper

The DMG is **unsigned and un-notarized**, so a downloaded copy is
quarantined and macOS refuses to open it on first launch ("Apple cannot
check it for malicious software"). Users get past it once with either:

- **right-click → Open** in Finder, then confirm (remembered thereafter), or
- `xattr -dr com.apple.quarantine /Applications/Rho.app`.

Signing + notarization (a paid Apple Developer ID, then `codesign` →
`notarytool` → `stapler`) would remove the warning entirely. The scripts
are structured so that step can be slotted in later without reworking the
DMG layout.

### Automated releases

`.github/workflows/release.yml` builds the DMG on a `macos-latest` (arm64)
runner. Push a version tag and it attaches the `.dmg` to a GitHub Release:

```sh
git tag v0.1.0 && git push origin v0.1.0
```

Running the workflow manually (Actions tab → *Release* → *Run workflow*)
skips the release step and just uploads the DMG as a build artifact.

## First launch

On first run Rho creates `~/.rho.yaml` with a starter template (see
[Configuration](./configuration.md)) and opens with both panes pointed at
`$HOME`. Subsequent launches restore the last-used folders from
`~/.rho-state.yaml` (see [Session state](./session-state.md)).
