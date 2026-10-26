# Agent notes

## Keep the docs current

Update these in the same commit as the change that affects them:

- `docs/ROADMAP.md` holds the plans: decisions, steps in progress, later ideas. Read it before
  working on a feature. Tick a step when it is done, record a decision when one is made, and note
  why a plan was dropped.
- `README.md` describes what users can do today: features, install, configuration, commands.
  Update it with any user-visible change (a new option, command, backend, install path or
  behaviour), along with `config.example.toml` for config options.
- `docs/CHANGELOG.md`: add user-visible changes under `## [Unreleased]`. It follows
  [Keep a Changelog](https://keepachangelog.com/en/1.1.0/): `Added`, `Changed`, `Deprecated`,
  `Removed`, `Fixed`, `Security`; version links at the end.

## Releases

- Releases are cut by the manual Release workflow (`.github/workflows/release.yml`, run with the
  version, e.g. `gh workflow run release.yml -f version=1.1.0`). `scripts/release.py` sets the
  version in `Cargo.toml` and `Cargo.lock` and moves the `[Unreleased]` changes and link under
  it. CI then checks and builds the packages with that version; if that passes, the workflow
  commits `chore(release): X`, tags `vX`, and publishes the GitHub release with the changelog
  section as notes, the tarballs and `SHA256SUMS`. Don't bump versions or edit the release
  sections by hand.
- `package/arch/PKGBUILD` is the `conflux-git` package; its `pkgver` comes from `git describe`,
  so it needs no version bump. The tarball name and layout
  (`conflux-vX-<arch>-linux.tar.gz`, one top-level directory with `usr/` and `etc/`) are used by
  the `conflux-bin` AUR package: keep them stable.

## CI

- `.github/workflows/ci.yml` runs on every push and pull request (and is called by the Release
  workflow): `cargo fmt --check`, clippy with warnings as errors, the tests, and the release
  tarballs for x86_64 and aarch64 (static builds with `cross` and vendored OpenSSL).
- Run the checks before committing:

  ```sh
  cargo fmt --all --check
  cargo clippy --workspace --all-targets --locked -- -D warnings
  cargo test --workspace --locked
  ```

## Commits

- Conventional Commits, e.g. `feat(core): add config_edit module`,
  `fix(daemon): keep the socket after a failed reload`.
