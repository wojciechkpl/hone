"""Spec section 11.11, K60-K62 (round-7 review follow-ups).

K60: the rig's .gitignore ignores editor and system leftovers (tested in throwaway repositories
that carry the rig's real .gitignore). K61: renames are reported by source and destination in
`prereg.changed_between` and `prereg.dirty_paths`. K62: a non-draft stage-cb pre-registration
refuses an HC_SERVER other than vllm, llamacpp or mock.
"""

import json
import subprocess
import sys

import pytest
from fakebin import LLAMA_PINNED
from test_campaign1_prereg import pin, run_cb
from test_campaign1_report import section
from test_campaign1_round4 import clean_git_env, git
from test_campaign1_round5 import code_campaign, code_rig, code_rows, rev, run_cli


def in_rig(rig, expr):
    """Evaluate a hcbench.prereg expression with the rig directory as ROOT; returns its JSON."""
    # json is imported before the rig is on sys.path: a stray rig/json.py must not shadow it here
    code = (
        f"import json, sys; sys.path.insert(0, {str(rig)!r}); "
        f"import hcbench.prereg as p; print(json.dumps({expr}))"
    )
    r = subprocess.run(
        [sys.executable, "-c", code],
        cwd=rig.parent, capture_output=True, text=True, check=True,  # not rig: a stray json.py
        env=clean_git_env(),
    )  # fmt: skip
    return json.loads(r.stdout)


def touch(rig, rel, text="x\n"):
    path = rig / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


# ---- K60 editor and system leftovers ------------------------------------------------------

LEFTOVERS = [
    "config/.bench.env.swp",
    "config/.bench.env.swo",
    "hcbench/report.py~",
    "Makefile~",
    ".#Makefile",
    "hcbench/.#report.py",
    "#Makefile#",
    "scripts/#30_report.sh#",
    ".DS_Store",
    "config/.DS_Store",
    "hcbench/.DS_Store",
]


@pytest.mark.parametrize("rel", LEFTOVERS)
def test_k60_an_editor_or_system_leftover_is_not_dirty(tmp_path, rel):
    rig, _ = code_rig(tmp_path)
    touch(rig, rel)
    assert in_rig(rig, "p.dirty_paths()") == []
    assert git(rig, "status", "--porcelain", "--untracked-files=all").stdout.strip() == ""


@pytest.mark.parametrize("rel", LEFTOVERS)
def test_k60_git_check_ignore_matches_the_leftover_patterns(tmp_path, rel):
    rig, _ = code_rig(tmp_path)
    touch(rig, rel)
    r = subprocess.run(
        ["git", "check-ignore", "-q", rel], cwd=rig, env=clean_git_env(), check=False
    )
    assert r.returncode == 0


def test_k60_leftovers_do_not_make_the_report_exit_3(tmp_path):
    rig, h1 = code_rig(tmp_path)
    for rel in LEFTOVERS:
        touch(rig, rel)
    camp, pre = code_campaign(rig, h1, [{"head": h1}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 0, md
    assert code_rows(md, "code.dirty") == []


@pytest.mark.parametrize("rel", ["json.py", "hcbench/swap.py", "config/bench.env.bak", "notes.txt"])
def test_k60_the_patterns_do_not_over_ignore(tmp_path, rel):
    rig, h1 = code_rig(tmp_path)
    touch(rig, rel)
    assert in_rig(rig, "p.dirty_paths()") == [rel]
    camp, pre = code_campaign(rig, h1, [{"head": h1}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    assert any(rel in ln for ln in code_rows(md, "code.dirty"))


def test_k60_a_tracked_file_with_a_leftover_name_is_still_seen_when_it_changes(tmp_path):
    # ignore rules never hide tracked files: guards against "git add -f" tricks being invisible
    rig, h1 = code_rig(tmp_path)
    touch(rig, "hcbench/keep~", "v1\n")
    git(rig, "add", "-f", "hcbench/keep~")
    git(rig, "commit", "-q", "-m", "tracked leftover")
    h2 = rev(rig)
    touch(rig, "hcbench/keep~", "v2\n")
    assert in_rig(rig, "p.dirty_paths()") == ["hcbench/keep~"]
    assert in_rig(rig, f"p.changed_between({h1!r}, {h2!r})") == ["hcbench/keep~"]


# ---- K61 renames show both paths ----------------------------------------------------------


def mv_and_commit(rig, src, dst):
    (rig / dst).parent.mkdir(parents=True, exist_ok=True)
    git(rig, "mv", src, dst)
    git(rig, "commit", "-q", "-m", f"mv {src} {dst}")
    return rev(rig)


def test_k61_changed_between_returns_both_sides_of_a_committed_rename(tmp_path):
    rig, h1 = code_rig(tmp_path)
    h2 = mv_and_commit(rig, "hcbench/stats.py", "tests/stats_moved.py")
    got = in_rig(rig, f"p.changed_between({h1!r}, {h2!r})")
    assert sorted(got) == ["hcbench/stats.py", "tests/stats_moved.py"]


def test_k61_a_committed_move_out_of_code_exits_3_naming_the_source(tmp_path):
    rig, h1 = code_rig(tmp_path)
    h2 = mv_and_commit(rig, "hcbench/stats.py", "tests/stats_moved.py")
    camp, pre = code_campaign(rig, h1, [{"head": h2}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    rows = code_rows(md, "code.changed")
    assert rows and all("hcbench/stats.py" in ln for ln in rows)
    assert "tests/stats_moved.py" in section(md, "Conformance")  # the destination is information


def test_k61_dirty_paths_returns_both_sides_of_a_staged_rename(tmp_path):
    rig, _ = code_rig(tmp_path)
    git(rig, "mv", "hcbench/stats.py", "tests/stats_moved.py")
    assert sorted(in_rig(rig, "p.dirty_paths()")) == ["hcbench/stats.py", "tests/stats_moved.py"]


def test_k61_a_staged_move_out_of_code_exits_3_with_code_dirty(tmp_path):
    rig, h1 = code_rig(tmp_path)
    git(rig, "mv", "hcbench/stats.py", "tests/stats_moved.py")
    camp, pre = code_campaign(rig, h1, [{"head": h1}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    rows = code_rows(md, "code.dirty")
    assert rows and all("hcbench/stats.py" in ln for ln in rows)
    assert "tests/stats_moved.py" in section(md, "Conformance")


def test_k61_a_rename_between_two_code_paths_names_both(tmp_path):
    rig, h1 = code_rig(tmp_path)
    h2 = mv_and_commit(rig, "hcbench/stats.py", "scripts/stats_moved.py")
    camp, pre = code_campaign(rig, h1, [{"head": h2}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    rows = code_rows(md, "code.changed")
    assert any("hcbench/stats.py" in ln and "scripts/stats_moved.py" in ln for ln in rows)


def test_k61_a_rename_between_two_non_code_paths_stays_information(tmp_path):
    rig, h1 = code_rig(tmp_path)
    h2 = mv_and_commit(rig, "README.md", "tests/readme_moved.md")
    camp, pre = code_campaign(rig, h1, [{"head": h2}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 0, md
    text = section(md, "Conformance")
    assert "README.md" in text and "tests/readme_moved.md" in text
    assert code_rows(md, "code.changed") == []


def test_k61_a_staged_rename_between_non_code_paths_stays_information(tmp_path):
    rig, h1 = code_rig(tmp_path)
    git(rig, "mv", "README.md", "tests/readme_moved.md")
    camp, pre = code_campaign(rig, h1, [{"head": h1}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 0, md
    assert code_rows(md, "code.dirty") == []
    assert "README.md" in section(md, "Conformance")


def test_k61_a_rename_of_a_file_with_spaces_keeps_both_names(tmp_path):
    rig, _ = code_rig(tmp_path)
    touch(rig, "hcbench/a b.py")
    git(rig, "add", "-A")
    git(rig, "commit", "-q", "-m", "spaced")
    git(rig, "mv", "hcbench/a b.py", "tests/c d.py")
    assert sorted(in_rig(rig, "p.dirty_paths()")) == ["hcbench/a b.py", "tests/c d.py"]


# ---- K62 the server is one the rig knows --------------------------------------------------


@pytest.mark.parametrize("server", ["vLLM", "VLLM", "ollama", "llama.cpp", "", " vllm"])
def test_k62_non_draft_prereg_refuses_an_unknown_server(env, server):
    pin(env)
    env["mp"].setenv("HC_SERVER", server)
    with pytest.raises(SystemExit) as exc:
        run_cb(env)
    assert "HC_SERVER" in str(exc.value)
    assert not (env["tmp"] / "pre.toml").exists()


@pytest.mark.parametrize("server", ["vllm", "llamacpp", "mock"])
def test_k62_the_three_known_servers_pass_the_check(env, server):
    pin(env)
    env["mp"].setenv("HC_SERVER", server)
    env["mp"].setenv("HC_LLAMACPP_IMAGE", LLAMA_PINNED)
    rc, out = run_cb(env)
    assert rc == 0
    assert 'HC_SERVER = "' + server + '"' in out.read_text()  # mock is visible in [config]


def test_k62_a_draft_goes_on_with_an_unknown_server(env):
    pin(env)
    env["mp"].setenv("HC_SERVER", "ollama")
    rc, _ = run_cb(env, "--draft")
    assert rc == 0


def test_k62_a_misspelled_server_no_longer_skips_the_revision_check(env):
    pin(env)
    env["mp"].setenv("HC_SERVER", "vLLM")
    env["mp"].setenv("HC_MODEL_REVISION", "main")
    with pytest.raises(SystemExit) as exc:
        run_cb(env)
    assert "HC_SERVER" in str(exc.value) or "HC_MODEL_REVISION" in str(exc.value)
