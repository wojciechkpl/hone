"""Build script for the hone-cli pip wheel.

Runs `cargo build --release -p hone-cli -p honed` from the repository
root and copies both binaries into the wheel so pip's console_scripts
entry points can launch them.

This is the simplest "works today" packaging path. The path forward
for per-platform pre-built wheels (no Rust toolchain on the user's
machine) is documented in packaging/README.md.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

from setuptools import setup
from setuptools.command.build_py import build_py


HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parent.parent
PACKAGE_DIR = HERE / "src" / "hone_cli"
BINARIES = ["hone", "honed"]


class CargoBuild(build_py):
    """Run `cargo build --release` and copy binaries into the package."""

    def run(self) -> None:
        if not (REPO_ROOT / "Cargo.toml").exists():
            raise RuntimeError(
                f"expected workspace Cargo.toml at {REPO_ROOT}; "
                "run pip install from a checkout of the hone repo"
            )

        print("hone-cli wheel: building Rust binaries with cargo …", flush=True)
        subprocess.check_call(
            [
                "cargo",
                "build",
                "--release",
                "-p",
                "hone-cli",
                "-p",
                "honed",
            ],
            cwd=REPO_ROOT,
        )

        bin_target = PACKAGE_DIR / "_bin"
        bin_target.mkdir(parents=True, exist_ok=True)
        for name in BINARIES:
            ext = ".exe" if sys.platform == "win32" else ""
            src = REPO_ROOT / "target" / "release" / f"{name}{ext}"
            if not src.exists():
                raise RuntimeError(f"cargo did not produce {src}")
            dst = bin_target / f"{name}{ext}"
            shutil.copy2(src, dst)
            os.chmod(dst, 0o755)
            print(f"hone-cli wheel: installed {dst}", flush=True)

        super().run()


setup(cmdclass={"build_py": CargoBuild})
