"""Spec section 11.13, K70-K77 (review of round 9, 2026-11-20).

K72: hcbench.bundle.pi_manifest_fields(prefix) returns pi_package, pi_installed_version and
pi_lock_sha256 (name and version read from node_modules/<PI_PACKAGE>/package.json under the
prefix). K73: hcbench.prereg.check_pi_bundle(env, manifest_path), run for non-draft
pre-registrations only (a draft skips it, unlike check_hermes_bundle). K74/K76: the CLI
(hcbench.bundle.main). K70/K71/K75/K76: an offline run of scripts/01_build_bundle.sh
--skip-hone --skip-hermes with a fake docker (writes a fake Pi package and lock into the mounted
prefix), a fake curl (Node tarball + SHASUMS256.txt) and a fake Node binary that reports its
version. Doc checks are substring checks.

Assumptions: check-node treats a missing `engines` key or a missing `engines.node` as "no
minimum" (passes); a null/list/string `engines` and a non-string `engines.node` (number, list,
object) give the "by hand" message. The shared-image comment in bench.env.example contains "Pi"
and "shared".
"""

import hashlib
import json
import os
import re
import stat
import subprocess
import sys

import pytest
from fakebin import ROOT, calls, docker_calls, make_fake_bin, make_rig, run_script
from test_campaign1_prereg import pin, run_cb

from hcbench import prereg

PKG = "@earendil-works/pi-coding-agent"
ENV_EXAMPLE = ROOT / "config" / "bench.env.example"
GOOD_LOCK = "ab" * 32


def bundle():
    from hcbench import bundle as b

    return b


def comment_above(key):
    lines = ENV_EXAMPLE.read_text().splitlines()
    i = next(i for i, ln in enumerate(lines) if ln.startswith(f"{key}="))
    j = i
    while j > 0 and lines[j - 1].startswith("#"):
        j -= 1
    return "\n".join(lines[j:i])


def install_fake_pi(prefix, version="1.5.0", name=PKG, lock=b"lock\n"):
    pkg = prefix / "node_modules" / name
    pkg.mkdir(parents=True)
    (pkg / "package.json").write_text(json.dumps({"name": name, "version": version}))
    if lock is not None:
        (prefix / "package-lock.json").write_bytes(lock)
    return pkg


# ---- K72 pi_manifest_fields reads the installed package -----------------------------------


def test_k72_fields_read_name_and_version_from_the_installed_package_json(tmp_path):
    install_fake_pi(tmp_path, version="1.5.0")
    f = bundle().pi_manifest_fields(tmp_path)
    assert set(f) == {"pi_package", "pi_installed_version", "pi_lock_sha256"}
    assert f["pi_package"] == PKG
    assert f["pi_installed_version"] == "1.5.0"


def test_k72_the_version_is_the_one_on_disk_not_a_constant(tmp_path):
    install_fake_pi(tmp_path, version="1.4.2")
    assert bundle().pi_manifest_fields(tmp_path)["pi_installed_version"] == "1.4.2"


def test_k72_the_package_name_is_read_from_package_json(tmp_path):
    # The directory is node_modules/<PI_PACKAGE>; the name inside the file is what is reported.
    pkg = install_fake_pi(tmp_path)
    (pkg / "package.json").write_text(json.dumps({"name": "renamed", "version": "1.5.0"}))
    assert bundle().pi_manifest_fields(tmp_path)["pi_package"] == "renamed"


def test_k72_version_is_none_when_the_package_is_not_installed(tmp_path):
    (tmp_path / "package-lock.json").write_text("x")
    f = bundle().pi_manifest_fields(tmp_path)
    assert f["pi_installed_version"] is None
    assert f["pi_lock_sha256"] == hashlib.sha256(b"x").hexdigest()


def test_k72_an_empty_prefix_gives_no_version_and_no_hash(tmp_path):
    f = bundle().pi_manifest_fields(tmp_path)
    assert f["pi_installed_version"] is None and f["pi_lock_sha256"] is None


# ---- K74 / K76 the CLI --------------------------------------------------------------------


def run_main(*argv):
    return bundle().main(list(argv))


def write_pkg_json(tmp_path, data):
    p = tmp_path / "package.json"
    p.write_text(json.dumps(data))
    return str(p)


def test_k74_manifest_fields_exits_with_an_error_when_the_lock_is_missing(tmp_path):
    install_fake_pi(tmp_path, lock=None)
    with pytest.raises(SystemExit) as e:
        run_main("manifest-fields", str(tmp_path))
    assert e.value.code not in (0, None)


def test_k74_manifest_fields_missing_lock_is_an_error_not_a_traceback(tmp_path):
    install_fake_pi(tmp_path, lock=None)
    r = subprocess.run(
        [sys.executable, "-m", "hcbench.bundle", "manifest-fields", str(tmp_path)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        env={**os.environ, "PYTHONPATH": str(ROOT)},
    )
    assert r.returncode != 0
    assert "Traceback" not in r.stderr
    assert "lock" in r.stderr.lower()


def test_k76_manifest_fields_prints_the_three_fields_as_json(tmp_path, capsys):
    install_fake_pi(tmp_path, lock=b"L")
    run_main("manifest-fields", str(tmp_path))
    assert json.loads(capsys.readouterr().out) == {
        "pi_package": PKG,
        "pi_installed_version": "1.5.0",
        "pi_lock_sha256": hashlib.sha256(b"L").hexdigest(),
    }


BY_HAND = [
    {"engines": None},
    {"engines": ["node"]},
    {"engines": ">=22.19.0"},
    {"engines": {"node": 22}},
    {"engines": {"node": [">=22.19.0"]}},
    {"engines": {"node": {"min": "22"}}},
]


@pytest.mark.parametrize("data", BY_HAND, ids=lambda d: json.dumps(d))
def test_k74_check_node_asks_for_a_manual_check_on_an_odd_engines_field(tmp_path, data):
    path = write_pkg_json(tmp_path, data)
    with pytest.raises(SystemExit) as e:
        run_main("check-node", "22.23.4", path)
    assert "by hand" in str(e.value.code)


@pytest.mark.parametrize("data", [{}, {"name": "x"}, {"engines": {}}, {"engines": {"npm": "1"}}])
def test_k74_check_node_passes_when_there_is_no_node_minimum(tmp_path, data):
    assert run_main("check-node", "22.11.0", write_pkg_json(tmp_path, data)) is None


def test_k76_check_node_passes_and_fails_through_the_cli(tmp_path):
    path = write_pkg_json(tmp_path, {"engines": {"node": ">=22.19.0"}})
    assert run_main("check-node", "22.23.4", path) is None
    with pytest.raises(SystemExit) as e:
        run_main("check-node", "22.9.0", path)
    assert "22.9.0" in str(e.value.code) and "22.19.0" in str(e.value.code)


def test_k76_copy_ext_copies_through_the_cli(tmp_path):
    ext = tmp_path / "pkg" / "examples" / "extensions"
    ext.mkdir(parents=True)
    for n in ("permission-gate", "protected-paths", "confirm-destructive"):
        (ext / f"{n}.ts").write_text(n)
    dest = tmp_path / "dest"
    dest.mkdir()
    run_main("copy-ext", str(tmp_path / "pkg"), str(dest))
    assert sorted(p.name for p in dest.iterdir()) == [
        "confirm-destructive.ts",
        "permission-gate.ts",
        "protected-paths.ts",
    ]


@pytest.mark.parametrize(
    "argv",
    [
        [],
        ["nope"],
        ["check-node"],
        ["check-node", "22.1.0"],
        ["copy-ext", "a"],
        ["manifest-fields"],
    ],
)
def test_k76_bad_usage_exits_with_an_error(argv):
    with pytest.raises(SystemExit) as e:
        run_main(*argv)
    assert e.value.code not in (0, None)


# ---- K73 the pre-registration refuses a Pi bundle it cannot vouch for ---------------------


def pi_env(**arms):
    return {
        "HC_ARMS_MAIN": "hone-inject",
        "HC_ARMS_REDTEAM": "hone-inject",
        "HC_ARMS_B": "hone-inject",
        "HC_PI_VERSION": "1.5.0",
        "HC_NODE_VERSION": "22.23.4",
        **arms,
    }


def good_manifest(**over):
    return {
        "pi_package": PKG,
        "pi_installed_version": "1.5.0",
        "pi_lock_sha256": GOOD_LOCK,
        "node_installed_version": "v22.23.4",
        **over,
    }


def write_manifest(tmp_path, manifest):
    p = tmp_path / "manifest.json"
    if manifest is not None:
        p.write_text(json.dumps(manifest))
    return p


@pytest.mark.parametrize("key", ["HC_ARMS_MAIN", "HC_ARMS_REDTEAM", "HC_ARMS_B"])
@pytest.mark.parametrize("arm", ["pi", "pi-guarded"])
def test_k73_a_good_manifest_is_accepted_for_every_pi_arm_list(tmp_path, key, arm):
    env = pi_env(**{key: f"hone-inject,{arm}"})
    prereg.check_pi_bundle(env, write_manifest(tmp_path, good_manifest()))


@pytest.mark.parametrize("key", ["HC_ARMS_MAIN", "HC_ARMS_REDTEAM", "HC_ARMS_B"])
@pytest.mark.parametrize("arm", ["pi", "pi-guarded"])
def test_k73_a_missing_manifest_is_refused_for_every_pi_arm_list(tmp_path, key, arm):
    env = pi_env(**{key: f"hone-inject,{arm}"})
    with pytest.raises(SystemExit):
        prereg.check_pi_bundle(env, write_manifest(tmp_path, None))


def test_k73_no_pi_arm_means_no_check(tmp_path):
    prereg.check_pi_bundle(
        pi_env(HC_ARMS_MAIN="hone-inject,hermes"), write_manifest(tmp_path, None)
    )


BAD = [
    ("pi_package", {"pi_package": "@mariozechner/pi-coding-agent"}),
    ("pi_package", {"pi_package": None}),
    ("pi_installed_version", {"pi_installed_version": "0.73.1"}),
    ("pi_installed_version", {"pi_installed_version": None}),
    ("pi_lock_sha256", {"pi_lock_sha256": None}),
    ("pi_lock_sha256", {"pi_lock_sha256": ""}),
    ("pi_lock_sha256", {"pi_lock_sha256": "ab" * 31}),
    ("pi_lock_sha256", {"pi_lock_sha256": "ab" * 33}),
    ("pi_lock_sha256", {"pi_lock_sha256": "AB" * 32}),
    ("pi_lock_sha256", {"pi_lock_sha256": "zz" * 32}),
]


@pytest.mark.parametrize(
    "field,over", BAD, ids=lambda v: json.dumps(v) if isinstance(v, dict) else v
)
def test_k73_a_bad_field_is_refused_and_named(tmp_path, field, over):
    env = pi_env(HC_ARMS_MAIN="pi")
    with pytest.raises(SystemExit) as e:
        prereg.check_pi_bundle(env, write_manifest(tmp_path, good_manifest(**over)))
    assert field in str(e.value.code)


@pytest.mark.parametrize("field", ["pi_package", "pi_installed_version", "pi_lock_sha256"])
def test_k73_a_missing_field_is_refused_and_named(tmp_path, field):
    m = good_manifest()
    del m[field]
    with pytest.raises(SystemExit) as e:
        prereg.check_pi_bundle(pi_env(HC_ARMS_B="pi-guarded"), write_manifest(tmp_path, m))
    assert field in str(e.value.code)


def test_k73_the_version_is_compared_with_hc_pi_version(tmp_path):
    env = pi_env(HC_ARMS_MAIN="pi", HC_PI_VERSION="1.6.0")
    with pytest.raises(SystemExit) as e:
        prereg.check_pi_bundle(env, write_manifest(tmp_path, good_manifest()))
    assert "pi_installed_version" in str(e.value.code)


def set_manifest(env, manifest):
    (env["tmp"] / "bundle" / "manifest.json").write_text(json.dumps(manifest))


def test_k73_a_non_draft_prereg_with_a_good_pi_manifest_is_written(env):
    pin(env)
    rc, out = run_cb(env)
    assert rc == 0 and out.exists()


@pytest.mark.parametrize("field", ["pi_package", "pi_installed_version", "pi_lock_sha256"])
def test_k73_a_non_draft_prereg_refuses_a_manifest_without_the_pi_field(env, field):
    pin(env)
    m = {k: v for k, v in env["manifest"].items() if k != field}
    set_manifest(env, m)
    with pytest.raises(SystemExit) as e:
        run_cb(env)
    assert field in str(e.value.code)


def test_k73_a_non_draft_prereg_refuses_the_old_pi_manifest_of_round_9(env):
    pin(env)
    old = {k: env["manifest"][k] for k in ("hermes_ref", "hermes_sha")}
    set_manifest(env, {**old, "pi_version": "0.73.1"})
    with pytest.raises(SystemExit):
        run_cb(env)


def test_k73_a_draft_prereg_skips_the_pi_check(env):
    set_manifest(env, {k: v for k, v in env["manifest"].items() if not k.startswith("pi_")})
    rc, out = run_cb(env, "--draft")
    assert rc == 0 and out.exists()


# ---- offline run of scripts/01_build_bundle.sh ---------------------------------------------

FAKE_LOCK = "fake lock for the pi prefix\n"

_DOCKER = r"""#!{python}
import json, os, subprocess, sys
from pathlib import Path
args = sys.argv[1:]
with open({log!r}, "a") as fh:
    fh.write(json.dumps(["docker"] + args) + "\n")
if args[:1] == ["run"] and "install" in args:
    mount = next(a for a in args if a.endswith(":/opt/harness"))
    bundle = Path(mount.split(":")[0])
    prefix = bundle / "pi"
    # K80: real npm merges into an existing prefix, so the build must have removed it
    if prefix.exists():
        sys.stderr.write("fake npm: the Pi prefix still exists: " + str(prefix) + "\n")
        sys.exit(3)
    # K80: log the version of the bundle Node that npm would run under
    probe = subprocess.run([str(bundle / "node" / "bin" / "node"), "--version"],
                           capture_output=True, text=True)
    with open({log!r}, "a") as fh:
        fh.write(json.dumps(["npm-node", probe.stdout.strip()]) + "\n")
    if os.environ.get("FAKE_NPM_FAIL"):
        sys.exit(1)
    ver = args[-1].rsplit("@", 1)[1]
    pkg = prefix / "node_modules" / "@earendil-works" / "pi-coding-agent"
    (pkg / "examples" / "extensions").mkdir(parents=True, exist_ok=True)
    for n in ("permission-gate", "protected-paths", "confirm-destructive"):
        (pkg / "examples" / "extensions" / (n + ".ts")).write_text("// " + n + "\n")
    engines = os.environ.get("FAKE_PI_ENGINES", ">=22.19.0")
    (pkg / "package.json").write_text(json.dumps({{
        "name": "@earendil-works/pi-coding-agent", "version": ver,
        "bin": {{"pi": "dist/cli.js"}}, "engines": {{"node": engines}}}}))
    (prefix / "package.json").write_text("{{}}")
    (prefix / "package-lock.json").write_text({lock!r})
"""

_CURL = r"""#!{python}
import hashlib, io, json, re, sys, tarfile
from pathlib import Path
args = sys.argv[1:]
with open({log!r}, "a") as fh:
    fh.write(json.dumps(["curl"] + args) + "\n")
out, url = Path(args[-2]), args[-1]
m = re.search(r"/dist/v([0-9.]+)/", url)
ver = m.group(1)
name = "node-v" + ver + "-linux-x64.tar.xz"
buf = io.BytesIO()
with tarfile.open(fileobj=buf, mode="w:xz") as t:
    data = ("#!/bin/sh\necho v" + ver + "\n").encode()
    ti = tarfile.TarInfo("node-v" + ver + "-linux-x64/bin/node")
    ti.size, ti.mode = len(data), 0o755
    t.addfile(ti, io.BytesIO(data))
tarball = buf.getvalue()
if url.endswith("SHASUMS256.txt"):
    out.write_text(hashlib.sha256(tarball).hexdigest() + "  " + name + "\n")
else:
    out.write_bytes(tarball)
"""


_SHA256SUM = r"""#!{python}
import hashlib, sys
# only `sha256sum -c [-]`: check "<hex>  <file>" lines from stdin or a file, in the cwd
args = [a for a in sys.argv[1:] if a != "-c"]
lines = (open(args[0]) if args and args[0] != "-" else sys.stdin).read().splitlines()
bad = 0
for ln in lines:
    want, name = ln.split(None, 1)
    ok = hashlib.sha256(open(name.strip(), "rb").read()).hexdigest() == want
    print(name.strip() + (": OK" if ok else ": FAILED"))
    bad += not ok
sys.exit(1 if bad else 0)
"""


def write_exec(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    path.chmod(path.stat().st_mode | stat.S_IEXEC)


class Build:
    def __init__(self, tmp_path):
        self.rig = make_rig(tmp_path)
        self.bindir, self.log = make_fake_bin(tmp_path)
        write_exec(
            self.bindir / "docker",
            _DOCKER.format(python=sys.executable, log=str(self.log), lock=FAKE_LOCK),
        )
        write_exec(self.bindir / "curl", _CURL.format(python=sys.executable, log=str(self.log)))
        write_exec(self.bindir / "sha256sum", _SHA256SUM.format(python=sys.executable))
        self.B = tmp_path / "bundle"
        self.B.mkdir()

    def node(self, version):
        write_exec(self.B / "node" / "bin" / "node", f"#!/bin/sh\necho v{version}\n")

    def run(self, *flags, **env):
        full = {"HC_BUNDLE_DIR": str(self.B), **env}
        return run_script(
            self.rig,
            "01_build_bundle.sh",
            ["--skip-hone", "--skip-hermes", *flags],
            self.bindir,
            full,
        )

    def manifest(self):
        return json.loads((self.B / "manifest.json").read_text())

    def calls(self, name):
        return [c for c in calls(self.log) if c[0] == name]


@pytest.fixture
def build(tmp_path):
    return Build(tmp_path)


def test_k76_the_offline_build_succeeds_and_installs_pi_and_the_extensions(build):
    r = build.run()
    assert r.returncode == 0, r.stderr
    assert (build.B / "bin" / "pi").is_file()
    assert sorted(p.name for p in (build.B / "pi-ext").iterdir()) == [
        "confirm-destructive.ts",
        "permission-gate.ts",
        "protected-paths.ts",
    ]
    assert PKG in (build.B / "bin" / "pi").read_text()


def test_k76_1_a_leftover_old_prefix_and_a_stray_extension_do_not_survive(build):
    old = build.B / "pi" / "node_modules" / "@mariozechner" / "pi-coding-agent"
    old.mkdir(parents=True)
    (old / "package.json").write_text("{}")
    (build.B / "pi" / "package-lock.json").write_text("old lock")
    (build.B / "pi-ext").mkdir(exist_ok=True)
    (build.B / "pi-ext" / "stray.ts").write_text("// stray")
    r = build.run()
    assert r.returncode == 0, r.stderr
    assert not (build.B / "pi" / "node_modules" / "@mariozechner").exists()
    assert not (build.B / "pi-ext" / "stray.ts").exists()
    assert "stray.ts" not in build.manifest()["pi_ext_sha256"]
    assert build.manifest()["pi_lock_sha256"] == hashlib.sha256(FAKE_LOCK.encode()).hexdigest()


def test_k71_the_manifest_lists_exactly_the_three_extensions(build):
    (build.B / "pi-ext").mkdir()
    (build.B / "pi-ext" / "stray.ts").write_text("// stray")
    assert build.run().returncode == 0
    assert sorted(build.manifest()["pi_ext_sha256"]) == [
        "confirm-destructive.ts",
        "permission-gate.ts",
        "protected-paths.ts",
    ]


def test_k76_2_a_bundle_node_of_another_version_is_downloaded_again(build):
    build.node("22.11.0")
    r = build.run()
    assert r.returncode == 0, r.stderr
    assert any("node-v22.23.4-linux-x64.tar.xz" in c[-1] for c in build.calls("curl"))
    out = subprocess.run(
        [str(build.B / "node" / "bin" / "node"), "--version"], capture_output=True, text=True
    )
    assert out.stdout.strip() == "v22.23.4"


def test_k70_a_broken_bundle_node_is_downloaded_again(build):
    write_exec(build.B / "node" / "bin" / "node", "#!/bin/sh\nexit 1\n")
    assert build.run().returncode == 0
    assert build.calls("curl")


def test_k70_a_bundle_node_of_the_pinned_version_is_kept(build):
    build.node("22.23.4")
    assert build.run().returncode == 0
    assert build.calls("curl") == []


def test_k70_a_missing_bundle_node_is_downloaded(build):
    assert build.run().returncode == 0
    assert any("nodejs.org/dist/v22.23.4/" in c[-1] for c in build.calls("curl"))


def test_k76_3_the_manifest_has_the_pi_package_version_and_lock_hash(build):
    assert build.run().returncode == 0
    m = build.manifest()
    assert m["pi_package"] == PKG
    assert m["pi_installed_version"] == "1.5.0"
    assert m["pi_lock_sha256"] == hashlib.sha256(FAKE_LOCK.encode()).hexdigest()
    assert m["node_version"] == "22.23.4"


def test_k76_4_skip_pi_on_a_built_bundle_keeps_the_pi_fields_read_from_disk(build):
    assert build.run().returncode == 0
    first = build.manifest()
    (build.B / "manifest.json").unlink()  # nothing may be merged from an earlier manifest
    assert build.run("--skip-pi").returncode == 0
    again = build.manifest()
    for k in ("pi_package", "pi_installed_version", "pi_lock_sha256"):
        assert again[k] == first[k] is not None


def test_k72_without_a_pi_prefix_the_three_fields_are_null_and_nothing_is_merged(build):
    (build.B / "manifest.json").write_text(
        json.dumps({"pi_package": "old", "pi_installed_version": "0.73.1", "pi_lock_sha256": "x"})
    )
    assert build.run("--skip-pi").returncode == 0
    m = build.manifest()
    assert (m["pi_package"], m["pi_installed_version"], m["pi_lock_sha256"]) == (None, None, None)


def test_k76_5_an_engines_minimum_above_the_node_stops_the_build_before_the_manifest(build):
    r = build.run(FAKE_PI_ENGINES=">=99.0.0")
    assert r.returncode != 0
    assert "99.0.0" in r.stderr and "22.23.4" in r.stderr
    assert not (build.B / "manifest.json").exists()


def test_k76_an_engines_form_that_is_not_a_minimum_stops_the_build(build):
    r = build.run(FAKE_PI_ENGINES="^22")
    assert r.returncode != 0
    assert "by hand" in r.stderr
    assert not (build.B / "manifest.json").exists()


# ---- K70 / K75 the npm command and the Pi container ---------------------------------------


def install_call(build):
    (c,) = [c for c in docker_calls(build.log) if c[:1] == ["run"] and "install" in c]
    return c


def test_k70_npm_runs_with_engine_strict_and_ignore_scripts(build):
    assert build.run().returncode == 0
    args = install_call(build)
    assert "--engine-strict" in args and "--ignore-scripts" in args
    assert args[-1] == f"{PKG}@1.5.0"
    assert args.index("--engine-strict") > args.index("install")


def test_k75_the_pi_container_is_the_pinned_shared_image_for_linux_amd64(build):
    assert build.run().returncode == 0
    args = install_call(build)
    image = ENV_EXAMPLE.read_text()
    image = re.search(r"^HC_HERMES_BUILD_IMAGE=(\S+)", image, re.M).group(1)
    assert image in args
    assert "ubuntu:22.04" not in args
    i = args.index("--platform")
    assert args[i + 1] == "linux/amd64"
    assert i < args.index(image)  # an option of docker run, not of the command in the image


@pytest.mark.parametrize("value", ["", None])
def test_k75_the_build_fails_without_the_hermes_build_image(build, value):
    env = {"HC_HERMES_BUILD_IMAGE": value} if value is not None else {}
    if value is None:  # unset: run_script cannot unset, so blank it through an env file
        envf = build.rig / "config" / "no-image.env"
        envf.write_text(
            "".join(
                ln + "\n"
                for ln in ENV_EXAMPLE.read_text().splitlines()
                if not ln.startswith("HC_HERMES_BUILD_IMAGE=")
            )
        )
        env = {"HC_ENV_FILE": str(envf)}
    r = build.run(**env)
    assert r.returncode != 0
    assert "HC_HERMES_BUILD_IMAGE" in r.stderr
    assert [c for c in docker_calls(build.log) if c[:1] == ["run"]] == []


# ---- K75 / K77 docs -----------------------------------------------------------------------


def test_k75_the_example_says_the_build_image_is_shared_with_pi():
    c = comment_above("HC_HERMES_BUILD_IMAGE")
    assert "Pi" in c and "shared" in c.lower()


def test_k77_the_max_model_len_comment_says_the_ratio_is_hones_for_a_qwen_name():
    c = comment_above("HC_MAX_MODEL_LEN")
    assert "0.45" in c and "qwen" in c.lower() and "served name" in c


def test_k77_the_readme_notes_what_skip_pi_keeps():
    text = (ROOT / "README.md").read_text()
    i = text.find("--skip-pi")
    assert i >= 0, "README does not mention --skip-pi"
    window = text[max(i - 300, 0) : i + 600].lower()
    assert "pi_package" in window or "manifest" in window
    assert "disk" in window or "keeps" in window or "kept" in window
