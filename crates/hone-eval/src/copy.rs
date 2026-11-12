//! Copying a task directory for one run (F12, F19, F21): gitignore-aware,
//! copy-on-write where the filesystem supports it, with symlinks that stay inside
//! the directory, special files skipped, and an independent git repository when
//! the original is a git worktree.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ignore::WalkBuilder;

/// What a copy did.
#[derive(Debug, Default)]
pub struct CopyReport {
    /// Bytes of file content copied.
    pub bytes: u64,
    /// Notes for the run's `check.txt` (skipped special files).
    pub notes: Vec<String>,
}

/// Copy `source` to `dest`.
///
/// - Honours the directory's own `.gitignore` files (keeping `.git`), and skips
///   `target/` and `node_modules/` at the top level only.
/// - A symlink pointing outside `source` is an error naming it; links inside are
///   recreated as links.
/// - FIFOs, sockets and devices are skipped with a note.
/// - A `.git` directory is copied as is (an independent repository); a `.git`
///   file (a git worktree) is never copied: the copy gets its own repository at
///   the same `HEAD` with the same working-tree contents.
/// - A task directory that is a subdirectory of a repository or worktree gets a
///   fresh repository with its files in one initial commit (no link to the
///   enclosing repository). A copied `.git/config` has `core.worktree` cleared.
pub fn copy_task_dir(source: &Path, dest: &Path) -> Result<CopyReport, String> {
    let canonical = std::fs::canonicalize(source)
        .map_err(|e| format!("cannot read {}: {e}", source.display()))?;
    let kind = classify_git(&canonical);
    std::fs::create_dir_all(dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;

    let mut report = CopyReport::default();
    let walker = WalkBuilder::new(&canonical)
        .hidden(false)
        .parents(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .git_ignore(true)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name();
            if name == ".git" {
                return false; // copied separately
            }
            let top_level_junk = e.depth() == 1
                && (name == "target" || name == "node_modules")
                && e.file_type().is_some_and(|t| t.is_dir());
            !top_level_junk
        })
        .build();
    for entry in walker {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", source.display()))?;
        if entry.depth() == 0 {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(&canonical).unwrap_or(path);
        let dst = dest.join(rel);
        let kind = entry
            .file_type()
            .ok_or_else(|| format!("cannot read {}", path.display()))?;
        if kind.is_dir() {
            std::fs::create_dir_all(&dst)
                .map_err(|e| format!("cannot create {}: {e}", dst.display()))?;
        } else if kind.is_symlink() {
            copy_link(path, &dst, Some(&canonical))?;
        } else if kind.is_file() {
            report.bytes += copy_file(path, &dst)?;
        } else {
            report.notes.push(format!(
                "skipped special file {} (not a regular file)",
                rel.display()
            ));
        }
    }

    let git = canonical.join(".git");
    if git.is_dir() {
        copy_plain(&git, &dest.join(".git"), &mut report)?;
        // A `core.worktree` would point the copy's git at another tree.
        let config = dest.join(".git/config");
        if config.is_file() {
            let _ = Command::new("git")
                .args(["config", "--file"])
                .arg(&config)
                .args(["--unset-all", "core.worktree"])
                .stdin(Stdio::null())
                .output();
        }
    }
    match kind {
        GitKind::Worktree => init_independent_repo(dest, &canonical)?,
        GitKind::Subdirectory => init_snapshot_repo(dest)?,
        GitKind::Plain | GitKind::None => {}
    }
    Ok(report)
}

/// How the task directory relates to git.
enum GitKind {
    /// Not in a repository.
    None,
    /// A repository root with a `.git` directory (copied as is).
    Plain,
    /// A worktree root (`.git` is a file).
    Worktree,
    /// A subdirectory of a repository or worktree.
    Subdirectory,
}

fn classify_git(dir: &Path) -> GitKind {
    for ancestor in dir.ancestors() {
        let Ok(meta) = std::fs::symlink_metadata(ancestor.join(".git")) else {
            continue;
        };
        return match (ancestor == dir, meta.is_file()) {
            (true, true) => GitKind::Worktree,
            (true, false) => GitKind::Plain,
            (false, _) => GitKind::Subdirectory,
        };
    }
    GitKind::None
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Give the copy its own repository at the original's `HEAD`. The new repository
/// only reads the original's object store (through `objects/info/alternates`);
/// everything it writes goes into the copy, so nothing reaches the original. The
/// index matches `HEAD`, so the copy's working-tree contents show as changes.
fn init_independent_repo(copy: &Path, root: &Path) -> Result<(), String> {
    let setup = || -> Result<(), String> {
        let head = git(root, &["rev-parse", "--verify", "-q", "HEAD"]).ok();
        let branch = git(root, &["symbolic-ref", "-q", "--short", "HEAD"]).ok();
        git(copy, &["init", "-q"])?;
        let Some(head) = head else {
            return Ok(()); // an unborn branch: an empty repository is the same state
        };
        let common = git(root, &["rev-parse", "--git-common-dir"])?;
        let objects = root.join(common).join("objects");
        let objects = std::fs::canonicalize(&objects)
            .map_err(|e| format!("cannot find the object store {}: {e}", objects.display()))?;
        let info_dir = copy.join(".git/objects/info");
        std::fs::create_dir_all(&info_dir)
            .and_then(|()| {
                std::fs::write(
                    info_dir.join("alternates"),
                    format!("{}\n", objects.display()),
                )
            })
            .map_err(|e| format!("cannot write alternates: {e}"))?;
        match &branch {
            Some(b) => git(copy, &["symbolic-ref", "HEAD", &format!("refs/heads/{b}")])?,
            None => String::new(),
        };
        git(copy, &["reset", "-q", "--mixed", &head])?;
        if branch.is_none() {
            git(copy, &["checkout", "-q", "--detach"])?;
        }
        Ok(())
    };
    setup().map_err(|e| {
        format!(
            "cannot give the copy of the git worktree {} its own repository (git): {e}",
            root.display()
        )
    })
}

/// Copy a directory tree as it is (no ignore rules), for `.git`.
fn copy_plain(from: &Path, to: &Path, report: &mut CopyReport) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("cannot create {}: {e}", to.display()))?;
    let entries =
        std::fs::read_dir(from).map_err(|e| format!("cannot read {}: {e}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", from.display()))?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry
            .file_type()
            .map_err(|e| format!("cannot read {}: {e}", src.display()))?;
        if kind.is_dir() {
            copy_plain(&src, &dst, report)?;
        } else if kind.is_symlink() {
            copy_link(&src, &dst, None)?;
        } else if kind.is_file() {
            report.bytes += copy_file(&src, &dst)?;
        }
    }
    Ok(())
}

/// Recreate a symlink. With `confine` set, a target outside that directory is an error.
fn copy_link(src: &Path, dst: &Path, confine: Option<&Path>) -> Result<(), String> {
    let target =
        std::fs::read_link(src).map_err(|e| format!("cannot read link {}: {e}", src.display()))?;
    if let Some(root) = confine {
        let parent = src.parent().unwrap_or(root);
        let resolved = if target.is_absolute() {
            target.clone()
        } else {
            parent.join(&target)
        };
        // Resolve through existing links; fall back to a lexical result for a dangling one.
        let resolved = std::fs::canonicalize(&resolved).unwrap_or_else(|_| lexical(&resolved));
        if !resolved.starts_with(root) {
            return Err(format!(
                "symlink {} points outside the task directory (to {}): refusing to copy it",
                src.display(),
                target.display()
            ));
        }
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, dst)
        .map_err(|e| format!("cannot create link {}: {e}", dst.display()))?;
    #[cfg(not(unix))]
    let _ = dst;
    Ok(())
}

/// Resolve `.` and `..` without touching the filesystem.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Copy one file, cloning it copy-on-write where the filesystem can.
fn copy_file(src: &Path, dst: &Path) -> Result<u64, String> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let size = std::fs::metadata(src)
        .map_err(|e| format!("cannot read {}: {e}", src.display()))?
        .len();
    if clone_file(src, dst) {
        return Ok(size);
    }
    std::fs::copy(src, dst)
        .map_err(|e| format!("cannot copy {} to {}: {e}", src.display(), dst.display()))
}

#[cfg(target_os = "macos")]
fn clone_file(src: &Path, dst: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let (Ok(s), Ok(d)) = (
        std::ffi::CString::new(src.as_os_str().as_bytes()),
        std::ffi::CString::new(dst.as_os_str().as_bytes()),
    ) else {
        return false;
    };
    // SAFETY: both arguments are valid NUL-terminated paths that outlive the call.
    unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), 0) == 0 }
}

#[cfg(target_os = "linux")]
fn clone_file(src: &Path, dst: &Path) -> bool {
    use std::os::fd::AsRawFd;
    // FICLONE = _IOW(0x94, 9, int)
    const FICLONE: libc::c_ulong = 0x4004_9409;
    let (Ok(s), Ok(d)) = (std::fs::File::open(src), std::fs::File::create(dst)) else {
        return false;
    };
    // SAFETY: both descriptors are open for the duration of the call.
    let ok = unsafe { libc::ioctl(d.as_raw_fd(), FICLONE, s.as_raw_fd()) == 0 };
    if ok {
        if let Ok(m) = std::fs::metadata(src) {
            let _ = std::fs::set_permissions(dst, m.permissions());
        }
    } else {
        drop(d);
        let _ = std::fs::remove_file(dst);
    }
    ok
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn clone_file(_src: &Path, _dst: &Path) -> bool {
    false
}

/// A fresh repository for the copy of a subdirectory of a larger repository: the
/// task's files in one initial commit, with no link to the enclosing repository.
fn init_snapshot_repo(copy: &Path) -> Result<(), String> {
    let run = || -> Result<(), String> {
        git(copy, &["init", "-q"])?;
        git(copy, &["add", "-A"])?;
        git(
            copy,
            &[
                "-c",
                "user.name=hone-eval",
                "-c",
                "user.email=eval@hone.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "task files",
            ],
        )?;
        Ok(())
    };
    run().map_err(|e| format!("cannot create a git repository for the copy (git): {e}"))
}
