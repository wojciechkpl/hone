//! Records the commit hone was built from, for the eval manifest (`HONE_GIT_SHA`,
//! the full sha; empty when the build is not from a git checkout). Shares its
//! resolution (overrides, `.cargo_vcs_info.json`, git) and rebuild tracking with
//! `hone --version` via `hone-build-info`.

fn main() {
    hone_build_info::emit();
}
