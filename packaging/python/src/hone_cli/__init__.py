"""hone-cli — pip-installable wrapper around the Rust hone + honed binaries.

The actual binaries live under `_bin/` inside the installed package
(populated by setup.py at build time). The two `*_main` functions are
the entry points wired up via `[project.scripts]` in pyproject.toml.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

__all__ = ["hone_main", "honed_main"]
__version__ = "0.1.0"

_BIN_DIR = Path(__file__).resolve().parent / "_bin"
_EXE_SUFFIX = ".exe" if sys.platform == "win32" else ""


def _exec(binary: str) -> None:
    """Replace the current process with the named binary, forwarding argv.

    Using `os.execv` (instead of `subprocess.run` + `sys.exit`) means the
    binary's exit code, signal handling, and TTY ownership flow naturally
    to the user. Important for the TUI.
    """
    path = _BIN_DIR / f"{binary}{_EXE_SUFFIX}"
    if not path.exists():
        raise FileNotFoundError(
            f"{binary} binary not found at {path}; the wheel was built "
            "without the binaries (re-install: `pip install --force-reinstall hone-cli`)"
        )
    os.execv(str(path), [str(path), *sys.argv[1:]])


def hone_main() -> None:
    _exec("hone")


def honed_main() -> None:
    _exec("honed")
