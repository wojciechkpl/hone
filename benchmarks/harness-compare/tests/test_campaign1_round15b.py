"""Spec section 11.17, round 15b, K129-K133: classify the fork's grading failures.

Contract choices made here (the spec leaves them open):
* K129/K130 are tested through ``hcbench.grade.main`` with ``subprocess.run`` faked. The fake
  writes the fork-style report and the logs under
  ``<grading dir>/logs/run_evaluation/<run_id>/<model>/<instance>/`` the way the fork does.
  Statuses are read from ``grading/outcomes.csv``: ``grader_infra`` and ``ambiguous`` have a
  blank ``resolved``, ``ambiguous_repeated`` has ``0``.
* K132: ``hcbench.grade.package_version(name) -> str`` reads a package version with
  ``importlib.metadata.version``; ``grading/grader.json`` gets ``swebench_version`` and
  ``docker_version``.
* K131: ``prereg/<name>/preregistration.toml`` holds the stage-a ``[grader]`` table
  (``repo``, ``commit``, ``namespace``). The environment values count as set only when the
  env file or the process sets them, so these tests use an env file without them.
"""
# ruff: noqa: F811, F401  (the fixtures `stage_a_env` and `hub` are imported by name)

import csv
import importlib.metadata
import io
import json
import logging
import subprocess

import pytest
from fakebin import ROOT, calls, env_file_without, make_fake_bin, make_rig, run_script
from test_campaign1_prereg import EXAMPLE, pin
from test_campaign1_round14 import run_a, stage_a_env
from test_campaign1_round14b import preflight
from test_campaign1_round15 import FORK, IDS3, NS, REF, fork_inputs, grade_args, make_campaign
from test_campaign1_round15 import flag as cmd_flag

from hcbench import grade, prereg

# ---- a fake run_evaluation that leaves logs behind ----


class ForkEvaluate:
    """One scripted step per call; the last step repeats. A step is
    {"report": {...}, "logs": {instance: {filename: text}}}."""

    def __init__(self, *steps):
        self.steps, self.cmds = steps, []

    def __call__(self, cmd, **kw):
        step = self.steps[min(len(self.cmds), len(self.steps) - 1)]
        self.cmds.append(list(cmd))
        cwd = kw["cwd"]
        report = {"resolved_ids": [], **step.get("report", {})}  # the fork always writes it
        (cwd / "hcbench.report.json").write_text(json.dumps(report))
        run_id = cmd_flag(cmd, "--run_id")
        for iid, files in step.get("logs", {}).items():
            d = cwd / "logs" / "run_evaluation" / run_id / "model" / iid
            d.mkdir(parents=True, exist_ok=True)
            for name, text in files.items():
                (d / name).write_text(text)
        return subprocess.CompletedProcess(cmd, 0)


def outcomes(camp):
    with (camp / "grading" / "outcomes.csv").open() as fh:
        return {
            r["run_id"].split(".", 3)[-1]: (r["resolved"], r["status"]) for r in csv.DictReader(fh)
        }


def run_fork(tmp_path, monkeypatch, steps, ids=("a__a-1",), *extra, fork=True, camp=None):
    camp = camp or make_campaign(tmp_path, list(ids))
    d, _ = fork_inputs(tmp_path, list(ids)) if not (tmp_path / "prereg").exists() else (
        tmp_path / "prereg", None)  # fmt: skip
    ev = ForkEvaluate(*steps)
    monkeypatch.setattr(grade.subprocess, "run", ev)
    if fork:
        args = grade_args(camp, d)
    else:
        args = ["--campaign", str(camp), "--dataset", str(d / "dataset.json"), "--split", "test"]
    rc = grade.main([*args, *extra])
    return rc, camp, ev


def fork_log(*messages):
    """Text as the fork's setup_logger writes it: every message goes through a real
    logging.Logger with `%(asctime)s - %(levelname)s - %(message)s` (docker_build.py:59).
    `messages` are (level, text) pairs; a multi-line text keeps its later lines unprefixed."""
    stream = io.StringIO()
    logger = logging.getLogger(f"fork-fixture.{id(stream)}")
    handler = logging.StreamHandler(stream)
    handler.setFormatter(logging.Formatter("%(asctime)s - %(levelname)s - %(message)s"))
    logger.addHandler(handler)
    logger.setLevel(logging.INFO)
    logger.propagate = False
    try:
        for level, text in messages:
            getattr(logger, level)(text)
    finally:
        logger.removeHandler(handler)
    return stream.getvalue()


def run_log(iid, error=None, patch="diff --git a/f.py b/f.py\n+pass\n"):
    """A run_instance.log as eval.py writes it. A failed run logs the traceback at INFO and
    then, at ERROR, `Error in evaluating model for <iid>: <error>` with the traceback and the
    "Check (...) for more information." line, as the fork's `except Exception` does."""
    ok = [
        ("info", f"Container for {iid} started: 1a2b3c"),
        ("info", f"Git diff before:\n{patch}"),
        ("info", "Test runtime: 1.23 seconds"),
        ("info", f"Test output for {iid} written to logs/run_evaluation/x/{iid}/test_output.txt"),
        ("info", f"Git diff after:\n{patch}"),
    ]
    if error is None:
        return fork_log(*ok)
    trace = 'Traceback (most recent call last):\n  File "eval.py", line 1, in run_instance'
    return fork_log(
        *ok[:3],
        ("info", trace),
        (
            "error",
            f"Error in evaluating model for {iid}: {error}\n{trace}\n"
            f"Check (logs/run_evaluation/x/{iid}/run_instance.log) for more information.",
        ),
        ("info", f"Container for {iid} stopped"),
    )


def step(iid, error=None, *, bucket="error_ids", log=None, test_output=None, **more):
    """A grading attempt for one instance. K134: the signature belongs in `error`, the
    harness's error block; `log` and `test_output` replace the whole files."""
    files = {"run_instance.log": log if log is not None else run_log(iid, error)}
    if test_output is not None:
        files["test_output.txt"] = test_output
    return {"report": {bucket: [iid], **more}, "logs": {iid: files}}


AMB = "TimeoutError: Test timed out after 1800 seconds."  # what the fork's eval.py writes
INFRA = "docker.errors.APIError: Error creating container"

# ---- K129 the signatures (K134: error_ids only, in the error block) -----------------------

ENVIRONMENT = [
    "Error building image",
    "Error occurred while pulling image x",
    "Error creating container",
    "Read timed out",
    "UnixHTTPConnectionPool(host='localhost')",
    "Failed to launch the browser",
    "Failed to connect to the bus",
    "cannot open display",
    "Missing X server",
    "unable to open X display",
    "Cannot allocate memory",
    "OutOfMemoryError",
    "boom\nKilled\nelse",
    "Error response from daemon: conflict",
    "Cannot connect to the Docker daemon",
    "Could not resolve host: pypi.org",
    "Temporary failure in name resolution",
]
AMBIGUOUS = [
    "Cannot find module 'x'",
    "MODULE_NOT_FOUND",
    "ModuleNotFoundError: No module named 'y'",
    "no tests ran in 0.1s",
    "collected 0 items",
    "Timeout error: 1800 seconds exceeded.",
    "TimeoutError: Test timed out after 1800 seconds.",  # K134: the fork's own wording
]


@pytest.mark.parametrize("text", ENVIRONMENT)
def test_k129_environment_signature_should_be_grader_infra(tmp_path, monkeypatch, text):
    rc, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", text)])
    assert outcomes(camp)["a__a-1"] == ("", "grader_infra")
    assert rc == 1  # to be re-graded


@pytest.mark.parametrize("text", AMBIGUOUS)
def test_k129_ambiguous_signature_should_be_ambiguous(tmp_path, monkeypatch, text):
    rc, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", text)])
    assert outcomes(camp)["a__a-1"] == ("", "ambiguous")
    assert rc == 1


@pytest.mark.parametrize(
    "text",
    [
        "AssertionError: 1 != 2",
        "Killed in action",  # ^Killed$ is a whole line
        "Timeout error: seconds exceeded",  # needs digits
        "Test timed out after seconds",  # needs digits
        "",
    ],
)
def test_k129_no_signature_should_keep_the_status(tmp_path, monkeypatch, text):
    rc, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", text)])
    assert outcomes(camp)["a__a-1"] == ("0", "error")
    assert rc == 0


def test_k129_an_instance_without_any_log_should_keep_the_status(tmp_path, monkeypatch):
    _, camp, _ = run_fork(tmp_path, monkeypatch, [{"report": {"error_ids": ["a__a-1"]}}])
    assert outcomes(camp)["a__a-1"] == ("0", "error")


def test_k129_the_first_signature_should_win_so_the_environment_tier_beats_ambiguous(
    tmp_path, monkeypatch
):
    text = "ModuleNotFoundError: x\nTimeout error: 5 seconds exceeded.\nCannot allocate memory"
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", text)])
    assert outcomes(camp)["a__a-1"][1] == "grader_infra"


def test_k129_apply_failure_should_keep_precedence_over_every_signature(tmp_path, monkeypatch):
    log = fork_log(("info", ">>>>> Patch Apply Failed:\nerror: patch failed: f.py:1")) + run_log(
        "a__a-1", INFRA + "\nTimeout error: 5 seconds exceeded."
    )
    rc, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", log=log)])
    assert outcomes(camp)["a__a-1"] == ("0", "apply_failed")
    assert rc == 0


def test_k129_resolved_and_empty_patch_should_never_be_classified(tmp_path, monkeypatch):
    ids = ("a__a-1", "b__b-2")
    s = {
        "report": {"resolved_ids": ["a__a-1"], "empty_patch_ids": ["b__b-2"]},
        "logs": {i: {"run_instance.log": run_log(i, INFRA + "\n" + AMB)} for i in ids},
    }
    rc, camp, _ = run_fork(tmp_path, monkeypatch, [s], ids)
    assert outcomes(camp) == {"a__a-1": ("1", "resolved"), "b__b-2": ("0", "empty_patch")}
    assert rc == 0


def test_k129_each_instance_should_be_classified_from_its_own_logs(tmp_path, monkeypatch):
    ids = ("a__a-1", "b__b-2", "c__c-3")
    s = {
        "report": {"error_ids": list(ids)},
        "logs": {
            "a__a-1": {"run_instance.log": run_log("a__a-1", INFRA)},
            "b__b-2": {"run_instance.log": run_log("b__b-2", AMB)},
            "c__c-3": {"run_instance.log": run_log("c__c-3", "AssertionError")},
        },
    }
    _, camp, _ = run_fork(tmp_path, monkeypatch, [s], ids)
    assert outcomes(camp) == {
        "a__a-1": ("", "grader_infra"),
        "b__b-2": ("", "ambiguous"),
        "c__c-3": ("0", "error"),
    }


def test_k129_the_lite_path_should_not_classify_logs(tmp_path, monkeypatch):
    s = {
        "report": {"error_ids": ["a__a-1"], "unresolved_ids": ["b__b-2"]},
        "logs": {
            "a__a-1": {"run_instance.log": run_log("a__a-1", INFRA)},
            "b__b-2": {"run_instance.log": run_log("b__b-2", "Cannot allocate memory")},
        },
    }
    rc, camp, _ = run_fork(tmp_path, monkeypatch, [s], ("a__a-1", "b__b-2"), fork=False)
    assert outcomes(camp) == {"a__a-1": ("0", "error"), "b__b-2": ("0", "unresolved")}
    assert rc == 0


def test_k129_the_lite_path_should_keep_the_report_infra_lists(tmp_path, monkeypatch):
    s = {"report": {"infra_failure_ids": ["a__a-1"], "ambiguous_failure_ids": ["b__b-2"]}}
    _, camp, _ = run_fork(tmp_path, monkeypatch, [s], ("a__a-1", "b__b-2"), fork=False)
    assert outcomes(camp) == {"a__a-1": ("", "grader_infra"), "b__b-2": ("", "ambiguous")}


# ---- K130 / K135 ending every run ----


def attempt(kind, iid="a__a-1"):
    if kind == "resolved":
        return {"report": {"resolved_ids": [iid]}}
    return step(iid, {"infra": INFRA, "amb": AMB, "error": "AssertionError"}[kind])


def run_attempts(tmp_path, monkeypatch, kinds, ids=("a__a-1",)):
    """One main() call grades once and re-grades once; every later call re-grades once more.
    Returns (rc of the last call, campaign dir, the fake evaluator)."""
    steps = [attempt(k, ids[0]) for k in kinds]
    rc, camp, ev = run_fork(tmp_path, monkeypatch, steps, ids, "--regrade")
    for _ in range(max(0, len(kinds) - 2)):
        rc = grade.main([*grade_args(camp, tmp_path / "prereg"), "--regrade"])
    return rc, camp, ev


def test_k130_a_single_attempt_should_stay_ambiguous(tmp_path, monkeypatch):
    rc, camp, ev = run_fork(tmp_path, monkeypatch, [attempt("amb")])
    assert outcomes(camp)["a__a-1"] == ("", "ambiguous")
    assert rc == 1 and len(ev.cmds) == 1


@pytest.mark.parametrize(
    ("kinds", "want", "evaluations"),
    [
        (["amb", "amb"], ("0", "ambiguous_repeated"), 2),
        (["infra", "amb", "amb"], ("0", "ambiguous_repeated"), 3),  # K137
        (["infra", "infra", "amb", "amb"], ("0", "ambiguous_repeated"), 4),
        (["amb", "infra", "amb", "amb"], ("0", "ambiguous_repeated"), 4),
        (["infra", "amb"], ("", "ambiguous"), 2),  # one ambiguous attempt after the infra
        (["amb", "infra", "amb"], ("", "ambiguous"), 3),
        (["infra", "infra", "amb"], ("", "ambiguous"), 3),
        (["infra", "infra"], ("", "grader_infra"), 2),  # not final after 2 attempts (K135)
        (["infra", "infra", "amb", "infra"], ("", "grader_infra"), 4),  # not 3 in a row
        (["infra", "infra", "infra"], ("", "grader_infra_final"), 3),  # K137
        (["amb", "infra", "infra", "infra"], ("", "grader_infra_final"), 4),
        (["infra"] * 5, ("", "grader_infra_final"), 3),  # no fourth attempt (K135)
        (["infra", "infra", "resolved"], ("1", "resolved"), 3),
        (["amb", "error"], ("0", "error"), 2),
        (["infra", "error"], ("0", "error"), 2),
    ],
)
def test_k135_every_run_should_end_judged_or_final(tmp_path, monkeypatch, kinds, want, evaluations):
    _, camp, ev = run_attempts(tmp_path, monkeypatch, kinds)
    assert outcomes(camp)["a__a-1"] == want
    assert len(ev.cmds) == evaluations


def test_k135_a_final_status_should_not_be_regraded_again(tmp_path, monkeypatch):
    for kinds, final in (
        (["amb", "amb"], "ambiguous_repeated"),
        (["infra"] * 3, "grader_infra_final"),
    ):
        sub = tmp_path / final
        sub.mkdir()
        mp = pytest.MonkeyPatch()
        try:
            _, camp, ev = run_attempts(sub, mp, kinds)
            before = len(ev.cmds)
            for _ in range(2):
                grade.main([*grade_args(camp, sub / "prereg"), "--regrade"])
                grade.main(grade_args(camp, sub / "prereg"))
                assert outcomes(camp)["a__a-1"][1] == final
            assert len(ev.cmds) == before
        finally:
            mp.undo()


def test_k135_a_final_grader_failure_should_stay_blank_and_not_count_as_resolved(
    tmp_path, monkeypatch
):
    _, camp, _ = run_attempts(tmp_path, monkeypatch, ["infra"] * 3)
    assert outcomes(camp)["a__a-1"][0] == ""  # left out of pass@1, not scored 0


def test_k130_ambiguous_repeated_should_count_as_unresolved(tmp_path, monkeypatch):
    rc, camp, _ = run_attempts(tmp_path, monkeypatch, ["amb", "amb"])
    assert outcomes(camp)["a__a-1"] == ("0", "ambiguous_repeated")
    assert rc == 0


def test_k130_the_rule_should_apply_per_instance(tmp_path, monkeypatch):
    ids = ("a__a-1", "b__b-2")
    both = {
        "report": {"error_ids": list(ids)},
        "logs": {i: {"run_instance.log": run_log(i, AMB)} for i in ids},
    }
    second = {
        "report": {"error_ids": ["a__a-1"], "resolved_ids": ["b__b-2"]},
        "logs": {"a__a-1": {"run_instance.log": run_log("a__a-1", AMB)}},
    }
    _, camp, _ = run_fork(tmp_path, monkeypatch, [both, second], ids, "--regrade")
    assert outcomes(camp) == {"a__a-1": ("0", "ambiguous_repeated"), "b__b-2": ("1", "resolved")}


def test_k130_the_lite_path_should_not_make_ambiguous_final(tmp_path, monkeypatch):
    s = {"report": {"ambiguous_failure_ids": ["a__a-1"]}}
    rc, camp, _ = run_fork(tmp_path, monkeypatch, [s, s], ("a__a-1",), "--regrade", fork=False)
    assert outcomes(camp)["a__a-1"] == ("", "ambiguous")
    assert rc == 1


def test_k130_analysis_plan_should_state_both_rules():
    text = " ".join(str(v) for v in prereg.ANALYSIS_PLAN_A.values()).lower()
    assert "ambiguous_repeated" in text
    assert "grader_infra_final" in text
    assert "three" in text or "3" in text


# ---- K131 grade with the pre-registered fork ----

TOML_REF = "b" * 40


def k131_rig(tmp_path, *, toml_commit=TOML_REF, toml_ns="tomlns", env=None, toml=True):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    d = rig / "prereg" / "camp"
    d.mkdir(parents=True)
    (d / "selection.json").write_text(json.dumps({"grader": FORK}))
    (d / "dataset.json").write_text("[]")
    (d / "images.digests").write_text("")
    if toml:
        (d / "preregistration.toml").write_text(
            'stage = "a"\n\n[grader]\n'
            f'repo = "{FORK}"\ncommit = "{toml_commit}"\nnamespace = "{toml_ns}"\n'
        )
    envf = env_file_without(rig, "HC_A_GRADER_REF", "HC_A_NAMESPACE")
    full = {"FAKE_UV_STUB_MODULES": "hcbench.grade", "HC_ENV_FILE": str(envf), **(env or {})}
    r = run_script(rig, "20_grade.sh", ["camp"], bindir, full)
    return r, [c for c in calls(log) if c[0] == "uv"], d


def test_k131_unset_environment_should_use_the_toml_values(tmp_path):
    r, (argv,), _ = k131_rig(tmp_path)
    assert r.returncode == 0, r.stderr
    text = " ".join(argv)
    assert f"git+https://github.com/SWE-rebench/SWE-bench-fork.git@{TOML_REF}" in text
    assert argv[argv.index("--grader-commit") + 1] == TOML_REF
    assert argv[argv.index("--namespace") + 1] == "tomlns"


def test_k131_environment_values_equal_to_the_toml_should_pass(tmp_path):
    r, uv, _ = k131_rig(tmp_path, env={"HC_A_GRADER_REF": TOML_REF, "HC_A_NAMESPACE": "tomlns"})
    assert r.returncode == 0, r.stderr
    assert len(uv) == 1


def test_k131_a_differing_commit_should_stop_naming_both_values(tmp_path):
    other = "c" * 40
    r, uv, _ = k131_rig(tmp_path, env={"HC_A_GRADER_REF": other})
    assert r.returncode != 0
    out = r.stdout + r.stderr
    assert other in out and TOML_REF in out
    assert uv == []


def test_k131_a_differing_namespace_should_stop_naming_both_values(tmp_path):
    r, uv, _ = k131_rig(tmp_path, env={"HC_A_NAMESPACE": "otherns"})
    assert r.returncode != 0
    out = r.stdout + r.stderr
    assert "otherns" in out and "tomlns" in out
    assert uv == []


@pytest.mark.parametrize("bad", ["", "abc", TOML_REF.upper(), TOML_REF + "0", "main"])
def test_k131_a_toml_commit_that_is_not_40_lowercase_hex_should_be_refused(tmp_path, bad):
    r, uv, _ = k131_rig(tmp_path, toml_commit=bad)
    assert r.returncode != 0
    assert uv == []


# ---- K132 record what graded ----


def test_k132_grader_json_should_record_the_package_versions(tmp_path, monkeypatch):
    versions = {"swebench": "4.0.3", "docker": "7.1.0"}
    monkeypatch.setattr(grade, "package_version", lambda name: versions[name])
    camp = make_campaign(tmp_path, IDS3)
    d, _ = fork_inputs(tmp_path, IDS3)
    monkeypatch.setattr(grade.subprocess, "run", ForkEvaluate({"report": {"resolved_ids": IDS3}}))
    grade.main(grade_args(camp, d))
    rec = json.loads((camp / "grading" / "grader.json").read_text())
    assert rec["swebench_version"] == "4.0.3" and rec["docker_version"] == "7.1.0"
    assert rec["repo"] == FORK and rec["commit"] == REF and rec["namespace"] == NS
    assert "dataset_sha256" in rec


def test_k132_package_version_should_read_importlib_metadata(monkeypatch):
    monkeypatch.setattr(importlib.metadata, "version", lambda name: f"v-{name}")
    assert grade.package_version("swebench") == "v-swebench"
    assert grade.package_version("docker") == "v-docker"


# ---- K133 gaps the review found ----


@pytest.mark.parametrize(
    ("key", "value"),
    [
        ("HC_A_DATASET", "nebius/SWE-rebench-leaderboard "),
        ("HC_A_MIN_SPLIT", " 2026_03"),
        ("HC_A_MIN_CREATED", "2026-03-01 "),
        ("HC_SELECTION_SEED", " 20261109 "),
        ("HC_A_N", " 60 "),
        ("HC_A_GRADER", "SWE-rebench/SWE-bench-fork "),
    ],
)
def test_k133_preflight_should_refuse_a_padded_rule_value(stage_a_env, key, value):
    stage_a_env["env"]["mp"].setenv(key, value)
    with pytest.raises(SystemExit) as exc:
        preflight(stage_a_env)
    assert key in str(exc.value)


def test_k133_the_fork_argv_should_have_no_split(tmp_path, monkeypatch):
    camp = make_campaign(tmp_path, IDS3)
    d, _ = fork_inputs(tmp_path, IDS3)
    ev = ForkEvaluate({"report": {"resolved_ids": IDS3}})
    monkeypatch.setattr(grade.subprocess, "run", ev)
    grade.main(grade_args(camp, d))
    (cmd,) = ev.cmds
    assert "--split" not in cmd


@pytest.mark.parametrize("content", ["[]", '"text"', "3", "null", "[1, 2]"])
def test_k133_a_manifest_that_is_not_an_object_should_be_refused_in_stage_cb(env, content):
    pin(env)
    for k in ("HC_ARMS_MAIN", "HC_ARMS_REDTEAM", "HC_ARMS_B"):
        env["mp"].setenv(k, "pi")
    (env["tmp"] / "bundle" / "manifest.json").write_text(content)
    args = ["--stage", "cb", "--out", str(env["tmp"] / "p.toml"), "--env-file", str(EXAMPLE)]
    with pytest.raises(SystemExit) as exc:
        prereg.main(args)
    assert "manifest.json" in str(exc.value)


@pytest.mark.parametrize("content", ["[]", '"text"', "3", "null"])
def test_k133_a_manifest_that_is_not_an_object_should_be_refused_in_stage_a(stage_a_env, content):
    (stage_a_env["env"]["tmp"] / "bundle" / "manifest.json").write_text(content)
    with pytest.raises(SystemExit) as exc:
        run_a(stage_a_env)
    assert "manifest.json" in str(exc.value)
