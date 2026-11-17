"""Spec section 11.8, K53-K55 (fifth review). Interfaces: see test_campaign1_report.py.

Changes to the K48 evidence shape (round 6):

    code = {
      "preregistration": {"head": str, "resolves": bool},     # K53; absent = resolves
      "changed": {head: [rig-relative path] | None},          # all paths, prereg/ excluded
      "report":  {"head": str | None, "dirty_paths": [str]},  # all paths, prereg/ excluded
    }

* resolves False: one mismatch {run_id None, invocation "preregistration", field "code.head",
  found = the head}. `changed` is not consulted, but the dirty checks for every invocation and
  for the report run are still made.
* K54: check_conformance splits every changed / dirty path. Under hcbench/, harness_bin/,
  scripts/, config/, suites/, docker/ or exactly Makefile or pyproject.toml, it is code:
  `code.changed` / `code.dirty` (found: the code paths only, sorted). Anything else is
  information: the result gains "other": [{"invocation": i | "report", "kind": "changed" |
  "dirty", "paths": [sorted non-code paths]}] and the Conformance section lists them. They are
  not mismatches and do not change the exit code.
* A pre-registration with no repo.head has no code evidence at all (no check, no mismatch).
* K55: the Conformance section no longer says "shown, not compared".
"""

import json
import subprocess
import sys

import pytest
from fakebin import make_rig
from test_campaign1_report import (
    b_run,
    build_campaign,
    good_sampling,
    rec,
    run_of,
    run_report,
    section,
    write_prereg,
    write_run,
)
from test_campaign1_round3 import CLEAN, START
from test_campaign1_round5 import (
    code_campaign,
    code_rig,
    code_rows,
    commit_change,
    inv,
    rev,
    run_cli,
)

from hcbench import report

# ---- K53 the code check never goes quiet (pure) -------------------------------------------

BAD_PRE = {"head": "nope", "resolves": False}


def code_check(invocations, code):
    return report.check_conformance(
        [run_of("b.pi.s0.t1", 0, rec(good_sampling(0)))],
        {**CLEAN, "repo": {"head": "nope", "dirty": False}},
        invocations=invocations,
        code=code,
    )


def test_k53_an_unresolvable_preregistered_head_is_a_mismatch_named_preregistration():
    code = {"preregistration": BAD_PRE, "changed": {}, "report": {"head": "h1", "dirty_paths": []}}
    (m,) = code_check([inv()], code)["mismatches"]
    assert (m["invocation"], m["field"], m["found"]) == ("preregistration", "code.head", "nope")
    assert m["run_id"] is None


def test_k53_dirty_checks_are_still_made_for_every_invocation_and_the_report_run():
    code = {
        "preregistration": BAD_PRE,
        "changed": {},
        "report": {"head": "h1", "dirty_paths": ["scripts/run.sh"]},
    }
    res = code_check([inv(dirty_paths=["config/arms.toml"]), inv()], code)
    got = sorted((str(m["invocation"]), m["field"]) for m in res["mismatches"])
    assert got == [
        ("0", "code.dirty"),
        ("preregistration", "code.head"),
        ("report", "code.dirty"),
    ]


def test_k53_changed_files_are_not_judged_without_a_base_to_compare_with():
    code = {
        "preregistration": BAD_PRE,
        "changed": {"h2": ["config/arms.toml"]},
        "report": {"head": "h2", "dirty_paths": []},
    }
    res = code_check([inv(head="h2")], code)
    assert [m["field"] for m in res["mismatches"]] == ["code.head"]


def test_k53_a_resolvable_head_gives_no_preregistration_mismatch():
    code = {
        "preregistration": {"head": "h1", "resolves": True},
        "changed": {"h1": []},
        "report": {"head": "h1", "dirty_paths": []},
    }
    assert code_check([inv()], code)["mismatches"] == []


# ---- K54 code paths and other paths (pure) ------------------------------------------------

CODE_PATHS = [
    "hcbench/report.py",
    "harness_bin/pi",
    "scripts/30_report.sh",
    "config/arms.toml",
    "suites/b_mini.toml",
    "docker/redteam/plant.sh",
    "docker/tools.Dockerfile",
    "Makefile",
    "pyproject.toml",
]
OTHER_PATHS = [
    "README.md",
    "SPEC.md",
    "tests/test_x.py",
    "hcbench_notes.md",
    "Makefile.bak",
    "docs/a.md",
]


def evidence(changed=(), dirty=()):
    return {
        "preregistration": {"head": "h1", "resolves": True},
        "changed": {"h1": [], "h2": list(changed)},
        "report": {"head": "h1", "dirty_paths": list(dirty)},
    }


@pytest.mark.parametrize("path", CODE_PATHS)
def test_k54_a_changed_code_path_is_a_mismatch(path):
    (m,) = code_check([inv(head="h2")], evidence(changed=[path]))["mismatches"]
    assert (m["field"], m["found"]) == ("code.changed", [path])


@pytest.mark.parametrize("path", CODE_PATHS)
def test_k54_a_dirty_code_path_is_a_mismatch(path):
    (m,) = code_check([inv(dirty_paths=[path])], evidence())["mismatches"]
    assert (m["field"], m["found"]) == ("code.dirty", [path])


@pytest.mark.parametrize("path", OTHER_PATHS)
def test_k54_a_path_outside_the_code_set_is_information_not_a_mismatch(path):
    res = code_check([inv(head="h2", dirty_paths=[path])], evidence(changed=[path]))
    assert res["mismatches"] == []
    assert {"invocation": 0, "kind": "changed", "paths": [path]} in res["other"]
    assert {"invocation": 0, "kind": "dirty", "paths": [path]} in res["other"]


def test_k54_mixed_paths_put_only_the_code_ones_in_the_mismatch():
    res = code_check(
        [inv(head="h2")],
        evidence(changed=["README.md", "config/arms.toml", "tests/a.py", "Makefile"]),
    )
    (m,) = res["mismatches"]
    assert m["found"] == ["Makefile", "config/arms.toml"]
    assert res["other"] == [
        {"invocation": 0, "kind": "changed", "paths": ["README.md", "tests/a.py"]}
    ]


def test_k54_report_time_dirty_paths_are_split_the_same_way():
    res = code_check([inv()], evidence(dirty=["tests/fakes.py", "hcbench/stats.py"]))
    (m,) = res["mismatches"]
    assert (m["invocation"], m["field"], m["found"]) == (
        "report",
        "code.dirty",
        ["hcbench/stats.py"],
    )
    assert {"invocation": "report", "kind": "dirty", "paths": ["tests/fakes.py"]} in res["other"]


# ---- K53 / K54 / K55 with real repositories -----------------------------------------------


def test_k53_a_preregistered_head_that_is_not_a_commit_exits_3_naming_preregistration(tmp_path):
    rig, h1 = code_rig(tmp_path)
    camp, pre = code_campaign(rig, "deadbeef" * 5, [{"head": h1}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    (row,) = code_rows(md, "code.head")
    assert "preregistration" in row and "deadbeef" * 5 in row
    assert code_rows(md, "code.changed") == []


def test_k53_dirty_checks_still_run_when_the_preregistered_head_does_not_resolve(tmp_path):
    rig, h1 = code_rig(tmp_path)
    (rig / "config" / "arms.toml").write_text("# edited, not committed\n")
    camp, pre = code_campaign(
        rig, "deadbeef" * 5, [{"head": h1, "dirty": True, "dirty_paths": ["suites/b_mini.toml"]}]
    )
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    dirty_rows = code_rows(md, "code.dirty")
    assert any("invocation 0" in ln and "suites/b_mini.toml" in ln for ln in dirty_rows)
    assert any("report" in ln and "config/arms.toml" in ln for ln in dirty_rows)


def info_text(md):
    return section(md, "Conformance")


def test_k54_a_readme_change_after_the_preregistered_head_is_listed_and_exits_0(tmp_path):
    rig, h1 = code_rig(tmp_path)
    h2 = commit_change(rig, "README.md")
    camp, pre = code_campaign(rig, h1, [{"head": h2}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 0, md
    assert "README.md" in info_text(md)
    assert code_rows(md, "code.changed") == []


def test_k54_a_dirty_tests_file_at_report_time_is_listed_and_exits_0(tmp_path):
    rig, h1 = code_rig(tmp_path)
    (rig / "tests" / "fakes.py").write_text((rig / "tests" / "fakes.py").read_text() + "\n# x\n")
    camp, pre = code_campaign(rig, h1, [{"head": h1}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 0, md
    assert "tests/fakes.py" in info_text(md)
    assert code_rows(md, "code.dirty") == []


@pytest.mark.parametrize("rel", ["Makefile", "pyproject.toml"])
def test_k54_a_makefile_or_pyproject_change_exits_3(tmp_path, rel):
    rig, h1 = code_rig(tmp_path)
    h2 = commit_change(rig, rel, "\n# changed\n")
    camp, pre = code_campaign(rig, h1, [{"head": h2}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    assert any(rel in ln for ln in code_rows(md, "code.changed"))


def test_k54_mixed_changes_put_only_the_code_path_in_the_mismatch(tmp_path):
    rig, h1 = code_rig(tmp_path)
    commit_change(rig, "README.md")
    h2 = commit_change(rig, "config/arms.toml")
    camp, pre = code_campaign(rig, h1, [{"head": h2}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    rows = code_rows(md, "code.changed")
    assert rows and all("config/arms.toml" in ln and "README.md" not in ln for ln in rows)
    assert "README.md" in info_text(md)


def test_k55_the_section_no_longer_says_shown_not_compared(tmp_path):
    rig, h1 = code_rig(tmp_path)
    camp, pre = code_campaign(rig, h1, [{"head": h1}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 0, md
    text = info_text(md)
    assert "shown, not compared" not in text
    low = text.lower()
    # it states what is compared for each invocation
    assert "bundle" in low and "files" in low and "code" in low


def test_k55_wording_also_holds_without_any_code_evidence(tmp_path):
    camp = build_campaign(tmp_path)
    write_run(camp, b_run("pi", 0, started_at=START), [rec(good_sampling(0))])
    (camp / "invocations.jsonl").write_text(json.dumps(inv()) + "\n")
    assert run_report(camp, write_prereg(tmp_path, CLEAN)) == 0
    assert "shown, not compared" not in info_text((camp / "report.md").read_text())


def test_helpers_are_importable():
    assert make_rig and rev and subprocess and sys
