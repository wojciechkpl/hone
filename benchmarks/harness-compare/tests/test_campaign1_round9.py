"""Spec section 11.12, K64-K67 and K69 (pre-tag pins, 2026-11-20).

K64: every Hone arm sets HONE_CONTEXT_WINDOW = "{{context_window}}". K65-K67: Pi comes from
@earendil-works/pi-coding-agent; the build logic lives in hcbench.bundle (PI_PACKAGE,
PI_EXTENSIONS, check_node_engine, copy_pi_extensions, pi_manifest_fields), called from
scripts/01_build_bundle.sh. K69: docs. Doc checks are substring checks.
"""

import hashlib
import json
import re
import subprocess

import pytest
from fakebin import ROOT

from hcbench import arms as arms_mod
from hcbench.config import load_env_file

ENV_EXAMPLE = ROOT / "config" / "bench.env.example"
SCRIPT = ROOT / "scripts" / "01_build_bundle.sh"
PLAN = ROOT.parent.parent / "docs" / "plans" / "harness-compare-v2-2026-11.md"
NEW_PKG = "@earendil-works/pi-coding-agent"
OLD_PKG = "@mariozechner/pi-coding-agent"


def bundle():
    from hcbench import bundle as b  # missing module is the RED reason

    return b


def load_arms():
    return arms_mod.load_arms(ROOT / "config" / "arms.toml")


# ---- K64 Hone is told the served window ---------------------------------------------------


def hone_arms():
    return {n: a for n, a in load_arms().items() if a.kind == "hone"}


def test_k64_there_are_four_hone_arms():
    assert sorted(hone_arms()) == [
        "hone-callgraph",
        "hone-guarded",
        "hone-inject",
        "hone-noinject",
    ]


@pytest.mark.parametrize("name", ["hone-noinject", "hone-inject", "hone-guarded", "hone-callgraph"])
def test_k64_every_hone_arm_sets_the_context_window_placeholder(name):
    assert hone_arms()[name].env.get("HONE_CONTEXT_WINDOW") == "{{context_window}}"


def test_k64_a_rendered_hone_arm_carries_the_configured_window():
    ctx = {
        "prompt": "p",
        "model": "m",
        "base_url": "http://x/run/k",
        "run_id": "r",
        "workdir": "/w",
        "context_window": "65536",
        "max_tokens": "4096",
    }
    for arm in hone_arms().values():
        _argv, env, _files = arms_mod.render(arm, ctx)
        assert env["HONE_CONTEXT_WINDOW"] == "65536"


def max_model_len_comment():
    lines = ENV_EXAMPLE.read_text().splitlines()
    i = next(i for i, ln in enumerate(lines) if ln.startswith("HC_MAX_MODEL_LEN="))
    j = i
    while j > 0 and lines[j - 1].startswith("#"):
        j -= 1
    return "\n".join(lines[j:i])


def test_k64_the_max_model_len_comment_names_hone_context_window():
    assert "HONE_CONTEXT_WINDOW" in max_model_len_comment()


def test_k64_the_max_model_len_comment_no_longer_says_no_override():
    assert "no override" not in max_model_len_comment().lower()


# ---- K65 constants, manifest, script text -------------------------------------------------


def test_k65_pi_package_and_extensions_constants():
    b = bundle()
    assert b.PI_PACKAGE == NEW_PKG
    assert tuple(b.PI_EXTENSIONS) == ("permission-gate", "protected-paths", "confirm-destructive")


def install_fake_pi(prefix, version="1.5.0", name=NEW_PKG, lock=b'{"lockfileVersion": 3}\n'):
    pkg = prefix / "node_modules" / name
    pkg.mkdir(parents=True)
    (pkg / "package.json").write_text(json.dumps({"name": name, "version": version}))
    if lock is not None:
        (prefix / "package-lock.json").write_bytes(lock)


def test_k65_manifest_fields_hash_the_package_lock(tmp_path):
    lock = b'{"lockfileVersion": 3}\n'
    install_fake_pi(tmp_path, lock=lock)
    f = bundle().pi_manifest_fields(tmp_path)
    assert f == {
        "pi_package": NEW_PKG,
        "pi_installed_version": "1.5.0",
        "pi_lock_sha256": hashlib.sha256(lock).hexdigest(),
    }


def test_k65_manifest_lock_hash_is_none_when_the_lock_is_absent(tmp_path):
    install_fake_pi(tmp_path, lock=None)
    f = bundle().pi_manifest_fields(tmp_path)
    assert f == {"pi_package": NEW_PKG, "pi_installed_version": "1.5.0", "pi_lock_sha256": None}


def script_text():
    return SCRIPT.read_text()


def test_k65_script_installs_the_new_package_at_the_pinned_version():
    assert f"{NEW_PKG}@${{HC_PI_VERSION}}" in script_text()


def npm_command():
    """The text from npm-cli.js to the package argument: the npm command line."""
    text = script_text()
    start = text.index("npm-cli.js")
    return text[start : text.index(f"{NEW_PKG}@${{HC_PI_VERSION}}", start)]


@pytest.mark.parametrize("flag", ["--ignore-scripts", "--engine-strict"])
def test_k65_script_runs_npm_with_the_flag_inside_the_npm_command(flag):
    assert flag in npm_command().split()


def test_k65_script_wrapper_points_into_the_new_package():
    text = script_text()
    wrapper = text[text.index('cat > "${B}/bin/pi"') :]
    wrapper = wrapper[: wrapper.index("chmod")]
    assert f"node_modules/{NEW_PKG}/" in wrapper


def test_k65_script_has_no_reference_to_the_old_package():
    assert OLD_PKG not in script_text()


@pytest.mark.parametrize("sub", ["check-node", "copy-ext", "manifest-fields"])
def test_k65_script_calls_each_bundle_subcommand_on_a_non_comment_line(sub):
    lines = [ln for ln in script_text().splitlines() if not ln.lstrip().startswith("#")]
    assert any(re.search(rf"\bhc_bundle {sub}\b", ln) for ln in lines)


def rig_code_files():
    out = subprocess.run(
        ["git", "ls-files", "-co", "--exclude-standard", "."],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split("\n")
    skip = {"README.md", "SPEC.md"}
    return [
        p
        for p in out
        if p
        and p not in skip
        and p.split("/")[0] not in {"tests", "prereg"}
        and (ROOT / p).is_file()
    ]


@pytest.mark.parametrize("needle", [OLD_PKG, "HC_PI_EXT_REF"])
def test_k65_no_rig_code_file_names_the_old_package_or_the_ext_ref(needle):
    hits = []
    for p in rig_code_files():
        try:
            text = (ROOT / p).read_text()
        except UnicodeDecodeError:
            continue
        if needle in text:
            hits.append(p)
    assert hits == []


def test_k65_env_example_pins_pi_1_5_0():
    env = load_env_file(ENV_EXAMPLE)
    assert env["HC_PI_VERSION"] == "1.5.0"


def test_k65_env_example_comment_explains_the_pin():
    lines = ENV_EXAMPLE.read_text().splitlines()
    i = next(i for i, ln in enumerate(lines) if ln.startswith("HC_PI_VERSION="))
    j = i
    while j > 0 and lines[j - 1].startswith("#"):
        j -= 1
    comment = "\n".join(lines[j:i])
    assert "0.73.1" in comment
    assert "2026-11-02" in comment


# ---- K66 Node meets Pi's minimum ----------------------------------------------------------


def test_k66_env_example_pins_node_22_23_4():
    assert load_env_file(ENV_EXAMPLE)["HC_NODE_VERSION"] == "22.23.4"


@pytest.mark.parametrize(
    "node,engines",
    [
        ("22.23.4", ">=22.19.0"),
        ("22.19.0", ">=22.19.0"),
        ("22.11.0", None),
        ("22.11.0", ""),
        ("23.0.0", ">=22.19.0"),
        ("22.100.0", ">=22.19.0"),
    ],
)
def test_k66_node_at_or_above_the_minimum_passes(node, engines):
    assert bundle().check_node_engine(node, engines) is None


@pytest.mark.parametrize(
    "node,engines",
    [
        ("22.9.0", ">=22.19.0"),
        ("22.11.0", ">=22.19.0"),
        ("20.19.4", ">=22.19.0"),
        ("22.18.9", ">=22.19.0"),
    ],
)
def test_k66_node_below_the_minimum_fails_naming_both_versions(node, engines):
    with pytest.raises(SystemExit) as e:
        bundle().check_node_engine(node, engines)
    msg = str(e.value)
    assert node in msg and "22.19.0" in msg


@pytest.mark.parametrize("engines", ["^22", ">=22 <24", "22.x", ">=22.19", "~22.19.0"])
def test_k66_any_other_engines_form_fails_asking_for_a_manual_check(engines):
    with pytest.raises(SystemExit) as e:
        bundle().check_node_engine("22.23.4", engines)
    assert "by hand" in str(e.value)


# ---- K67 guard extensions come from the pinned package only -------------------------------


def make_pkg(tmp_path, names):
    pkg = tmp_path / "pkg"
    ext = pkg / "examples" / "extensions"
    ext.mkdir(parents=True)
    for n in names:
        (ext / f"{n}.ts").write_text(f"// {n}\n")
    return pkg


ALL_EXT = ("permission-gate", "protected-paths", "confirm-destructive")


def test_k67_copy_copies_every_extension(tmp_path):
    pkg = make_pkg(tmp_path, ALL_EXT)
    dest = tmp_path / "pi-ext"
    dest.mkdir()
    bundle().copy_pi_extensions(pkg, dest)
    for n in ALL_EXT:
        assert (dest / f"{n}.ts").read_text() == f"// {n}\n"


def test_k67_copy_fails_naming_every_missing_file(tmp_path):
    pkg = make_pkg(tmp_path, ["permission-gate"])
    dest = tmp_path / "pi-ext"
    dest.mkdir()
    with pytest.raises(SystemExit) as e:
        bundle().copy_pi_extensions(pkg, dest)
    msg = str(e.value)
    assert "protected-paths" in msg and "confirm-destructive" in msg


def test_k67_copy_never_touches_the_network(tmp_path, monkeypatch):
    import socket
    import urllib.request

    def boom(*a, **k):
        raise AssertionError("network access")

    monkeypatch.setattr(socket, "socket", boom)
    monkeypatch.setattr(socket, "create_connection", boom)
    monkeypatch.setattr(urllib.request, "urlopen", boom)
    pkg = make_pkg(tmp_path, ALL_EXT[:1])
    dest = tmp_path / "pi-ext"
    dest.mkdir()
    with pytest.raises(SystemExit):
        bundle().copy_pi_extensions(pkg, dest)


@pytest.mark.parametrize("needle", ["raw.githubusercontent.com", "HC_PI_EXT_REF", "pi-mono"])
def test_k67_script_downloads_nothing_from_a_branch(needle):
    assert needle not in script_text()


def test_k67_env_example_has_no_ext_ref_line():
    assert not re.search(r"^\s*#?\s*HC_PI_EXT_REF", ENV_EXAMPLE.read_text(), re.M)


# ---- K69 Docs -----------------------------------------------------------------------------


@pytest.mark.parametrize("doc", ["README.md", "SPEC.md"])
@pytest.mark.parametrize("needle", [NEW_PKG, "1.5.0", "22.23.4", "HONE_CONTEXT_WINDOW"])
def test_k69_rig_docs_name_the_pi_package_version_and_hone_window(doc, needle):
    assert needle in (ROOT / doc).read_text()


def test_k69_known_gaps_table_has_a_pi_dependency_tree_row():
    text = PLAN.read_text()
    start = text.index("### 11.9 Known gaps")
    end = text.index("\n### ", start + 1)
    rows = [ln for ln in text[start:end].splitlines() if ln.startswith("|")]
    assert any(
        "Pi" in r and ("dependency" in r or "dependencies" in r) and "lock" in r for r in rows
    )


# ---- K68 Hermes stays at v0.21.5 and the comment says why ----------------------------------


def hermes_ref_comment():
    lines = ENV_EXAMPLE.read_text().splitlines()
    i = next(i for i, ln in enumerate(lines) if ln.startswith("HC_HERMES_REF="))
    j = i
    while j > 0 and lines[j - 1].startswith("#"):
        j -= 1
    return "\n".join(lines[j:i])


def test_k68_hermes_pins_are_unchanged():
    env = load_env_file(ENV_EXAMPLE)
    assert env["HC_HERMES_REF"] == "v2026.9.24"
    assert env["HC_HERMES_SHA"] == "f97608f178d1ffeca59860195ab7da295f7c8e5f"


@pytest.mark.parametrize("needle", ["v0.21.5", "2026-09-24", "gateway"])
def test_k68_the_hermes_comment_explains_the_pin(needle):
    assert needle in hermes_ref_comment()


def test_k68_the_hermes_comment_no_longer_says_latest():
    assert "latest" not in hermes_ref_comment().lower()


@pytest.mark.parametrize("needle", ["v0.21.5", "gateway"])
def test_k68_the_readme_mentions_the_hermes_version_and_gateway(needle):
    assert needle in (ROOT / "README.md").read_text()
