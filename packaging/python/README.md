# `hone-cli` Python wheel

A pip-installable wrapper around the Rust `hone` + `honed` binaries.

## Install (from a checkout)

```bash
git clone https://github.com/wojciechkpl/hone.git
cd hone
pip install ./packaging/python
```

The wheel build runs `cargo build --release -p hone-cli -p honed` from
the workspace root and copies both binaries into the wheel's data dir.
After install, `hone` and `honed` are on your `PATH` (under
`~/.local/bin/` or whatever pip installed scripts to).

Requires Rust 1.85+ at build time. Once installed, the binaries run
without any Python or Rust toolchain present.

## Layout

```
packaging/python/
├── pyproject.toml          (project metadata + [project.scripts])
├── setup.py                (CargoBuild command — runs cargo + copies binaries)
├── README.md               (this file)
└── src/
    └── hone_cli/
        ├── __init__.py     (hone_main / honed_main entry points; os.execv to the bins)
        └── _bin/           (populated at build time; gitignored)
```

## Path to per-platform wheels (no Rust toolchain needed)

The setup above requires Rust on the user's machine. For wide
distribution we want pre-built per-platform wheels on PyPI, so
`pip install hone-cli` Just Works without any Rust install. Two
common approaches:

1. **`maturin` with `bindings = "bin"`** — produces a binary wheel
   per (Python version × OS × arch). Need to run it in CI for each
   target platform (manylinux, macos-arm64, macos-x86_64, windows).
2. **`cibuildwheel` + the existing `setup.py`** — keeps the current
   setup but matrices it across platforms in CI. Less Rust-native,
   more Python-standard.

Either way, `apps/hone-desktop/ci/desktop-release.yml` is the existing
template for cross-platform release builds in this repo. A
`packaging/python/ci/release.yml` mirroring its matrix would publish
to PyPI on tag.

## Why ship hone via pip at all?

Two audiences:
1. Python-heavy data/ML teams that already use `pip` for their
   tooling (Jupyter, ruff, black, etc.) and want one consistent
   install path.
2. Anyone behind a corporate proxy where `cargo install` is blocked
   but PyPI is mirrored internally (common pattern).

Neither audience is well-served by `cargo install hone-cli`.
