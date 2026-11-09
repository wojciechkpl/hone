"""Harness-neutral tool taxonomy (Pi and Hone tool names → read/edit/exec/other)."""

from __future__ import annotations

READ_TOOLS = frozenset(
    {
        "read",
        "read_file",
        "grep",
        "glob",
        "find",
        "ls",
        "list_dir",
        "lsp_hover",
        "lsp_diagnostics",
        "git_status",
        "git_diff",
        "memory_read",
    }
)
EDIT_TOOLS = frozenset({"edit", "write", "edit_file", "write_file", "multi_edit", "apply_patch"})
EXEC_TOOLS = frozenset({"bash", "shell"})


def classify_tool(name: str) -> str:
    n = (name or "").lower()
    if n in READ_TOOLS:
        return "read"
    if n in EDIT_TOOLS:
        return "edit"
    if n in EXEC_TOOLS:
        return "exec"
    return "other"
