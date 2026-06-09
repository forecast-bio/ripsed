# Packaging

Templates for distributing ripsed through package managers. Each file
contains `@VERSION@` / `@SHA256_*@` placeholders to fill from a GitHub
release (the release workflow publishes `.sha256` files next to every
artifact).

## Already automated

- **crates.io** — all five crates are published (`cargo install
  ripsed-cli` installs the `ripsed` binary). Publish order for a new
  version: `ripsed-core` → `ripsed-fs` → `ripsed-json` → `ripsed` →
  `ripsed-cli`. Dry-run first: `cargo publish -p <crate> --dry-run`.
- **GitHub releases** — five prebuilt targets plus a `.deb`
  (`cargo deb -p ripsed-cli`), with SHA256 checksums.

## Human steps (one-time, need account credentials)

- **Homebrew**: create a `homebrew-ripsed` tap repository and copy
  `homebrew/ripsed.rb` into `Formula/`. Per release: update version,
  URLs, and sha256 values.
- **Scoop**: submit `scoop/ripsed.json` to a bucket (or host an own
  bucket repo). Per release: update version and hash.
- **AUR**: create the `ripsed` AUR package with `aur/PKGBUILD`
  (needs an AUR account and SSH key). Per release: bump `pkgver`,
  update sha256, regenerate `.SRCINFO` with `makepkg --printsrcinfo`.

Checksums for a release:

```bash
curl -sL https://github.com/dollspace-gay/ripsed/releases/download/v<V>/<asset>.sha256
```
