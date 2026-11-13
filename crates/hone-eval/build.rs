//! Records the git SHA hone was built from, for the eval manifest (`HONE_GIT_SHA`;
//! empty when the build is not from a git checkout).

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn main() {
    let sha = git(&["rev-parse", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=HONE_GIT_SHA={sha}");
    println!("cargo:rerun-if-changed=build.rs");
    // Re-run when HEAD moves, and when the branch it names moves (a commit changes
    // `refs/heads/<branch>` or `packed-refs`, not `HEAD`).
    for path in ["HEAD", "packed-refs"] {
        if let Some(p) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={p}");
        }
    }
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(p) = git(&["rev-parse", "--git-path", &branch]) {
            println!("cargo:rerun-if-changed={p}");
        }
    }
}
