"""Spec section 4: hcbench.tools."""

import pytest

from hcbench.tools import classify_tool

READ = (
    "read read_file grep glob find ls list_dir lsp_hover lsp_diagnostics "
    "git_status git_diff memory_read"
)
EDIT = "edit write edit_file write_file multi_edit apply_patch"
EXEC = "bash shell"


@pytest.mark.parametrize("name", READ.split())
def test_classify_tool_when_read_tool_should_return_read(name):
    assert classify_tool(name) == "read"


@pytest.mark.parametrize("name", EDIT.split())
def test_classify_tool_when_edit_tool_should_return_edit(name):
    assert classify_tool(name) == "edit"


@pytest.mark.parametrize("name", EXEC.split())
def test_classify_tool_when_exec_tool_should_return_exec(name):
    assert classify_tool(name) == "exec"


@pytest.mark.parametrize(
    ("name", "expected"),
    [
        ("Read", "read"),
        ("GREP", "read"),
        ("Edit", "edit"),
        ("Apply_Patch", "edit"),
        ("BASH", "exec"),
        ("Shell", "exec"),
    ],
)
def test_classify_tool_should_be_case_insensitive(name, expected):
    assert classify_tool(name) == expected


@pytest.mark.parametrize("name", ["web_search", "task", "mystery", "", "reader", "bash_extra"])
def test_classify_tool_when_unknown_should_return_other(name):
    assert classify_tool(name) == "other"
