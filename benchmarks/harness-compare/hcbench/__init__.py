"""hcbench — harness-controlled benchmark for comparing coding-agent harnesses.

Holds the model constant (one locally served endpoint behind a metering proxy)
and varies only the harness. See SPEC.md for module contracts and
docs/plans/trust-defaults-daemon-2026-11.md for the experiment design.
"""

import sys

__version__ = "0.1.0"


def _keep_stdlib_ahead_of_the_working_directory() -> None:
    """When run as `python -m hcbench.<tool>` the working directory is first on sys.path, so a
    stray `json.py` there would replace the standard library inside the tool that judges the runs
    (the report would crash, or worse). Put the working directory after everything else; the
    hcbench package itself is already imported (spec 11.10 K56)."""
    argv = getattr(sys, "orig_argv", [])
    if (
        "-m" in argv
        and argv[argv.index("-m") + 1 :][:1]
        and argv[argv.index("-m") + 1].startswith("hcbench.")
    ):
        import os

        cwd = os.getcwd()
        sys.path[:] = [p for p in sys.path if os.path.abspath(p or cwd) != cwd] + [cwd]


_keep_stdlib_ahead_of_the_working_directory()
