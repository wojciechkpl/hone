"""Spec section 11.17, round 14, K93-K104: the SWE-rebench loader and the stage-a pre-registration.

Contract choices made here (the spec leaves them open; the implementer must honour them):

hcbench.rebench (pure)
* ``split_eligible(name, min_split) -> bool``
* ``created_date(value) -> str | None`` (the ten-character ISO date text, not a date object)
* ``eligible_rows(rows_by_split, min_split, min_created) -> list[dict]``; kept rows keep their
  other fields, get ``instance_id`` and ``repo`` as ``str`` and a ``split`` field
* ``choose_tasks(rows, n, seed) -> list[str]`` is ``hcbench.sampling.select_suite_a`` unchanged

hcbench.rebench (seams, module level, monkeypatched here; no test touches the network or Docker)
* ``resolve_revision(dataset, revision) -> str | None``  (None: cannot resolve)
* ``list_splits(dataset, sha) -> list[str]``
* ``load_split(dataset, split, sha) -> list[dict]``
* ``run_command(argv: list[str]) -> subprocess.CompletedProcess`` runs ``docker run ...``

CLI (``hcbench.rebench.main(argv)``)
* ``select --dataset D --revision R --min-split S --min-created DAY --n N --seed SEED
  --grader G --out DIR``
* ``check-images --dir DIR``: reads tasks.txt, instances.jsonl, selection.json and
  images.digests in DIR; rewrites tasks.txt and selection.json; writes images.check.json
  (``{task id: {"ok": bool, ...}}``)
* the container prints three blocks separated by a line ``---``: the HEAD of /testbed, the
  ``git status --porcelain`` lines, the Python version text.
* refusals are ``SystemExit`` with a message; nothing is written when a refusal happens

Stage a (hcbench.prereg)
* ``--stage a --selection DIR/selection.json --rules <cb toml> [--accept-diff KEY]...``
* ``[rules_check]`` holds the cb file's path, sha256 and repo head (key names are not pinned,
  the values are) and ``accepted`` (empty when nothing was accepted; otherwise it names each
  key with both values)
* ``files_sha256`` keys: prompt_swebench, tasks_txt, instances_jsonl, dataset_json,
  selection_json, images_digests, images_check_json (underscore form of the K98 file names)
* ``[task_created_at]``, ``[task_images]`` and ``[excluded_tasks]`` map task id to text
"""

import datetime
import hashlib
import importlib
import json
import platform
import re
import subprocess
import tomllib
from pathlib import Path

import pytest
from fakebin import calls, docker_calls, make_fake_bin, make_rig, run_script
from test_campaign1_prereg import EXAMPLE, ROOT, flat, pin, run_cb
from test_campaign1_prereg2 import squash
from test_campaign1_prereg_script import prereg_env

from hcbench import prereg, sampling
from hcbench.config import load_env_file

SHA = "34d5a588" + "0" * 32
DATASET = "nebius/SWE-rebench-leaderboard"
GRADER = "SWE-rebench/SWE-bench-fork"
K105_PATH = (
    "PATH=/opt/conda/envs/testbed/bin:/opt/conda/bin:/usr/local/sbin:/usr/local/bin:"
    "/usr/sbin:/usr/bin:/sbin:/bin:/lib/jvm/java-17-openjdk-amd64/bin"
)


def rebench():
    return importlib.import_module("hcbench.rebench")


def image_of(iid: str) -> str:
    return f"swerebench/sweb.eval.x86_64.{str(iid).replace('__', '_1776_')}:latest".lower()


def row(iid, repo, created="2026-03-22 09:18:02", **extra):
    r = {
        "instance_id": iid,
        "repo": repo,
        "base_commit": "c" * 40,
        "problem_statement": f"fix {iid}",
        "created_at": created,
        "image_name": image_of(iid),
        "docker_image": image_of(iid),
        "FAIL_TO_PASS": ["tests/t.py::a", "tests/t.py::b"],
        "PASS_TO_PASS": ["tests/t.py::c"],
        "install_config": {"test_cmd": "pytest", "log_parser": "parse_log_pytest"},
        "version": "unknown",
        "interface": "",
    }
    r.update(extra)
    return r


# ---- K93 pure functions ------------------------------------------------------------------


@pytest.mark.parametrize(
    ("name", "min_split", "want"),
    [
        ("2026_03", "2026_03", True),  # equal counts: kills a ">" for ">="
        ("2026_04", "2026_03", True),
        ("2027_01", "2026_03", True),
        ("2026_12", "2026_03", True),
        ("2026_02", "2026_03", False),
        ("2025_12", "2026_03", False),
        ("2026_13", "2026_03", False),  # not a month although it sorts later
        ("2026_00", "2025_01", False),
        ("test", "2026_03", False),  # "test" >= "2026_03" as text
        ("2026_3", "2026_03", False),
        (" 2026_03", "2026_03", False),
        ("2026_03\n", "2026_03", False),
        ("2026_031", "2026_03", False),
        ("x2026_03", "2026_03", False),
        ("2026-03", "2026_03", False),
        ("\u0662\u0660\u0662\u0666_03", "2026_03", False),  # non-ASCII digits
        ("", "2026_03", False),
    ],
)
def test_k93_split_eligible(name, min_split, want):
    assert rebench().split_eligible(name, min_split) is want


@pytest.mark.parametrize(
    ("value", "want"),
    [
        ("2026-03-22 09:18:02", "2026-03-22"),
        ("2026-03-22", "2026-03-22"),
        ("  2026-03-22T09:18:02Z ", "2026-03-22"),
        (datetime.datetime(2026, 3, 22, 9, 18, 2), "2026-03-22"),
        (datetime.date(2026, 3, 22), "2026-03-22"),
        (None, None),
        ("", None),
        ("   ", None),
        ("yesterday", None),
        ("2026-13-01 00:00:00", None),
        ("2026-02-30 00:00:00", None),
        ("26-03-22 09:18:02", None),
        ("2026/03/22", None),
        (20260322, None),
    ],
)
def test_k93_created_date(value, want):
    assert rebench().created_date(value) == want


def test_k93_eligible_rows_should_keep_equal_split_and_equal_date():
    rows = {
        "2026_03": [
            row("a__a-1", "a/a", "2026-03-01 00:00:00"),  # exactly min_created: kept
            row("b__b-2", "b/b", "2026-02-28 23:59:59"),  # a second before: dropped
            row("c__c-3", "c/c", "2026-03-02"),
        ],
        "2026_02": [row("d__d-4", "d/d", "2026-03-05 00:00:00")],  # older split: dropped
        "2026_04": [row("e__e-5", "e/e", "2026-04-01 00:00:00")],
    }
    got = rebench().eligible_rows(rows, "2026_03", "2026-03-01")
    assert [r["instance_id"] for r in got] == ["a__a-1", "c__c-3", "e__e-5"]
    assert [r["split"] for r in got] == ["2026_03", "2026_03", "2026_04"]


@pytest.mark.parametrize("bad", [None, "", "n/a", "soon", "2026-13-40"])
def test_k93_eligible_rows_should_drop_a_row_without_a_usable_creation_date(bad):
    rows = {"2026_03": [row("a__a-1", "a/a", bad), row("b__b-2", "b/b")]}
    got = rebench().eligible_rows(rows, "2026_03", "2026-03-01")
    assert [r["instance_id"] for r in got] == ["b__b-2"]


def test_k93_eligible_rows_should_take_a_datetime_creation_value():
    rows = {"2026_03": [row("a__a-1", "a/a", datetime.datetime(2026, 3, 22, 9, 18))]}
    assert len(rebench().eligible_rows(rows, "2026_03", "2026-03-01")) == 1


def test_k93_eligible_rows_should_ignore_the_test_split_and_malformed_split_names():
    rows = {
        "test": [row("a__a-1", "a/a")],
        "2026_3": [row("b__b-2", "b/b")],
        " 2026_03": [row("c__c-3", "c/c")],
        "2026_13": [row("d__d-4", "d/d")],
        "2026_03": [row("e__e-5", "e/e")],
    }
    got = rebench().eligible_rows(rows, "2026_03", "2026-03-01")
    assert [r["instance_id"] for r in got] == ["e__e-5"]


def test_k93_eligible_rows_should_coerce_id_and_repo_to_text_and_sort_by_id():
    rows = {
        "2026_04": [row(30, 7)],
        "2026_03": [row("9", "z/z"), row(100, "y/y")],
    }
    got = rebench().eligible_rows(rows, "2026_03", "2026-03-01")
    assert [r["instance_id"] for r in got] == ["100", "30", "9"]
    assert all(type(r["instance_id"]) is str and type(r["repo"]) is str for r in got)
    assert {r["repo"] for r in got} == {"7", "z/z", "y/y"}


def test_k93_eligible_rows_should_keep_the_other_fields():
    got = rebench().eligible_rows({"2026_03": [row("a__a-1", "a/a")]}, "2026_03", "2026-03-01")
    assert got[0]["base_commit"] == "c" * 40
    assert got[0]["FAIL_TO_PASS"] == ["tests/t.py::a", "tests/t.py::b"]
    assert got[0]["created_at"] == "2026-03-22 09:18:02"


def test_k93_eligible_rows_should_be_empty_when_nothing_qualifies():
    assert rebench().eligible_rows({"test": [row("a__a-1", "a/a")]}, "2026_03", "2026-03-01") == []


def realistic(total=110, repos=79):
    spec = {f"org/repo{r:02d}": 2 if r < total - repos else 1 for r in range(repos)}
    return [
        {"instance_id": f"{repo}__t{i}", "repo": repo} for repo, k in spec.items() for i in range(k)
    ]


@pytest.mark.parametrize("n", [60, 40, 79, 200])
def test_k95_choose_tasks_should_be_select_suite_a_unchanged(n):
    inst = realistic()
    assert rebench().choose_tasks(inst, n, 20261109) == sampling.select_suite_a(inst, n, 20261109)


def test_k95_choose_tasks_should_use_the_stratified_fallback_when_repos_are_fewer_than_n():
    inst = [
        {"instance_id": f"{r}__{i}", "repo": r} for r, k in {"a": 6, "b": 4, "c": 2}.items()
        for i in range(k)
    ]  # fmt: skip
    assert rebench().choose_tasks(inst, 7, 5) == sampling.stratified_sample(inst, 7, 5)


# ---- K94-K96 selection with the network seams faked ---------------------------------------


class Hub:
    """A fake dataset hub: records every seam call."""

    def __init__(self, splits, sha=SHA, resolves=True):
        self.splits, self.sha, self.resolves = splits, sha, resolves
        self.log = []

    def install(self, mp):
        rb = rebench()
        mp.setattr(rb, "resolve_revision", self.resolve_revision)
        mp.setattr(rb, "list_splits", self.list_splits)
        mp.setattr(rb, "load_split", self.load_split)

    def resolve_revision(self, dataset, revision):
        self.log.append(("resolve", dataset, revision))
        return self.sha if self.resolves else None

    def list_splits(self, dataset, sha):
        self.log.append(("list", dataset, sha))
        return list(self.splits)

    def load_split(self, dataset, split, sha):
        self.log.append(("load", dataset, split, sha))
        return [dict(r) for r in self.splits[split]]

    def loads(self):
        return [c for c in self.log if c[0] == "load"]


def population():
    """Eligible rows (min_split 2026_02, min_created 2026-02-10) over two splits plus noise."""
    s02 = [row(f"o{i}__r{i % 5}-{i}", f"o{i}/r{i % 5}", "2026-02-20 10:00:00") for i in range(6)]
    s02 += [row("old__old-1", "old/old", "2026-02-09 23:59:59")]  # before min_created
    s02 += [row("nodate__nd-1", "nd/nd", None)]  # no creation date: not eligible
    s03 = [row(f"p{i}__r{i % 3}-{i}", f"p{i}/r{i % 3}", "2026-03-22 09:18:02") for i in range(6)]
    s03 += [row("twin__t-1", "p0/r0", "2026-03-05 00:00:00")]  # a second task of a repo
    s25 = [row("ancient__a-1", "anc/a", "2025-12-30 00:00:00")]
    splits = {"2025_12": s25, "2026_02": s02, "2026_03": s03}
    splits["test"] = [r for rows in splits.values() for r in rows]  # the union
    eligible = [r for r in s02 + s03 if r["instance_id"] not in ("old__old-1", "nodate__nd-1")]
    return splits, eligible


def select_args(out, *, n=4, seed=20261109, revision="", min_split="2026_02", **kw):
    args = {
        "dataset": DATASET,
        "revision": revision,
        "min-split": min_split,
        "min-created": "2026-02-10",
        "n": str(n),
        "seed": str(seed),
        "grader": GRADER,
        "out": str(out),
    }
    args.update({k.replace("_", "-"): str(v) for k, v in kw.items()})
    return ["select"] + [x for k, v in args.items() for x in (f"--{k}", v)]


@pytest.fixture
def hub(monkeypatch):
    splits, eligible = population()
    h = Hub(splits)
    h.eligible = eligible
    h.install(monkeypatch)
    return h


def do_select(out, **kw):
    return rebench().main(select_args(out, **kw))


def test_k95_tasks_txt_should_hold_the_chosen_ids_sorted(hub, tmp_path):
    assert do_select(tmp_path / "s", n=4) == 0
    want = sampling.select_suite_a(hub.eligible, 4, 20261109)
    assert (tmp_path / "s" / "tasks.txt").read_text() == "\n".join(want) + "\n"
    assert len(want) == 4


def test_k95_selection_should_not_depend_on_the_order_the_hub_returns_rows(hub, tmp_path):
    do_select(tmp_path / "a", n=4)
    hub.splits = {k: list(reversed(v)) for k, v in reversed(list(hub.splits.items()))}
    do_select(tmp_path / "b", n=4)
    assert (tmp_path / "a" / "tasks.txt").read_text() == (tmp_path / "b" / "tasks.txt").read_text()


def test_k95_instances_jsonl_should_hold_exactly_the_named_fields(hub, tmp_path):
    do_select(tmp_path / "s", n=100)  # n above the eligible count: every eligible task
    lines = (tmp_path / "s" / "instances.jsonl").read_text().splitlines()
    recs = [json.loads(ln) for ln in lines]
    assert [r["instance_id"] for r in recs] == sorted(r["instance_id"] for r in hub.eligible)
    by_id = {r["instance_id"]: r for r in hub.eligible}
    for rec in recs:
        src = by_id[rec["instance_id"]]
        assert set(rec) == {
            "instance_id", "repo", "base_commit", "problem_statement", "created_at", "split",
            "image",
        }  # fmt: skip
        assert rec["created_at"] == src["created_at"]  # text as published
        assert rec["image"] == src["image_name"]
        assert rec["problem_statement"] == src["problem_statement"]
    assert {r["instance_id"]: r["split"] for r in recs}["p1__r1-1"] == "2026_03"


def test_k95_image_should_fall_back_to_docker_image_when_image_name_is_empty(hub, tmp_path):
    hub.splits["2026_03"][0]["image_name"] = ""
    hub.splits["2026_03"][0]["docker_image"] = "swerebench/only-docker:latest"
    do_select(tmp_path / "s", n=100)
    recs = {
        json.loads(ln)["instance_id"]: json.loads(ln)
        for ln in (tmp_path / "s" / "instances.jsonl").read_text().splitlines()
    }
    assert recs[hub.splits["2026_03"][0]["instance_id"]]["image"] == "swerebench/only-docker:latest"


def test_k95_dataset_json_should_hold_the_full_chosen_rows_with_text_ids(hub, tmp_path):
    hub.splits["2026_03"].append(row(4242, 99, "2026-03-10 00:00:00"))
    do_select(tmp_path / "s", n=100)
    rows = json.loads((tmp_path / "s" / "dataset.json").read_text())
    by_id = {r["instance_id"]: r for r in rows}
    assert "4242" in by_id and by_id["4242"]["repo"] == "99"
    assert len(rows) == len(hub.eligible) + 1
    src = next(r for r in hub.eligible if r["instance_id"] == "o1__r1-1")
    for key, value in src.items():
        assert (
            by_id["o1__r1-1"][key] == value
        )  # FAIL_TO_PASS stays a list, install_config an object
    assert "4242" in (tmp_path / "s" / "tasks.txt").read_text().splitlines()


def test_k95_selection_json_should_record_the_selection(hub, tmp_path):
    do_select(tmp_path / "s", n=4, revision="main")
    sel = json.loads((tmp_path / "s" / "selection.json").read_text())
    chosen = sampling.select_suite_a(hub.eligible, 4, 20261109)
    ids = [r["instance_id"] for r in hub.eligible]
    assert sel["dataset"] == DATASET
    assert sel["revision"] == "main"
    assert sel["resolved_revision"] == SHA
    assert sel["splits"] == ["2026_02", "2026_03"]
    assert sel["min_split"] == "2026_02"
    assert sel["min_created"] == "2026-02-10"
    assert sel["n"] == 4 and sel["seed"] == 20261109
    assert sel["method"] == "hcbench.sampling.select_suite_a"
    assert sel["eligible_count"] == len(ids)
    assert sel["eligible_repos"] == len({r["repo"] for r in hub.eligible})
    assert sel["eligible_digest"] == sampling.manifest_digest(ids)
    assert sel["digest"] == sampling.manifest_digest(chosen)
    assert sel["python_version"] == platform.python_version()
    assert sel["python_implementation"] == platform.python_implementation()
    assert sel["grader"] == GRADER
    assert re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ", sel["created_at"])


def test_k95_selection_should_record_the_revision_as_requested_when_empty(hub, tmp_path):
    do_select(tmp_path / "s", revision="")
    sel = json.loads((tmp_path / "s" / "selection.json").read_text())
    assert sel["revision"] == ""
    assert sel["resolved_revision"] == SHA


def test_k96_should_resolve_the_revision_first_and_load_every_eligible_split_at_the_sha(
    hub, tmp_path
):
    do_select(tmp_path / "s", revision="main")
    assert hub.log[0] == ("resolve", DATASET, "main")
    assert ("list", DATASET, SHA) in hub.log
    assert hub.log.index(("list", DATASET, SHA)) < hub.log.index(hub.loads()[0])
    assert sorted(hub.loads()) == [
        ("load", DATASET, "2026_02", SHA),
        ("load", DATASET, "2026_03", SHA),
    ]  # not test, not 2025_12; never an unpinned load


def test_k96_should_stop_when_the_revision_cannot_be_resolved(monkeypatch, tmp_path):
    splits, _ = population()
    h = Hub(splits, resolves=False)
    h.install(monkeypatch)
    with pytest.raises(SystemExit) as exc:
        do_select(tmp_path / "s", revision="main")
    assert "revision" in str(exc.value).lower()
    assert [c[0] for c in h.log] == ["resolve"]  # nothing listed or loaded, no unpinned load
    assert not (tmp_path / "s" / "tasks.txt").exists()


def test_k94_should_refuse_when_no_task_is_eligible(hub, tmp_path):
    with pytest.raises(SystemExit) as exc:
        do_select(tmp_path / "s", revision="main", min_split="2027_01")
    msg = str(exc.value)
    assert DATASET in msg
    assert "main" in msg or SHA in msg
    assert "2027_01" in msg and "2026-02-10" in msg
    assert not (tmp_path / "s" / "tasks.txt").exists()


def test_k94_should_not_take_the_test_split_as_eligible(monkeypatch, tmp_path):
    h = Hub({"test": [row("a__a-1", "a/a")]})
    h.install(monkeypatch)
    with pytest.raises(SystemExit):
        do_select(tmp_path / "s", min_split="2026_03")
    assert h.loads() == []


def test_k94_should_refuse_an_id_in_two_eligible_splits(hub, tmp_path):
    dup = dict(hub.splits["2026_03"][0], instance_id=hub.splits["2026_02"][0]["instance_id"])
    hub.splits["2026_03"].append(dup)
    with pytest.raises(SystemExit) as exc:
        do_select(tmp_path / "s", n=2)
    msg = str(exc.value)
    assert dup["instance_id"] in msg and "2026_02" in msg and "2026_03" in msg
    assert not (tmp_path / "s" / "tasks.txt").exists()


def test_k94_the_test_split_union_should_not_count_as_a_duplicate(hub, tmp_path):
    assert do_select(tmp_path / "s", n=2) == 0  # every id is also in the test split


@pytest.mark.parametrize("empty", ["", None])
def test_k94_should_refuse_an_eligible_row_without_an_image(hub, tmp_path, empty):
    bad = hub.splits["2026_03"][1]
    bad["image_name"] = bad["docker_image"] = empty
    with pytest.raises(SystemExit) as exc:
        do_select(tmp_path / "s", n=1)  # refused even when the row is not drawn
    assert bad["instance_id"] in str(exc.value)
    assert "image" in str(exc.value).lower()


def test_k94_should_refuse_differing_image_name_and_docker_image(hub, tmp_path):
    bad = hub.splits["2026_02"][2]
    bad["docker_image"] = "swerebench/other:latest"
    with pytest.raises(SystemExit) as exc:
        do_select(tmp_path / "s", n=1)
    assert bad["instance_id"] in str(exc.value)


def test_k94_an_ineligible_row_without_an_image_should_not_stop_the_selection(hub, tmp_path):
    hub.splits["2026_02"][6]["image_name"] = hub.splits["2026_02"][6]["docker_image"] = ""
    assert do_select(tmp_path / "s", n=2) == 0  # old__old-1 is before min_created


# ---- K104 image check --------------------------------------------------------------------


def pinned(iid):
    n = int(hashlib.sha256(iid.encode()).hexdigest()[:8], 16)
    return image_of(iid).rsplit(":", 1)[0] + "@sha256:" + f"{n:064x}"


def make_sel_dir(d: Path, ids, drawn=None):
    d.mkdir(parents=True, exist_ok=True)
    drawn = list(drawn or ids)
    (d / "tasks.txt").write_text("\n".join(sorted(ids)) + "\n")
    (d / "instances.jsonl").write_text(
        "".join(
            json.dumps(
                {
                    "instance_id": i,
                    "repo": i.split("__")[0],
                    "base_commit": f"{k:040x}",
                    "problem_statement": "p",
                    "created_at": "2026-03-22 09:18:02",
                    "split": "2026_03",
                    "image": image_of(i),
                }
            )
            + "\n"
            for k, i in enumerate(sorted(drawn), start=1)
        )
    )
    (d / "images.digests").write_text(
        "".join(f"{image_of(i)} {pinned(i)}\n" for i in sorted(drawn))
    )
    (d / "selection.json").write_text(
        json.dumps({"dataset": DATASET, "n": len(drawn), "digest": sampling.manifest_digest(drawn)})
    )
    return {i: f"{k:040x}" for k, i in enumerate(sorted(drawn), start=1)}


class Docker:
    """A fake `docker run`: maps a pinned reference to (returncode, stdout)."""

    def __init__(self, results):
        self.results = results
        self.argvs = []

    def __call__(self, argv):
        self.argvs.append(list(argv))
        ref = argv[argv.index("bash") + 1]
        rc, out = self.results[ref]
        return subprocess.CompletedProcess(argv, rc, stdout=out, stderr="")


def good(head, py="3.13.2 (main) [GCC 12]"):
    return 0, f"{head}\n---\n---\n{py}\n"


IDS = ["a__a-1", "b__b-2", "c__c-3", "d__d-4"]


def check(d, monkeypatch, docker):
    monkeypatch.setattr(rebench(), "run_command", docker)
    return rebench().main(["check-images", "--dir", str(d)])


def test_k104_should_run_each_pinned_image_once_without_network(tmp_path, monkeypatch):
    heads = make_sel_dir(tmp_path, IDS)
    docker = Docker({pinned(i): good(heads[i]) for i in IDS})
    assert check(tmp_path, monkeypatch, docker) == 0
    assert sorted(a[a.index("bash") + 1] for a in docker.argvs) == sorted(pinned(i) for i in IDS)
    for argv in docker.argvs:
        assert argv[:7] == ["docker", "run", "--rm", "--network", "none", "--entrypoint", "bash"]
        assert argv[8] == "-c"
        script = argv[9]
        assert "git status --porcelain" in script
        assert "/testbed" in script
        assert "sys.version" in script
        assert K105_PATH in script


def test_k104_all_passing_should_keep_the_list_and_record_an_empty_exclusion(tmp_path, monkeypatch):
    heads = make_sel_dir(tmp_path, IDS)
    before = (tmp_path / "tasks.txt").read_text()
    check(tmp_path, monkeypatch, Docker({pinned(i): good(heads[i]) for i in IDS}))
    assert (tmp_path / "tasks.txt").read_text() == before
    sel = json.loads((tmp_path / "selection.json").read_text())
    assert sel["excluded"] == {}
    assert sel["digest"] == sampling.manifest_digest(IDS)
    assert sel["run_digest"] == sel["digest"]
    res = json.loads((tmp_path / "images.check.json").read_text())
    assert set(res) == set(IDS)
    assert all(res[i]["ok"] is True for i in IDS)


def test_k104_a_failing_task_should_be_excluded_with_the_failed_part(tmp_path, monkeypatch):
    heads = make_sel_dir(tmp_path, IDS)
    docker = Docker(
        {
            pinned("a__a-1"): good(heads["a__a-1"]),
            pinned("b__b-2"): good("f" * 40),  # HEAD is not the base commit
            pinned("c__c-3"): (0, f"{heads['c__c-3']}\n---\n M setup.py\n---\n3.13.2\n"),
            pinned("d__d-4"): (0, f"{heads['d__d-4']}\n---\n---\n"),  # Python does not run
        }
    )
    assert check(tmp_path, monkeypatch, docker) == 0
    assert (tmp_path / "tasks.txt").read_text() == "a__a-1\n"
    sel = json.loads((tmp_path / "selection.json").read_text())
    assert sel["digest"] == sampling.manifest_digest(IDS)  # the drawn list stays
    assert sel["run_digest"] == sampling.manifest_digest(["a__a-1"])
    assert set(sel["excluded"]) == {"b__b-2", "c__c-3", "d__d-4"}
    assert "head" in sel["excluded"]["b__b-2"].lower()
    assert "status" in sel["excluded"]["c__c-3"].lower()
    assert "python" in sel["excluded"]["d__d-4"].lower()
    res = json.loads((tmp_path / "images.check.json").read_text())
    assert set(res) == set(IDS)  # every result, passing or not
    assert res["a__a-1"]["ok"] is True and res["b__b-2"]["ok"] is False


def test_k115_docker_exit_125_should_stop_the_check_and_write_nothing(tmp_path, monkeypatch):
    # Round 14b (K115) replaces the round 14 reading, where 125 excluded the task: 125 is
    # Docker's own error, and an exclusion is final.
    heads = make_sel_dir(tmp_path, IDS[:2])
    before = {n: (tmp_path / n).read_bytes() for n in ("tasks.txt", "selection.json")}
    docker = Docker({pinned("a__a-1"): good(heads["a__a-1"]), pinned("b__b-2"): (125, "")})
    with pytest.raises(SystemExit) as exc:
        check(tmp_path, monkeypatch, docker)
    assert "b__b-2" in str(exc.value)
    assert {n: (tmp_path / n).read_bytes() for n in before} == before
    assert not (tmp_path / "images.check.json").exists()


def test_k104_should_not_touch_a_task_by_its_run_outcome(tmp_path, monkeypatch):
    # Only the image is inspected: the check reads no results directory and starts no agent.
    heads = make_sel_dir(tmp_path, IDS[:1])
    docker = Docker({pinned("a__a-1"): good(heads["a__a-1"])})
    check(tmp_path, monkeypatch, docker)
    assert len(docker.argvs) == 1


# ---- stage a: pre-registration -----------------------------------------------------------

STAGE_IDS = ["django__django-1", "psf__requests-22", "a__b-333"]


def write_selection(d: Path, rules: dict, run=None, drawn=None, excluded=None, digest_lines=None):
    """Files `rebench select` and `rebench check-images` would leave behind."""
    d.mkdir(parents=True, exist_ok=True)
    drawn = sorted(drawn or STAGE_IDS)
    run = sorted(run or drawn)
    (d / "tasks.txt").write_text("\n".join(run) + "\n")
    (d / "instances.jsonl").write_text(
        "".join(
            json.dumps(
                {
                    "instance_id": i,
                    "repo": i.split("__")[0],
                    "base_commit": "c" * 40,
                    "problem_statement": "p",
                    "created_at": f"2026-03-{k + 10:02d} 09:18:02",
                    "split": "2026_03",
                    "image": image_of(i),
                }
            )
            + "\n"
            for k, i in enumerate(drawn)
        )
    )
    (d / "dataset.json").write_text(json.dumps([row(i, i.split("__")[0]) for i in run]))
    lines = digest_lines or [f"{image_of(i)} {pinned(i)}" for i in drawn]
    (d / "images.digests").write_text("\n".join(lines) + "\n")
    (d / "images.check.json").write_text(json.dumps({i: {"ok": i in run} for i in drawn}))
    sel = {
        "dataset": rules["dataset"],
        "revision": "",
        "resolved_revision": SHA,
        "splits": ["2026_03"],
        "min_split": rules["min_split"],
        "min_created": rules["min_created"],
        "n": rules["n"],
        "seed": rules["selection_seed"],
        "method": "hcbench.sampling.select_suite_a",
        "eligible_count": 110,
        "eligible_repos": 79,
        "eligible_digest": "e" * 64,
        "digest": sampling.manifest_digest(drawn),
        "python_version": "3.12.1",
        "python_implementation": "CPython",
        "grader": rules["grader"],
        "created_at": "2026-12-01T10:00:00Z",
        "excluded": excluded or {},
        "run_digest": sampling.manifest_digest(run),
    }
    (d / "selection.json").write_text(json.dumps(sel, indent=1))
    return sel


@pytest.fixture
def stage_a_env(env):
    pin(env)
    rc, cb = run_cb(env)
    assert rc == 0
    rules = tomllib.loads(cb.read_text())["suite_a_rules"]
    sel_dir = env["tmp"] / "sel"
    sel = write_selection(sel_dir, rules)
    return {"env": env, "cb": cb, "rules": rules, "dir": sel_dir, "sel": sel}


def run_a(s, *extra, cb=None, out=None, rules=True):
    out = out or s["env"]["tmp"] / "a.toml"
    args = ["--stage", "a", "--selection", str(s["dir"] / "selection.json")]
    if rules:
        args += ["--rules", str(cb or s["cb"])]
    args += ["--out", str(out), "--env-file", str(EXAMPLE), *extra]
    return prereg.main(args), out


def stage_a_data(s, *extra):
    rc, out = run_a(s, *extra)
    assert rc == 0
    return tomllib.loads(out.read_text()), out


def rewrite_selection(s, **changes):
    p = s["dir"] / "selection.json"
    data = json.loads(p.read_text())
    data.update(changes)
    p.write_text(json.dumps(data))


# K97 --------------------------------------------------------------------------------------


def test_k97_rules_should_be_required(stage_a_env, capsys):
    with pytest.raises(SystemExit):
        run_a(stage_a_env, rules=False)
    assert "--rules" in capsys.readouterr().err


def test_k97_a_matching_stage_cb_file_should_be_accepted(stage_a_env):
    rc, out = run_a(stage_a_env)
    assert rc == 0 and out.exists()
    assert tomllib.loads(out.read_text())["stage"] == "a"


def test_k97_should_refuse_a_missing_rules_file(stage_a_env):
    with pytest.raises(SystemExit) as exc:
        run_a(stage_a_env, cb=stage_a_env["env"]["tmp"] / "nope.toml")
    assert "rules" in str(exc.value).lower()
    assert not (stage_a_env["env"]["tmp"] / "a.toml").exists()


def test_k97_should_refuse_a_rules_file_that_is_not_stage_cb(stage_a_env):
    other = stage_a_env["env"]["tmp"] / "other.toml"
    other.write_text(stage_a_env["cb"].read_text().replace('stage = "cb"', 'stage = "a"'))
    with pytest.raises(SystemExit) as exc:
        run_a(stage_a_env, cb=other)
    assert "cb" in str(exc.value)


def test_k97_should_refuse_a_draft_rules_file(stage_a_env):
    env = stage_a_env["env"]
    rc, draft = run_cb(env, "--draft")
    assert rc == 0 and tomllib.loads(draft.read_text())["draft"] is True
    draft_copy = env["tmp"] / "draft.toml"
    draft_copy.write_text(draft.read_text())
    with pytest.raises(SystemExit) as exc:
        run_a(stage_a_env, cb=draft_copy)
    assert "draft" in str(exc.value).lower()


@pytest.mark.parametrize(
    ("key", "field", "bad"),
    [
        ("dataset", "dataset", "other/dataset"),
        ("min_split", "min_split", "2026_04"),
        ("min_created", "min_created", "2026-04-01"),
        ("n", "n", 40),
        ("seed", "seed", 1),
        ("grader", "grader", "someone/else"),
    ],
)
def test_k97_selection_that_differs_from_the_rules_should_be_refused(stage_a_env, key, field, bad):
    rewrite_selection(stage_a_env, **{field: bad})
    with pytest.raises(SystemExit) as exc:
        run_a(stage_a_env)
    assert key in str(exc.value) or field in str(exc.value)
    # Nothing overrides a rules difference: --accept-diff does not let it through.
    with pytest.raises(SystemExit):
        run_a(stage_a_env, "--accept-diff", key, "--accept-diff", field)
    assert not (stage_a_env["env"]["tmp"] / "a.toml").exists()


def test_k97_a_config_difference_should_be_refused_and_named(stage_a_env):
    stage_a_env["env"]["mp"].setenv("HC_TOP_K", "21")
    with pytest.raises(SystemExit) as exc:
        run_a(stage_a_env)
    assert "HC_TOP_K" in str(exc.value)


def test_k97_accepting_the_named_difference_should_record_both_values(stage_a_env):
    cb_top_k = tomllib.loads(stage_a_env["cb"].read_text())["config"]["HC_TOP_K"]
    stage_a_env["env"]["mp"].setenv("HC_TOP_K", "21")
    data, _ = stage_a_data(stage_a_env, "--accept-diff", "HC_TOP_K")
    accepted = flat(data["rules_check"]["accepted"])
    assert "HC_TOP_K" in accepted and "21" in accepted and cb_top_k in accepted
    assert data["config"]["HC_TOP_K"] == "21"


def test_k97_accepting_another_key_should_not_let_the_difference_through(stage_a_env):
    stage_a_env["env"]["mp"].setenv("HC_TOP_K", "21")
    with pytest.raises(SystemExit) as exc:
        run_a(stage_a_env, "--accept-diff", "HC_MIN_P")
    assert "HC_TOP_K" in str(exc.value)


def test_k97_accept_diff_may_repeat_and_each_key_is_recorded(stage_a_env):
    mp = stage_a_env["env"]["mp"]
    mp.setenv("HC_TOP_K", "21")
    mp.setenv("HC_MIN_P", "0.1")
    data, _ = stage_a_data(stage_a_env, "--accept-diff", "HC_TOP_K", "--accept-diff", "HC_MIN_P")
    accepted = flat(data["rules_check"]["accepted"])
    assert "HC_TOP_K" in accepted and "HC_MIN_P" in accepted


def test_k97_a_bundle_difference_should_be_refused_and_accepted_by_name(stage_a_env):
    manifest = stage_a_env["env"]["tmp"] / "bundle" / "manifest.json"
    manifest.write_text(json.dumps({**stage_a_env["env"]["manifest"], "x": 2}))
    with pytest.raises(SystemExit) as exc:
        run_a(stage_a_env)
    assert "bundle" in str(exc.value)
    data, _ = stage_a_data(stage_a_env, "--accept-diff", "bundle")
    assert "bundle" in flat(data["rules_check"]["accepted"])
    assert data["bundle"]["x"] == 2


def test_k97_nothing_accepted_should_leave_accepted_empty(stage_a_env):
    data, _ = stage_a_data(stage_a_env)
    assert not data["rules_check"]["accepted"]


def test_k97_a_placeholder_in_a_non_draft_stage_a_should_be_refused(stage_a_env):
    stage_a_env["env"]["mp"].setenv("HC_MODEL_REVISION", "CHANGE-ME-commit-sha")
    with pytest.raises(SystemExit) as exc:
        run_a(stage_a_env)
    assert "HC_MODEL_REVISION" in str(exc.value)


def test_k97_draft_should_be_recorded_for_a_draft_stage_a(stage_a_env):
    data, _ = stage_a_data(stage_a_env, "--draft")
    assert data["draft"] is True


# K98 --------------------------------------------------------------------------------------


def test_k98_stage_a_should_keep_what_every_stage_records(stage_a_env):
    data, _ = stage_a_data(stage_a_env)
    assert data["stage"] == "a"
    for key in ("title", "created_at", "draft", "repo", "config", "bundle", "files_sha256"):
        assert key in data
    assert data["draft"] is False
    assert data["title"] == "Hone vs Pi vs Hermes Agent, harness-controlled, one local model"
    for key in ("suites_in_scope", "suite_a_rules", "cut_order"):
        assert key not in data


def test_k98_rules_check_should_hold_the_path_sha_and_head_of_the_cb_file(stage_a_env):
    cb_path = stage_a_env["cb"]
    cb = tomllib.loads(cb_path.read_text())
    data, _ = stage_a_data(stage_a_env)
    values = [str(v) for v in data["rules_check"].values()]
    assert str(cb_path) in values
    assert hashlib.sha256(cb_path.read_bytes()).hexdigest() in values
    assert cb["repo"]["head"] and cb["repo"]["head"] in values


def test_k98_tasks_table_should_hold_the_selection_keys_and_the_run_digest(stage_a_env):
    sel = stage_a_env["sel"]
    data, _ = stage_a_data(stage_a_env)
    tasks = data["tasks"]
    for key in sel:
        if key in ("created_at", "excluded"):
            continue
        assert tasks[key] == sel[key], key
    assert "created_at" not in tasks
    assert tasks["run_digest"] == sel["run_digest"]
    assert tasks["method"] == "hcbench.sampling.select_suite_a"
    assert tasks["python_version"] == "3.12.1"


def test_k98_task_created_at_and_task_images_should_map_every_task(stage_a_env):
    data, _ = stage_a_data(stage_a_env)
    recs = {
        json.loads(ln)["instance_id"]: json.loads(ln)
        for ln in (stage_a_env["dir"] / "instances.jsonl").read_text().splitlines()
    }
    assert data["task_created_at"] == {i: recs[i]["created_at"] for i in STAGE_IDS}
    assert data["task_images"] == {i: pinned(i) for i in STAGE_IDS}
    assert data["excluded_tasks"] == {}


def test_k98_excluded_tasks_should_map_each_excluded_id_to_its_reason(stage_a_env):
    s = stage_a_env
    drawn = [*STAGE_IDS, "x__y-9"]
    write_selection(
        s["dir"], s["rules"], run=STAGE_IDS, drawn=drawn, excluded={"x__y-9": "head differs"}
    )
    data, _ = stage_a_data(s)
    assert data["excluded_tasks"] == {"x__y-9": "head differs"}
    sel = json.loads((s["dir"] / "selection.json").read_text())
    assert data["tasks"]["digest"] == sel["digest"] != sel["run_digest"]
    assert data["tasks"]["run_digest"] == sel["run_digest"]
    for i in STAGE_IDS:
        assert i in data["task_images"] and i in data["task_created_at"]


def test_k98_grader_table_should_come_from_the_new_keys(stage_a_env):
    vals = load_env_file(EXAMPLE, base={"HOME": "/h"})
    data, _ = stage_a_data(stage_a_env)
    assert data["grader"] == {
        "repo": vals["HC_A_GRADER"],
        "commit": vals["HC_A_GRADER_REF"],
        "namespace": vals["HC_A_NAMESPACE"],
    }


def test_k98_grader_table_should_follow_the_environment(stage_a_env):
    mp = stage_a_env["env"]["mp"]
    mp.setenv("HC_A_GRADER_REF", "ab" * 20)
    mp.setenv("HC_A_NAMESPACE", "other")
    data, _ = stage_a_data(stage_a_env)
    assert data["grader"]["commit"] == "ab" * 20 and data["grader"]["namespace"] == "other"


def test_k98_files_sha256_should_cover_the_stage_a_files(stage_a_env):
    d = stage_a_env["dir"]
    data, _ = stage_a_data(stage_a_env)
    expected = {
        "arms_toml": ROOT / "config" / "arms.toml",
        "prompt_swebench": ROOT / "config" / "prompts" / "swebench.md",
        "tasks_txt": d / "tasks.txt",
        "instances_jsonl": d / "instances.jsonl",
        "dataset_json": d / "dataset.json",
        "selection_json": d / "selection.json",
        "images_digests": d / "images.digests",
        "images_check_json": d / "images.check.json",
    }
    for key, path in expected.items():
        assert data["files_sha256"][key] == hashlib.sha256(path.read_bytes()).hexdigest(), key


def test_k98_config_should_record_the_new_suite_a_keys(stage_a_env):
    vals = load_env_file(EXAMPLE, base={"HOME": "/h"})
    data, _ = stage_a_data(stage_a_env)
    assert data["config"]["HC_A_GRADER_REF"] == vals["HC_A_GRADER_REF"]
    assert data["config"]["HC_A_NAMESPACE"] == vals["HC_A_NAMESPACE"]
    assert data["config"]["HC_A_REVISION"] == ""


def test_k98_nothing_in_the_file_should_say_lite_n40_or_the_official_harness(stage_a_env):
    _, out = stage_a_data(stage_a_env)
    text = out.read_text().lower()
    for stale in ("n=40", "swe-bench lite", "official swebench harness"):
        assert stale not in text, stale


def test_k100_stage_a_should_not_read_the_swe_keys(stage_a_env):
    base, _ = stage_a_data(stage_a_env)
    mp = stage_a_env["env"]["mp"]
    for key, value in (("HC_SWE_N", "7"), ("HC_SWE_DATASET", "x/y"), ("HC_SWE_SPLIT", "val")):
        mp.setenv(key, value)
    other, out = stage_a_data(stage_a_env, "--draft")
    assert "HC_SWE" not in out.read_text()
    assert other["tasks"] == base["tasks"]


# K99 --------------------------------------------------------------------------------------


def test_k99_example_should_define_the_new_keys():
    vals = load_env_file(EXAMPLE, base={"HOME": "/h"})
    assert vals["HC_A_REVISION"] == ""
    assert vals["HC_A_GRADER_REF"] == "e4907b7a90eafaa1f0a6428fd04fe31cdd8b4284"
    assert vals["HC_A_NAMESPACE"] == "swerebench"


@pytest.mark.parametrize(
    "ref",
    ["", "e4907b7", "E4907B7A90EAFAA1F0A6428FD04FE31CDD8B4284", "e" * 41, "g" * 40, "main"],
)
def test_k99_a_bad_grader_ref_should_be_refused_in_a_non_draft_stage_a(stage_a_env, ref):
    stage_a_env["env"]["mp"].setenv("HC_A_GRADER_REF", ref)
    with pytest.raises(SystemExit) as exc:
        run_a(stage_a_env)
    assert "HC_A_GRADER_REF" in str(exc.value)


def test_k99_an_empty_namespace_should_be_refused_in_a_non_draft_stage_a(stage_a_env):
    stage_a_env["env"]["mp"].setenv("HC_A_NAMESPACE", "")
    with pytest.raises(SystemExit) as exc:
        run_a(stage_a_env)
    assert "HC_A_NAMESPACE" in str(exc.value)


# K103 -------------------------------------------------------------------------------------


def bad_lines(kind: str) -> list[str]:
    ok = [f"{image_of(i)} {pinned(i)}" for i in STAGE_IDS]
    bad = {
        "missing": f"{image_of(STAGE_IDS[0])} MISSING",
        "short": f"{image_of(STAGE_IDS[0])} swerebench/x@sha256:{'a' * 63}",
        "upper": f"{image_of(STAGE_IDS[0])} swerebench/x@sha256:{'A' * 64}",
        "tag": f"{image_of(STAGE_IDS[0])} {image_of(STAGE_IDS[0])}",
        "blank": f"{image_of(STAGE_IDS[0])} ",
    }[kind]
    return [bad, *ok[1:]]


@pytest.mark.parametrize("kind", ["missing", "short", "upper", "tag", "blank"])
def test_k103_a_missing_or_malformed_digest_line_should_be_refused_and_named(stage_a_env, kind):
    s = stage_a_env
    write_selection(s["dir"], s["rules"], digest_lines=bad_lines(kind))
    with pytest.raises(SystemExit) as exc:
        run_a(s)
    assert image_of(STAGE_IDS[0]) in str(exc.value)


def test_k103_every_bad_line_should_be_named(stage_a_env):
    s = stage_a_env
    lines = [f"{image_of(i)} MISSING" for i in STAGE_IDS]
    write_selection(s["dir"], s["rules"], digest_lines=lines)
    with pytest.raises(SystemExit) as exc:
        run_a(s)
    for i in STAGE_IDS:
        assert image_of(i) in str(exc.value)


# K101 -------------------------------------------------------------------------------------


@pytest.mark.parametrize(
    "needle",
    [
        "pass@1",
        "swe-rebench",
        "hone-inject - pi",
        "hone-inject - hermes",
        "hone-inject - hone-noinject",
        "pi - hermes",
        "total tokens",
        "wall",
        "holm",
        "10,000",
        "seed 0",
        "mcnemar",
        "repeat 0",
        "0.05",
        "no detectable difference",
        "per-task mean",
        "not replaced",
        "contaminated",
        "re-run",
        "re-graded",
    ],
)
def test_k101_stage_a_analysis_should_state(stage_a_env, needle):
    data, _ = stage_a_data(stage_a_env)
    assert needle in squash(data["analysis"]), needle


def test_k101_analysis_should_not_carry_the_old_stage_a_text(stage_a_env):
    data, _ = stage_a_data(stage_a_env)
    text = squash(data["analysis"])
    for stale in ("n=40", "official swebench harness", "swe-bench lite"):
        assert stale not in text
    assert "ANALYSIS_PLAN_A" in dir(prereg)
    assert data["analysis"] == prereg.ANALYSIS_PLAN_A


# ---- K100 script and Makefile ------------------------------------------------------------


def make_n(args: list[str]):
    return subprocess.run(
        ["make", "-n", "prereg", *args], cwd=ROOT, capture_output=True, text=True, check=False
    )


def test_k100_make_prereg_stage_a_should_pass_the_rules_path():
    r = make_n(["NAME=c1-a", "STAGE=a", "RULES=c1"])
    assert r.returncode == 0, r.stderr
    line = next(
        ln for ln in r.stdout.splitlines() if "05_preregister.sh" in ln and "test -n" not in ln
    )
    assert "--stage a" in line
    assert "--rules prereg/c1/preregistration.toml" in line
    assert " c1-a" in line


def test_k100_make_prereg_stage_a_without_rules_should_stop_before_the_script():
    r = make_n(["NAME=c1-a", "STAGE=a"])
    assert r.returncode != 0
    assert "RULES" in r.stderr + r.stdout
    assert "05_preregister.sh" not in "\n".join(
        ln for ln in r.stdout.splitlines() if "test -n" not in ln
    )


def test_k100_make_prereg_stage_cb_should_not_need_rules():
    r = make_n(["NAME=x", "STAGE=cb"])
    assert r.returncode == 0, r.stderr
    assert "--rules" not in r.stdout


def stage_a_rig(tmp_path, *, preplace=True, extra_env=None, drop_swe=False):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    env = {
        **prereg_env(rig, tmp_path),
        "FAKE_UV_STUB_MODULES": "hcbench.rebench,hcbench.prereg",
        **(extra_env or {}),
    }
    if drop_swe:
        keep = [
            ln
            for ln in (rig / "config" / "bench.env.example").read_text().splitlines()
            if not ln.startswith("HC_SWE_")
        ]
        envf = rig / "config" / "no-swe.env"
        envf.write_text("\n".join(keep) + "\n")
        env["HC_ENV_FILE"] = str(envf)
        env.pop("HC_SWE_DATASET_REVISION", None)
    rules = rig / "prereg" / "c1" / "preregistration.toml"
    rules.parent.mkdir(parents=True)
    rules.write_text('stage = "cb"\n')
    d = rig / "prereg" / "t"
    if preplace:  # what the stubbed rebench and prereg would have written
        make_sel_dir(d, STAGE_IDS)
    return rig, bindir, log, env, d


def argv_text(c):
    return " ".join(c)


def find(log, *needles, without=()):
    """Index of the first uv call holding every needle and none of `without`."""
    for k, c in enumerate(calls(log)):
        if c[0] == "uv" and all(n in c for n in needles) and not any(w in c for w in without):
            return k
    return None


def pull_index(log):
    for k, c in enumerate(calls(log)):
        if c[:2] == ["docker", "pull"]:
            return k
    return None


A_ARGS = ["t", "--stage", "a", "--rules", "prereg/c1/preregistration.toml"]


def test_k100_stage_a_script_should_run_select_pull_check_then_prereg_in_order(tmp_path):
    rig, bindir, log, env, _ = stage_a_rig(tmp_path)
    r = run_script(rig, "05_preregister.sh", A_ARGS, bindir, env)
    assert r.returncode == 0, r.stderr
    i_sel = find(log, "hcbench.rebench", "select")
    i_chk = find(log, "hcbench.rebench", "check-images")
    i_pre = find(log, "hcbench.prereg", without=("--preflight",))  # K117: not the preflight
    i_pull = pull_index(log)
    assert None not in (i_sel, i_chk, i_pre, i_pull)
    assert i_sel < i_pull < i_chk < i_pre
    inspects = [k for k, c in enumerate(calls(log)) if c[:3] == ["docker", "image", "inspect"]]
    assert inspects and max(inspects) < i_chk  # digests are pinned before the check
    pulled = {c[-1] for c in docker_calls(log) if c[:1] == ["pull"]}
    assert pulled == {image_of(i) for i in STAGE_IDS}


def test_k100_stage_a_script_should_hand_the_selection_and_rules_to_prereg(tmp_path):
    rig, bindir, log, env, d = stage_a_rig(tmp_path)
    run_script(rig, "05_preregister.sh", A_ARGS, bindir, env)
    c = calls(log)[find(log, "hcbench.prereg", without=("--preflight",))]
    assert c[c.index("--selection") + 1] == str(d / "selection.json")
    assert c[c.index("--rules") + 1] == "prereg/c1/preregistration.toml"
    assert c[c.index("--stage") + 1] == "a"
    sel = calls(log)[find(log, "hcbench.rebench", "select")]
    assert sel[sel.index("--out") + 1] == str(d)
    chk = calls(log)[find(log, "hcbench.rebench", "check-images")]
    assert chk[chk.index("--dir") + 1] == str(d)


def test_k100_stage_a_script_should_pass_the_suite_a_rules_to_select(tmp_path):
    rig, bindir, log, env, _ = stage_a_rig(tmp_path)
    vals = load_env_file(rig / "config" / "bench.env.example", base={"HOME": "/h"})
    run_script(rig, "05_preregister.sh", A_ARGS, bindir, env)
    c = calls(log)[find(log, "hcbench.rebench", "select")]
    for flag, key in (
        ("--dataset", "HC_A_DATASET"),
        ("--min-split", "HC_A_MIN_SPLIT"),
        ("--min-created", "HC_A_MIN_CREATED"),
        ("--n", "HC_A_N"),
        ("--seed", "HC_SELECTION_SEED"),
        ("--grader", "HC_A_GRADER"),
    ):
        assert c[c.index(flag) + 1] == vals[key], flag
    assert "--revision" in c


def test_k100_stage_a_script_should_not_call_select_tasks_or_read_the_swe_keys(tmp_path):
    rig, bindir, log, env, _ = stage_a_rig(tmp_path, drop_swe=True)
    r = run_script(rig, "05_preregister.sh", A_ARGS, bindir, env)
    assert r.returncode == 0, r.stderr
    assert not any("hcbench.select_tasks" in argv_text(c) for c in calls(log))
    assert find(log, "hcbench.rebench", "select") is not None


def test_k100_stage_a_without_rules_should_stop_before_any_file_or_call(tmp_path):
    rig, bindir, log, env, d = stage_a_rig(tmp_path, preplace=False)
    r = run_script(rig, "05_preregister.sh", ["t", "--stage", "a"], bindir, env)
    assert r.returncode != 0
    assert "rules" in (r.stdout + r.stderr).lower()
    assert not d.exists()
    assert calls(log) == []


def test_k100_skip_pull_without_draft_should_be_refused_before_any_work(tmp_path):
    rig, bindir, log, env, d = stage_a_rig(
        tmp_path, preplace=False, extra_env={"HC_SKIP_PULL": "1"}
    )
    r = run_script(rig, "05_preregister.sh", A_ARGS, bindir, env)
    assert r.returncode != 0
    assert "HC_SKIP_PULL" in r.stdout + r.stderr
    assert not d.exists()
    assert calls(log) == []


def test_k100_skip_pull_with_draft_should_skip_only_the_pulls(tmp_path):
    rig, bindir, log, env, _ = stage_a_rig(tmp_path, extra_env={"HC_SKIP_PULL": "1"})
    r = run_script(rig, "05_preregister.sh", [*A_ARGS, "--draft"], bindir, env)
    assert r.returncode == 0, r.stderr
    assert pull_index(log) is None
    assert find(log, "hcbench.rebench", "select") is not None
    c = calls(log)[find(log, "hcbench.prereg", without=("--preflight",))]
    assert "--draft" in c


def test_k100_stage_a_script_should_still_write_a_deviations_file(tmp_path):
    rig, bindir, _, env, d = stage_a_rig(tmp_path)
    run_script(rig, "05_preregister.sh", A_ARGS, bindir, env)
    assert (d / "DEVIATIONS.md").read_text().startswith("# Deviations")


def test_k100_skip_pull_with_stage_cb_should_not_be_affected(tmp_path):
    rig, bindir, _, env, _ = stage_a_rig(tmp_path, preplace=False, extra_env={"HC_SKIP_PULL": "1"})
    r = run_script(rig, "05_preregister.sh", ["t", "--stage", "cb", "--draft"], bindir, env)
    assert r.returncode == 0, r.stderr
