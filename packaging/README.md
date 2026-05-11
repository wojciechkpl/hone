# `packaging/` — distribution channels for Hone

Hone is shipped through three channels today, with two more covered
by this directory's scaffolding:

| Channel | Path | Status |
|---|---|---|
| `cargo install hone-cli honed` | crates.io (no files in this repo) | **shipped** |
| GitHub source (build from `main`) | top-level `Cargo.toml` | **shipped** |
| Tauri desktop installers | `apps/hone-desktop/ci/desktop-release.yml` | **shipped** (manual trigger) |
| `brew install wojciechkpl/hone/hone` | `packaging/homebrew/Formula/hone.rb` | **build from source today** — needs a tap repo + tagged release for `brew install hone` |
| `pip install hone-cli` | `packaging/python/` | **build from source today** — needs CI-built per-platform wheels for `pip install hone-cli` to skip the Rust toolchain step |

## Homebrew

```bash
# From a checkout — works today.
brew install --build-from-source ./packaging/homebrew/Formula/hone.rb

# From a tap once the formula is published — see § "Publishing".
brew tap wojciechkpl/hone
brew install hone

# From HEAD (always-latest, builds from main):
brew install --HEAD wojciechkpl/hone/hone
```

The formula builds the embedded SolidJS web UI when `npm` is
available (so `/ui` works out of the box), then installs both the
`hone` CLI and the `honed` daemon.

## pip

```bash
# From a checkout — works today.
git clone https://github.com/wojciechkpl/hone.git
cd hone
pip install ./packaging/python

# From PyPI once published — see § "Publishing".
pip install hone-cli
```

See [`packaging/python/README.md`](python/README.md) for the
implementation details (it's a `setup.py`-based wheel that runs
`cargo build` and copies the binaries into the wheel data dir).

## Publishing — what's left

### Homebrew tap (one-time)

1. Create a new GitHub repo named `homebrew-hone` under
   `wojciechkpl/`.
2. Copy `packaging/homebrew/Formula/hone.rb` into
   `Formula/hone.rb` of that repo.
3. Cut a release tag here (`git tag v0.1.0 && git push --tags`) and
   update the formula to point at the release tarball:
   ```ruby
   url "https://github.com/wojciechkpl/hone/archive/refs/tags/v0.1.0.tar.gz"
   sha256 "<output of `brew create` or `brew bump`>"
   ```
4. Push the tap repo. `brew tap wojciechkpl/hone && brew install hone`
   should then work.

For *automated* release bumps, add the `dawidd6/action-homebrew-bump-formula`
GitHub action to a workflow that fires on tag push.

### PyPI per-platform wheels

The current `packaging/python/setup.py` requires Rust at install
time. To remove that requirement, the path forward is:

1. Add a `packaging/python/ci/wheels.yml` workflow that mirrors
   `apps/hone-desktop/ci/desktop-release.yml`'s matrix
   (manylinux x86_64 + aarch64, macos-13 x86_64, macos-14 arm64,
   windows-2022).
2. On each runner, run `cibuildwheel` against `packaging/python/`.
3. Upload to PyPI via `pypa/gh-action-pypi-publish@release/v1`.

The `setup.py` already produces a working wheel; the only piece
missing is the CI matrix that runs it on each target platform.

## Why both?

`cargo install` is fastest for Rust users but unusable for anyone
without a Rust toolchain. Brew is the macOS/Linux norm for CLI
tools and lands `hone` next to `gh`, `jq`, `fd`, etc. pip lets
Python-heavy teams (ML, data, science) install Hone alongside their
existing `pip` workflow — particularly important for users behind
corporate proxies where `cargo install` is blocked but PyPI is
mirrored internally.
