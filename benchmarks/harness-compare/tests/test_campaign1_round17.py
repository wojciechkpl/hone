"""Round 17 (spec 11.18, K166-K170): the Hone part of scripts/01_build_bundle.sh.

The script runs offline against a fake `docker` that logs its argv, writes a fake `hone` into
the directory mounted at /out for the build call, and prints canned `readelf -d` text for the
call whose argv contains `readelf` (FAKE_READELF = good | bad | none | all_allowed;
fail = the call exits 1 with only stderr, fail_good = exits 1 after the good text).
Assertions are on the logged argv, never on the script's text.

Assumptions the implementer must honor:
* the build call is the `docker run` whose `bash -c` command contains `cargo build`; it mounts
  the bundle's bin/ at `<dir>:/out` (optionally `:rw`);
* the readelf call is a second `docker run` whose argv contains `readelf`, with the same image,
  after the build call, mounting the bundle's bin/ as `<B>/bin:<path>:ro`; its stdout is saved
  to a file under `.build/` and passed to `python -m hcbench.bundle check-needed <file>`;
* `check-needed` is tested only through the command line (exit code, stdout+stderr), no
  Python function name is assumed; its message names every disallowed library;
* when the check fails the script's output carries check-needed's message;
* the container's `bash -c` text is run locally (with /usr/lib/x86_64-linux-gnu and
  /usr/include rewritten to scratch dirs) to check the K166 library test.
"""

from __future__ import annotations

import json
import re
import stat
import subprocess
import sys
from pathlib import Path

import pytest
from fakebin import ROOT, calls, docker_calls, make_fake_bin, make_rig, run_script

PINNED = "rust:1-bullseye@sha256:" + "cd" * 32
EXAMPLE_IMAGE = (
    "rust:1-bullseye@sha256:4730e387a220a08a365c77da3096544dde214f9d796c16284d4be45438cad4a9"
)
ALLOWED = [
    "libc.so.6",
    "libm.so.6",
    "libdl.so.2",
    "libpthread.so.0",
    "librt.so.1",
    "libutil.so.1",
    "libresolv.so.2",
    "ld-linux-x86-64.so.2",
    "libgcc_s.so.1",
]


def needed(lib: str) -> str:
    return f" 0x0000000000000001 (NEEDED)             Shared library: [{lib}]"


HEADER = [
    "",
    "Dynamic section at offset 0x4a3f000 contains 31 entries:",
    "  Tag        Type                         Name/Value",
]
OTHER = [
    " 0x000000000000000c (INIT)               0x5a000",
    " 0x000000000000000d (FINI)               0x3f1c4c8",
    " 0x000000000000000e (SONAME)             Library soname: [libssl.so.1.1]",
    " 0x000000000000001d (RUNPATH)            Library runpath: [/opt/libcrypto.so.1.1]",
]
GOOD_LIBS = [
    "libgcc_s.so.1",
    "libpthread.so.0",
    "libm.so.6",
    "libdl.so.2",
    "libc.so.6",
    "ld-linux-x86-64.so.2",
]
BAD_EXTRA = ["libssl.so.1.1", "libcrypto.so.1.1", "libz.so.1"]


def readelf_text(libs: list[str]) -> str:
    return (
        "\n".join(HEADER + [needed(x) for x in libs[:2]] + OTHER + [needed(x) for x in libs[2:]])
        + "\n"
    )


GOOD = readelf_text(GOOD_LIBS)
BAD = readelf_text([*BAD_EXTRA, *GOOD_LIBS])
NONE = "\n".join(HEADER + OTHER) + "\n"
ALL_ALLOWED = readelf_text(ALLOWED)
TEXTS = {"good": GOOD, "bad": BAD, "none": NONE, "all_allowed": ALL_ALLOWED}

_DOCKER = r"""#!{python}
import json, os, re, sys
from pathlib import Path
args = sys.argv[1:]
with open({log!r}, "a") as fh:
    fh.write(json.dumps(["docker"] + args) + "\n")
cmd = ""
for i, a in enumerate(args[:-1]):
    if a == "-c":
        cmd = args[i + 1]
is_readelf = "readelf" in args or (
    "cargo build" not in cmd and re.search(r"(^|[\s;&|])readelf\s", cmd) is not None
)
if args[:1] == ["run"]:
    if is_readelf:
        mode = os.environ.get("FAKE_READELF", "good")
        with open({log!r}, "a") as fh:
            fh.write(json.dumps(["fake-readelf", mode]) + "\n")
        texts = json.loads({texts!r})
        if mode == "fail":
            sys.stderr.write("fake readelf: docker run failed\\n")
            sys.exit(1)
        sys.stdout.write(texts["good" if mode == "fail_good" else mode])
        sys.exit(1 if mode == "fail_good" else 0)
    elif "cargo build" in cmd:
        with open({log!r}, "a") as fh:
            fh.write(json.dumps(["fake-build"]) + "\n")
        for a in args:
            m = re.match(r"^(.+):/out(:rw)?$", a)
            if m:
                Path(m.group(1), "hone").write_text("#!/bin/sh\necho hone fake\n")
                Path(m.group(1), "hone").chmod(0o755)
"""


def git(repo: Path, *args: str, date: str | None = None, author_date: str | None = None) -> str:
    env = {"PATH": "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin", "HOME": str(repo.parent)}
    if date:
        env |= {"GIT_COMMITTER_DATE": date, "GIT_AUTHOR_DATE": author_date or date}
    cfg = ["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"]
    out = subprocess.run(
        ["git", *cfg, "-C", str(repo), *args], capture_output=True, text=True, env=env, check=True
    )
    return out.stdout.strip()


class Hone:
    def __init__(self, tmp_path: Path):
        self.rig = make_rig(tmp_path)
        self.bindir, self.log = make_fake_bin(tmp_path)
        docker = self.bindir / "docker"
        docker.write_text(
            _DOCKER.format(python=sys.executable, log=str(self.log), texts=json.dumps(TEXTS))
        )
        docker.chmod(docker.stat().st_mode | stat.S_IEXEC)
        self.B = tmp_path / "bundle"
        (self.B / "bin").mkdir(parents=True)
        self.repo = tmp_path / "a"  # REPO_ROOT of the copied rig
        git(self.repo, "init", "-q")
        (self.repo / "f.txt").write_text("one\n")
        git(self.repo, "add", "f.txt")
        git(
            self.repo,
            "commit",
            "-q",
            "-m",
            "one",
            date="2024-03-05T12:00:00 +0000",
            author_date="2023-01-02T12:00:00 +0000",
        )
        self.first = git(self.repo, "rev-parse", "HEAD")
        git(self.repo, "tag", "v1")
        (self.repo / "f.txt").write_text("two\n")
        git(
            self.repo,
            "commit",
            "-q",
            "-am",
            "two",
            date="2025-07-19T12:00:00 +0000",
            author_date="2022-11-03T12:00:00 +0000",
        )

    def run(self, *flags, image=PINNED, readelf="good", **env):
        full = {
            "HC_BUNDLE_DIR": str(self.B),
            "HC_HONE_REF": "HEAD",
            "FAKE_READELF": readelf,
            **({"HC_RUST_IMAGE": image} if image is not None else {}),
            **env,
        }
        return run_script(
            self.rig,
            "01_build_bundle.sh",
            ["--skip-pi", "--skip-hermes", *flags],
            self.bindir,
            full,
        )

    def runs(self):
        return [c for c in docker_calls(self.log) if c[:1] == ["run"]]

    def build_call(self):
        found = [c for c in self.runs() if "cargo build" in command_text(c)]
        assert len(found) == 1, found
        return found[0]

    def readelf_call(self):
        found = [c for c in self.runs() if is_readelf_call(c)]
        assert len(found) == 1, found
        return found[0]

    def readelf_mode(self) -> str:
        """The mode the fake docker reached for its readelf call (a marker in the call log)."""
        marks = [c[1] for c in calls(self.log) if c[0] == "fake-readelf"]
        assert len(marks) == 1, f"the fake readelf was reached {len(marks)} times"
        return marks[0]

    def stale(self):
        (self.B / "manifest.json").write_text('{"stale": true}\n')
        (self.B / "bin" / "hone").write_text("stale hone\n")


@pytest.fixture
def hone(tmp_path):
    return Hone(tmp_path)


def command_text(call: list[str]) -> str:
    for i, a in enumerate(call):
        if a == "-c" and i + 1 < len(call):
            return call[i + 1]
    return ""


def is_readelf_call(call: list[str]) -> bool:
    text = command_text(call)
    return "readelf" in call or (
        "cargo build" not in text and re.search(r"(^|[\s;&|])readelf\s", text) is not None
    )


def env_pairs(call: list[str]) -> dict[str, str]:
    out = {}
    for i, a in enumerate(call[:-1]):
        if a == "-e" and "=" in call[i + 1]:
            k, v = call[i + 1].split("=", 1)
            out[k] = v
    return out


def has_platform(call: list[str]) -> bool:
    return (
        any(a == "--platform" and call[i + 1] == "linux/amd64" for i, a in enumerate(call[:-1]))
        or "--platform=linux/amd64" in call
    )


# ---------------------------------------------------------------- K166


def test_k166_the_build_command_runs_no_package_manager(hone):
    assert hone.run().returncode == 0
    text = command_text(hone.build_call())
    assert "cargo build" in text
    assert not re.search(r"\bapt(-get)?\b", text)


def test_k166_the_build_runs_in_the_configured_image(hone):
    assert hone.run().returncode == 0
    assert PINNED in hone.build_call()


def test_k166_the_example_env_file_pins_the_full_image_by_digest():
    lines = (ROOT / "config" / "bench.env.example").read_text().splitlines()
    vals = [ln.split("=", 1)[1].strip() for ln in lines if ln.startswith("HC_RUST_IMAGE=")]
    assert vals == [EXAMPLE_IMAGE]


@pytest.mark.parametrize(
    "image",
    [
        "rust:1-bullseye",
        "rust:1-slim-bullseye",
        "rust:1-bullseye@sha256:" + "ab" * 31 + "a",  # 63 hex
        "rust:1-bullseye@sha256:" + "ab" * 32 + "a",  # 65 hex
        "rust:1-bullseye@sha256:" + "AB" * 32,  # upper case
        "rust:1-bullseye@sha256:" + "zz" * 32,  # not hex
        "rust:1-bullseye@sha256:" + "ab" * 32 + " ",  # trailing text
        "rust:1-bullseye@sha1:" + "ab" * 20,
        "rust:1-bullseye@sha256:",
    ],
)
def test_k166_an_unpinned_image_is_refused_before_any_docker_call(hone, image):
    hone.stale()
    r = hone.run(image=image)
    assert r.returncode != 0
    assert "HC_RUST_IMAGE" in r.stderr
    assert docker_calls(hone.log) == []
    assert [c for c in calls(hone.log) if c[0] == "docker"] == []


@pytest.mark.parametrize("flag", ["--skip-hone", "--helpers-only"])
def test_k166_the_image_is_not_checked_when_hone_is_not_built(hone, flag):
    r = hone.run(flag, image="rust:1-slim-bullseye")
    assert r.returncode == 0, r.stderr
    assert docker_calls(hone.log) == []


@pytest.fixture
def container_sandbox(hone, tmp_path):
    """Run the build call's `bash -c` text locally, with the two library dirs rewritten."""
    assert hone.run().returncode == 0
    call = hone.build_call()
    lib, inc = tmp_path / "libdir", tmp_path / "incdir"
    (inc / "openssl").mkdir(parents=True)
    lib.mkdir()
    fakebin = tmp_path / "cbin"
    fakebin.mkdir()
    cargo_log = tmp_path / "cargo.log"
    cargo = fakebin / "cargo"
    cargo.write_text(f'#!/bin/sh\necho "$@" >> {cargo_log}\n')
    cargo.chmod(0o755)
    text = command_text(call).replace("/usr/lib/x86_64-linux-gnu", str(lib))
    text = text.replace("/usr/include", str(inc))

    def run():
        env = env_pairs(call) | {"OPENSSL_LIB_DIR": str(lib), "OPENSSL_INCLUDE_DIR": str(inc)}
        env |= {"PATH": f"{fakebin}:/usr/bin:/bin", "HOME": str(tmp_path)}
        return subprocess.run(
            ["bash", "-c", text], capture_output=True, text=True, env=env, cwd=tmp_path
        )

    for rel in ("libssl.a", "libcrypto.a"):
        (lib / rel).write_text("")
    (inc / "openssl" / "ssl.h").write_text("")
    return run, lib, inc, cargo_log, call


def test_k166_with_the_static_libraries_present_cargo_build_runs(container_sandbox):
    run, _lib, _inc, cargo_log, hone_call = container_sandbox
    assert "cargo build" in command_text(hone_call)
    assert not re.search(r"\bapt(-get)?\b", command_text(hone_call))
    run()  # later steps (install into /out) may fail off the box; only cargo matters here
    assert cargo_log.exists() and "build" in cargo_log.read_text()


@pytest.mark.parametrize("missing", ["libssl.a", "libcrypto.a", "openssl/ssl.h"])
def test_k166_a_missing_static_library_or_header_stops_before_cargo_build(
    container_sandbox, missing
):
    run, lib, inc, cargo_log, _call = container_sandbox
    ((inc if missing.endswith(".h") else lib) / missing).unlink()
    r = run()
    assert r.returncode != 0
    assert "HC_RUST_IMAGE" in r.stderr + r.stdout
    assert not cargo_log.exists()


# ---------------------------------------------------------------- K167


def test_k167_the_build_targets_linux_amd64(hone):
    assert hone.run().returncode == 0
    assert has_platform(hone.build_call())


def test_k167_the_build_sets_the_five_static_link_variables(hone):
    assert hone.run().returncode == 0
    got = env_pairs(hone.build_call())
    assert {
        k: got.get(k)
        for k in (
            "OPENSSL_STATIC",
            "OPENSSL_NO_PKG_CONFIG",
            "OPENSSL_LIB_DIR",
            "OPENSSL_INCLUDE_DIR",
            "LIBZ_SYS_STATIC",
        )
    } == {
        "OPENSSL_STATIC": "1",
        "OPENSSL_NO_PKG_CONFIG": "1",
        "OPENSSL_LIB_DIR": "/usr/lib/x86_64-linux-gnu",
        "OPENSSL_INCLUDE_DIR": "/usr/include",
        "LIBZ_SYS_STATIC": "1",
    }


# ---------------------------------------------------------------- K168


def test_k168_a_good_binary_gives_a_bundle_with_a_manifest(hone):
    hone.stale()
    r = hone.run()
    assert r.returncode == 0, r.stderr
    assert hone.readelf_mode() == "good"
    assert hone.readelf_call()
    assert (hone.B / "bin" / "hone").read_text() != "stale hone\n"
    m = json.loads((hone.B / "manifest.json").read_text())
    assert m.get("stale") is None and m["hone_sha"] == git(hone.repo, "rev-parse", "HEAD")


def test_k168_readelf_runs_in_the_build_image_after_the_build_with_bin_read_only(hone):
    assert hone.run().returncode == 0
    runs = hone.runs()
    build, elf = hone.build_call(), hone.readelf_call()
    assert runs.index(build) < runs.index(elf)
    assert PINNED in elf
    assert "-d" in elf
    assert has_platform(elf)
    assert any(a.startswith(f"{hone.B}/bin:") and a.endswith(":ro") for a in elf)


def test_k168_the_readelf_output_is_saved_under_build(hone):
    assert hone.run().returncode == 0
    saved = [
        p
        for p in (hone.rig / ".build").rglob("*")
        if p.is_file() and "NEEDED" in p.read_text(errors="ignore")
    ]
    assert saved


def test_k168_all_nine_allowed_libraries_pass_the_script(hone):
    r = hone.run(readelf="all_allowed")
    assert r.returncode == 0, r.stderr
    assert hone.readelf_mode() == "all_allowed"
    assert hone.readelf_call()


@pytest.mark.parametrize("readelf", ["bad", "none"])
def test_k168_a_failed_check_removes_bin_hone_writes_no_manifest_and_fails(hone, readelf):
    hone.stale()
    r = hone.run(readelf=readelf)
    assert hone.readelf_mode() == readelf
    assert r.returncode != 0
    assert not (hone.B / "bin" / "hone").exists()
    assert not (hone.B / "manifest.json").exists()


@pytest.mark.parametrize("readelf", ["fail", "fail_good"])
def test_k168_a_failing_readelf_run_removes_bin_hone_writes_no_manifest_and_fails(hone, readelf):
    hone.stale()
    r = hone.run(readelf=readelf)
    assert hone.readelf_mode() == readelf  # the fake reached the failing mode, it did not crash
    assert hone.readelf_call()
    assert r.returncode != 0
    assert not (hone.B / "bin" / "hone").exists()
    assert not (hone.B / "manifest.json").exists()


def test_k168_the_script_failure_names_the_offending_libraries(hone):
    r = hone.run(readelf="bad")
    assert hone.readelf_mode() == "bad"
    out = r.stdout + r.stderr
    for lib in BAD_EXTRA:
        assert lib in out


def _check(tmp_path: Path, text: str):
    f = tmp_path / "readelf.txt"
    f.write_text(text)
    return subprocess.run(
        [sys.executable, "-m", "hcbench.bundle", "check-needed", str(f)],
        capture_output=True,
        text=True,
        cwd=ROOT,
        env={"PYTHONPATH": str(ROOT), "PATH": "/usr/bin:/bin"},
    )


def test_k168_check_needed_accepts_the_good_binary(tmp_path):
    r = _check(tmp_path, GOOD)
    assert r.returncode == 0, r.stderr


def test_k168_check_needed_accepts_every_allowed_library(tmp_path):
    r = _check(tmp_path, ALL_ALLOWED)
    assert r.returncode == 0, r.stderr


def test_k168_check_needed_rejects_openssl_1_1_and_names_every_extra_library(tmp_path):
    r = _check(tmp_path, BAD)
    assert r.returncode != 0
    for lib in BAD_EXTRA:
        assert lib in r.stdout + r.stderr


def test_k168_check_needed_does_not_name_the_allowed_libraries_as_offenders(tmp_path):
    r = _check(tmp_path, BAD)
    assert r.returncode != 0 and "libz.so.1" in r.stdout + r.stderr
    assert "libgcc_s.so.1" not in r.stdout + r.stderr


def test_k168_check_needed_rejects_one_unlisted_library(tmp_path):
    r = _check(tmp_path, readelf_text([*GOOD_LIBS, "libcrypt.so.1"]))
    assert r.returncode != 0
    assert "libcrypt.so.1" in r.stdout + r.stderr


def test_k168_check_needed_ignores_entries_that_are_not_needed(tmp_path):
    # SONAME and RUNPATH lines name libssl.so.1.1 in OTHER; they are not NEEDED entries
    r = _check(tmp_path, GOOD)
    assert "libssl" not in r.stdout + r.stderr and r.returncode == 0


@pytest.mark.parametrize("text", [NONE, "", "\n".join(HEADER) + "\n"])
def test_k168_check_needed_fails_when_there_is_no_needed_entry(tmp_path, text):
    assert _check(tmp_path, GOOD).returncode == 0  # the subcommand exists and accepts a good file
    r = _check(tmp_path, text)
    assert r.returncode != 0
    assert (r.stdout + r.stderr).strip()


# ---------------------------------------------------------------- K169


def test_k169_the_build_passes_the_full_sha_and_the_committer_date_of_head(hone):
    assert hone.run().returncode == 0
    got = env_pairs(hone.build_call())
    assert got.get("HONE_BUILD_GIT_SHA") == git(hone.repo, "rev-parse", "HEAD")
    assert re.fullmatch(r"[0-9a-f]{40}", got["HONE_BUILD_GIT_SHA"])
    assert got.get("HONE_BUILD_GIT_DATE") == "2025-07-19"


def test_k169_a_ref_that_is_not_head_gives_that_commits_sha_and_date(hone):
    assert hone.run(HC_HONE_REF="v1").returncode == 0
    got = env_pairs(hone.build_call())
    assert got["HONE_BUILD_GIT_SHA"] == hone.first
    assert got["HONE_BUILD_GIT_SHA"] != git(hone.repo, "rev-parse", "HEAD")
    assert got["HONE_BUILD_GIT_DATE"] == "2024-03-05"


# ---------------------------------------------------------------- K170


def _readme_section() -> str:
    text = (ROOT / "README.md").read_text()
    m = re.search(r"^.*Hone in the bundle.*$", text, re.M | re.I)
    assert m, "README has no 'Hone in the bundle' paragraph"
    rest = text[m.start() :]
    nxt = re.search(r"^#{1,6} ", rest[len(m.group(0)) :], re.M)
    return rest[: len(m.group(0)) + nxt.start()] if nxt else rest


def test_k170_the_readme_paragraph_names_the_full_image_and_the_four_properties():
    sec = _readme_section()
    assert "rust:1-bullseye" in sec
    assert re.search(r"install(s)? nothing", sec, re.I)
    assert re.search(r"static", sec, re.I) and "OpenSSL" in sec
    assert "NEEDED" in sec
    assert re.search(r"commit", sec, re.I)


def test_k170_the_script_header_has_one_line_with_the_same_points():
    header = []
    for ln in (ROOT / "scripts" / "01_build_bundle.sh").read_text().splitlines()[1:]:
        if not ln.startswith("#"):
            break
        header.append(ln)
    assert any(
        re.search(r"install(s)? nothing", ln, re.I)
        and re.search(r"static", ln, re.I)
        and "NEEDED" in ln
        and re.search(r"commit", ln, re.I)
        for ln in header
    ), header


def test_k170_the_env_example_comment_above_the_image_says_why_full_and_why_debian_11():
    lines = (ROOT / "config" / "bench.env.example").read_text().splitlines()
    i = next(n for n, ln in enumerate(lines) if ln.startswith("HC_RUST_IMAGE="))
    block = []
    for ln in reversed(lines[:i]):
        if not ln.startswith("#"):
            break
        block.append(ln)
    text = " ".join(block)
    assert re.search(r"\bfull\b", text, re.I)
    assert "Debian 11" in text
