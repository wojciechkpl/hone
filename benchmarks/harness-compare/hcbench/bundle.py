"""Pi bundle helpers called by scripts/01_build_bundle.sh (spec 11.12 K65-K67).

Usage: python3 -m hcbench.bundle check-node NODE_VERSION PACKAGE_JSON
       python3 -m hcbench.bundle copy-ext PACKAGE_DIR DEST_DIR
       python3 -m hcbench.bundle manifest-fields PI_PREFIX_DIR
       python3 -m hcbench.bundle check-needed READELF_OUTPUT_FILE
"""

import hashlib
import json
import re
import shutil
import sys
from pathlib import Path

PI_PACKAGE = "@earendil-works/pi-coding-agent"
PI_EXTENSIONS = ("permission-gate", "protected-paths", "confirm-destructive")

# Libraries the Hone binary may need at run time (spec 11.18 K168).
ALLOWED_NEEDED = frozenset(
    {
        "libc.so.6",
        "libm.so.6",
        "libdl.so.2",
        "libpthread.so.0",
        "librt.so.1",
        "libutil.so.1",
        "libresolv.so.2",
        "ld-linux-x86-64.so.2",
        "libgcc_s.so.1",
    }
)
_NEEDED_RE = re.compile(r"\(NEEDED\)\s+Shared library: \[([^\]]+)\]")

_MIN_RE = re.compile(r"^>=(\d+)\.(\d+)\.(\d+)$")
_VERSION_RE = re.compile(r"^v?(\d+)\.(\d+)\.(\d+)$")


def check_node_engine(node_version: str, engines_node: str | None) -> None:
    """Exit unless node_version meets a `>=X.Y.Z` engines.node. Missing or empty passes."""
    if not engines_node:
        return
    m = _MIN_RE.match(engines_node.strip())
    if not m:
        sys.exit(
            f"{PI_PACKAGE} declares engines.node {engines_node!r}, which is not a '>=X.Y.Z' "
            f"minimum; check it by hand against HC_NODE_VERSION={node_version}"
        )
    v = _VERSION_RE.match(node_version.strip())
    if not v:
        sys.exit(f"HC_NODE_VERSION {node_version!r} is not X.Y.Z; check it by hand")
    minimum = tuple(int(x) for x in m.groups())
    if tuple(int(x) for x in v.groups()) < minimum:
        sys.exit(
            f"Node {node_version} is below the {'.'.join(map(str, minimum))} that "
            f"{PI_PACKAGE} requires (engines.node {engines_node})"
        )


def check_needed(readelf_text: str) -> None:
    """Exit unless every NEEDED entry of `readelf -d` output is allowed; none at all fails too."""
    libs = _NEEDED_RE.findall(readelf_text)
    if not libs:
        sys.exit("no NEEDED entry in the readelf output: cannot tell what bin/hone needs")
    extra = sorted({lib for lib in libs if lib not in ALLOWED_NEEDED})
    if extra:
        sys.exit("bin/hone needs libraries outside the allowed list: " + ", ".join(extra))


def copy_pi_extensions(pkg_dir: Path, dest_dir: Path) -> None:
    """Copy the guard extensions from the installed package; exit naming every missing one."""
    src = Path(pkg_dir) / "examples" / "extensions"
    missing = [n for n in PI_EXTENSIONS if not (src / f"{n}.ts").is_file()]
    if missing:
        names = ", ".join(f"{n}.ts" for n in missing)
        sys.exit(f"{PI_PACKAGE} lacks examples/extensions/: {names}")
    for n in PI_EXTENSIONS:
        shutil.copyfile(src / f"{n}.ts", Path(dest_dir) / f"{n}.ts")


def pi_manifest_fields(pi_prefix: Path) -> dict[str, str | None]:
    """Name, version and lock hash of the Pi install under pi_prefix, read from disk."""
    prefix = Path(pi_prefix)
    name = version = None
    pkg_json = prefix / "node_modules" / PI_PACKAGE / "package.json"
    if pkg_json.is_file():
        try:
            data = json.loads(pkg_json.read_text())
        except (OSError, ValueError):
            data = {}
        if isinstance(data, dict):
            name, version = data.get("name"), data.get("version")
    lock = prefix / "package-lock.json"
    digest = hashlib.sha256(lock.read_bytes()).hexdigest() if lock.is_file() else None
    return {"pi_package": name, "pi_installed_version": version, "pi_lock_sha256": digest}


def _engines_node(package_json: Path) -> str | None:
    """engines.node of a package.json; exits asking for a manual check on an odd shape."""
    try:
        data = json.loads(Path(package_json).read_text())
    except (OSError, ValueError):
        sys.exit(f"{package_json} is not valid JSON; check Node against its engines by hand")
    if not isinstance(data, dict):
        sys.exit(f"{package_json} is not a JSON object; check Node against its engines by hand")
    if "engines" not in data:
        return None
    engines = data["engines"]
    if not isinstance(engines, dict):
        sys.exit(f"engines in {package_json} is not an object; check Node against it by hand")
    if "node" not in engines:
        return None
    node = engines["node"]
    if not isinstance(node, str):
        sys.exit(f"engines.node in {package_json} is not a string; check Node against it by hand")
    return node


def main(argv: list[str]) -> None:
    usage = __doc__ or ""
    if not argv:
        sys.exit(usage)
    cmd, args = argv[0], argv[1:]
    if cmd == "check-node" and len(args) == 2:
        check_node_engine(args[0], _engines_node(Path(args[1])))
    elif cmd == "check-needed" and len(args) == 1:
        try:
            text = Path(args[0]).read_text(errors="replace")
        except OSError as exc:
            sys.exit(f"cannot read {args[0]}: {exc}")
        check_needed(text)
    elif cmd == "copy-ext" and len(args) == 2:
        copy_pi_extensions(Path(args[0]), Path(args[1]))
    elif cmd == "manifest-fields" and len(args) == 1:
        if not (Path(args[0]) / "package-lock.json").is_file():
            sys.exit(f"no package-lock.json in {args[0]}: the Pi install did not write a lock")
        print(json.dumps(pi_manifest_fields(Path(args[0]))))
    else:
        sys.exit(usage)


if __name__ == "__main__":
    main(sys.argv[1:])
