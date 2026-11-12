#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! E2, E3, E4 and the sources of E7 of `docs/plans/env-snapshot-spec.md`, at the
//! source itself, independent of the machine: fake executables in a temp dir stand
//! in for the toolchains, and the probe search path is overridden.
//!
//! API required (new, in `hone_core::strategy`):
//!
//! ```ignore
//! pub struct EnvSnapshot { .. }
//! impl EnvSnapshot {
//!     pub fn new(working_dir: impl Into<PathBuf>, timeout_secs: u64) -> Self;
//!     /// The PATH used to find the probe programs (and given to them as their PATH).
//!     /// Without it the process's PATH is used.
//!     pub fn with_search_path(self, path: impl Into<std::ffi::OsString>) -> Self;
//!     /// None when nothing at all was gathered. Never errors, never prints.
//!     pub async fn gather(&self) -> Option<EnvBlock>;
//! }
//! pub struct EnvBlock { pub text: String, pub sources: Vec<hone_proto::ScoredSource> }
//! ```
//!
//! Block format (pinned here; the spec fixes the order and the wording of the
//! headers only):
//!
//! ```text
//! # Environment
//! OS: <std::env::consts::OS> <ARCH>[ (kernel <uname -r>)]
//! Working directory: <working_dir as given>
//! Top-level entries:
//!   <name>[/]            one per line, sorted, dirs suffixed, hidden skipped
//!   … and N more         after 50 entries (U+2026)
//! Toolchains:
//!   <program>: <first line of its output, at most 120 chars>   probe-list order
//! Memory: <N> GiB        (omitted when it cannot be read)
//! ```
//! A header whose list is empty is omitted. Toolchain probes (fixed argv, no shell,
//! stdin closed, cwd = working dir, run in parallel): `<prog> --version`, except
//! `java -version`, whose output is on stderr; every other probe reads stdout.
//! Probe order: rustc cargo python3 node npm go java ruby deno bun uv pip3 poetry
//! pnpm yarn git make cmake docker.
//!
//! Sources (spec E7): one `SourceRef::Command { argv, sha256 }` per probe that
//! contributed a line, `sha256` = `sha256_hex` of the probe's raw captured output,
//! `argv` = program name as in the probe list plus its arguments; the listing is
//! `Command { argv: ["<builtin:listing>"], .. }`. Scores are `None`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hone_core::strategy::EnvSnapshot;

/// Creating a script and executing it from parallel threads races with `fork` (ETXTBSY on
/// Linux): every test holds this for its whole duration, so no other test's fork can inherit
/// the still-open write handle of a fake executable. (M5 of the env-snapshot review.)
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
use hone_proto::{SourceRef, sha256_hex};

struct Fixture {
    _tmp: tempfile::TempDir,
    bin: PathBuf,
    work: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let (bin, work) = (root.join("bin"), root.join("work"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        Self { _tmp: tmp, bin, work }
    }

    /// An executable `bin/<name>` with the given `sh` body.
    fn fake(&self, name: &str, body: &str) {
        let path = self.bin.join(name);
        std::fs::write(&path, format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A fake that prints one line to stdout.
    fn version(&self, name: &str, line: &str) {
        self.fake(name, &format!("echo '{line}'"));
    }

    fn snapshot(&self, timeout_secs: u64) -> EnvSnapshot {
        EnvSnapshot::new(&self.work, timeout_secs).with_search_path(self.bin.as_os_str())
    }
}

fn alive(pid: &str) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    !stat.is_empty() && !stat.starts_with('Z')
}

fn gone_within(pid: &str, limit: Duration) -> bool {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if !alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !alive(pid)
}

fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

fn pos(text: &str, needle: &str) -> usize {
    text.lines()
        .position(|l| l.starts_with(needle))
        .unwrap_or_else(|| panic!("no line starting {needle:?} in:\n{text}"))
}

// ---- E2: what it gathers ------------------------------------------------------------------

#[tokio::test]
async fn test_e2_block_should_have_the_documented_header_and_line_order() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new();
    f.version("rustc", "rustc 1.99.0 (fake)");
    std::fs::write(f.work.join("a.txt"), "x").unwrap();

    let b = f.snapshot(15).gather().await.expect("a block");

    let t = &b.text;
    assert_eq!(lines(t)[0], "# Environment");
    let os = format!("OS: {} {}", std::env::consts::OS, std::env::consts::ARCH);
    assert!(lines(t)[1].starts_with(&os), "{}", lines(t)[1]);
    assert_eq!(lines(t)[2], format!("Working directory: {}", f.work.display()));
    assert!(pos(t, "Working directory:") < pos(t, "Top-level entries:"));
    assert!(pos(t, "Top-level entries:") < pos(t, "Toolchains:"));
    if t.contains("\nMemory:") {
        assert!(pos(t, "Toolchains:") < pos(t, "Memory:"), "Memory comes last:\n{t}");
        let mem = lines(t)[pos(t, "Memory:")];
        assert!(mem.ends_with(" GiB"), "{mem}");
    }
    assert!(t.contains("\n  rustc: rustc 1.99.0 (fake)\n") || t.ends_with("\n  rustc: rustc 1.99.0 (fake)"), "{t}");
}

#[tokio::test]
async fn test_e2_top_level_entries_should_be_sorted_skip_hidden_and_mark_directories() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new();
    for name in ["b.txt", "a.txt", ".env", ".hidden"] {
        std::fs::write(f.work.join(name), "x").unwrap();
    }
    for d in ["target", "src", ".git"] {
        std::fs::create_dir(f.work.join(d)).unwrap();
    }

    let b = f.snapshot(15).gather().await.unwrap();

    let start = pos(&b.text, "Top-level entries:");
    let entries: Vec<&str> = lines(&b.text)[start + 1..start + 5].to_vec();
    assert_eq!(entries, ["  a.txt", "  b.txt", "  src/", "  target/"]);
    assert!(!b.text.contains(".git") && !b.text.contains(".env") && !b.text.contains(".hidden"));
}

#[tokio::test]
async fn test_e2_listing_should_stop_at_fifty_entries_and_count_the_rest() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new();
    for i in 0..60 {
        std::fs::write(f.work.join(format!("file_{i:02}.txt")), "x").unwrap();
    }

    let b = f.snapshot(15).gather().await.unwrap();

    let start = pos(&b.text, "Top-level entries:");
    let listed = &lines(&b.text)[start + 1..start + 51];
    assert!(listed.iter().all(|l| l.starts_with("  file_")), "{listed:?}");
    assert_eq!(listed[0], "  file_00.txt");
    assert_eq!(listed[49], "  file_49.txt");
    assert_eq!(lines(&b.text)[start + 51], "  … and 10 more");
    assert!(!b.text.contains("file_50.txt"));
}

#[tokio::test]
async fn test_e2_exactly_fifty_entries_should_not_get_a_more_line() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new();
    for i in 0..50 {
        std::fs::write(f.work.join(format!("f{i:02}")), "x").unwrap();
    }
    let b = f.snapshot(15).gather().await.unwrap();
    assert!(!b.text.contains("more"), "{}", b.text);
}

#[tokio::test]
async fn test_e2_toolchains_should_list_present_programs_in_probe_order_with_their_first_line() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new();
    // Created in a different order than the probe order.
    f.version("git", "git version 2.45.0");
    f.fake("cargo", "echo 'cargo 1.99.0 (fake)'; echo 'second line'");
    f.fake("java", "echo 'openjdk version \"21.0.1\"' >&2");
    f.version("python3", "Python 3.12.1");

    let b = f.snapshot(15).gather().await.unwrap();

    let start = pos(&b.text, "Toolchains:");
    assert_eq!(
        &lines(&b.text)[start + 1..start + 5],
        [
            "  cargo: cargo 1.99.0 (fake)",
            "  python3: Python 3.12.1",
            "  java: openjdk version \"21.0.1\"",
            "  git: git version 2.45.0",
        ]
    );
    assert!(!b.text.contains("second line"), "only the first line");
    assert!(!b.text.contains("rustc:") && !b.text.contains("docker:"), "absent programs have no line");
}

#[tokio::test]
async fn test_e2_a_long_version_line_should_be_cut_to_120_characters() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new();
    f.fake("git", &format!("echo '{}'", "x".repeat(300)));

    let b = f.snapshot(15).gather().await.unwrap();

    assert!(b.text.contains(&format!("\n  git: {}\n", "x".repeat(120))) || b.text.ends_with(&format!("\n  git: {}", "x".repeat(120))), "{}", b.text);
    assert!(!b.text.contains(&"x".repeat(121)));
}

#[tokio::test]
async fn test_e2_toolchains_header_should_be_omitted_when_no_probe_succeeds() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new(); // an empty bin dir
    let b = f.snapshot(15).gather().await.unwrap();
    assert!(!b.text.contains("Toolchains:"), "{}", b.text);
    assert!(b.text.contains("Working directory:"));
}

// ---- E3: no shell, bounded -------------------------------------------------------------------------

#[tokio::test]
async fn test_e3_probes_should_run_with_a_fixed_argv_and_closed_stdin_in_an_empty_directory() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new();
    let args = f.work.parent().unwrap().join("args.txt");
    std::fs::write(f.work.join("repo-file.txt"), "x").unwrap();
    // Records its arguments, would hang on an open stdin, prints its cwd (E9: not the repository).
    f.fake("git", &format!("echo \"$@\" >> '{}'; cat > /dev/null; pwd -P", args.display()));
    f.fake("java", &format!("echo \"$@\" >> '{}'; echo ok >&2", args.display()));

    let b = f.snapshot(15).gather().await.unwrap();

    let recorded = std::fs::read_to_string(&args).unwrap();
    let mut got: Vec<&str> = recorded.lines().collect();
    got.sort_unstable();
    assert_eq!(got, ["--version", "-version"], "argv is fixed: {recorded:?}");
    let line = b.text.lines().find_map(|l| l.strip_prefix("  git: ")).expect("a git line (stdin closed: cat returned)");
    assert_ne!(std::path::Path::new(line), f.work, "the probe must not run in the working directory:\n{}", b.text);
    assert!(!std::path::Path::new(line).starts_with(&f.work), "nor below it: {line}");
}

#[tokio::test]
async fn test_e3_probes_should_run_in_parallel() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new();
    for name in ["rustc", "cargo", "python3", "node", "npm"] {
        f.fake(name, &format!("sleep 1; echo '{name} ok'"));
    }

    let start = Instant::now();
    let b = f.snapshot(15).gather().await.unwrap();
    let took = start.elapsed();

    for name in ["rustc", "cargo", "python3", "node", "npm"] {
        assert!(b.text.contains(&format!("  {name}: {name} ok")), "{name}");
    }
    assert!(took < Duration::from_millis(2500), "five 1 s probes took {took:?}: they run in parallel");
}

#[tokio::test]
async fn test_e3_a_probe_should_be_killed_after_three_seconds_with_its_process_group() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new();
    let pids = f.work.parent().unwrap();
    f.fake(
        "node",
        &format!("echo $$ > '{0}/leader.pid'; sleep 30 & echo $! > '{0}/child.pid'; wait", pids.display()),
    );
    f.version("git", "git version 2.45.0");

    let start = Instant::now();
    let b = f.snapshot(15).gather().await.unwrap();
    let took = start.elapsed();

    assert!(took >= Duration::from_millis(2800) && took < Duration::from_secs(5), "per-probe timeout is 3 s: {took:?}");
    assert!(!b.text.contains("node:"), "a timed-out probe has no line");
    assert!(b.text.contains("  git: git version 2.45.0"), "the others are kept");
    for file in ["leader.pid", "child.pid"] {
        let pid = std::fs::read_to_string(pids.join(file)).unwrap();
        assert!(gone_within(pid.trim(), Duration::from_secs(2)), "{file} ({}) still running", pid.trim());
    }
}

#[tokio::test]
async fn test_e3_the_whole_snapshot_should_finish_within_timeout_secs() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new();
    f.fake("node", "sleep 30");
    f.version("git", "git version 2.45.0");

    let start = Instant::now();
    let b = f.snapshot(1).gather().await.unwrap();
    let took = start.elapsed();

    assert!(took < Duration::from_millis(2200), "timeout_secs = 1 took {took:?}");
    assert!(!b.text.contains("node:"));
    assert!(b.text.contains("  git: git version 2.45.0"), "finished probes are kept");
}

// ---- E4: fails silently -------------------------------------------------------------------------------

#[tokio::test]
async fn test_e4_failing_empty_and_missing_probes_should_leave_their_lines_out() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new();
    f.fake("go", "echo 'go broke' >&2; exit 1");
    f.fake("ruby", "true"); // succeeds with no output
    f.fake("deno", "echo 'deno 2.0'; exit 3"); // non-zero exit: omitted even with output
    f.version("git", "git version 2.45.0");

    let b = f.snapshot(15).gather().await.unwrap();

    for absent in ["go:", "ruby:", "deno:", "go broke", "docker:"] {
        assert!(!b.text.contains(absent), "{absent} should be omitted:\n{}", b.text);
    }
    assert!(b.text.contains("  git: git version 2.45.0"));
}

#[tokio::test]
async fn test_e4_an_unreadable_working_directory_should_not_fail_the_snapshot() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new();
    f.version("git", "git version 2.45.0");
    let missing = f.work.join("no-such-dir");

    let b = EnvSnapshot::new(&missing, 5)
        .with_search_path(f.bin.as_os_str())
        .gather()
        .await
        .expect("the OS line is still there");

    assert!(b.text.starts_with("# Environment\nOS: "));
    assert!(!b.text.contains("Top-level entries:"), "{}", b.text);
}

// ---- E7: sources ---------------------------------------------------------------------------------------

#[tokio::test]
async fn test_e7_each_contributing_probe_and_the_listing_should_be_a_command_source() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new();
    f.version("rustc", "rustc 1.99.0 (fake)");
    f.fake("java", "echo 'openjdk version \"21\"' >&2");
    f.fake("go", "exit 1");
    std::fs::write(f.work.join("a.txt"), "x").unwrap();

    let b = f.snapshot(15).gather().await.unwrap();

    let commands: Vec<(Vec<String>, String)> = b
        .sources
        .iter()
        .map(|s| {
            assert!(s.score.is_none());
            match &s.source {
                SourceRef::Command { argv, sha256 } => (argv.clone(), sha256.clone()),
                other => panic!("env-snapshot sources are commands: {other:?}"),
            }
        })
        .collect();
    let find = |argv: &[&str]| {
        commands
            .iter()
            .find(|(a, _)| a.iter().map(String::as_str).eq(argv.iter().copied()))
            .map(|(_, h)| h.clone())
    };
    assert_eq!(find(&["rustc", "--version"]).unwrap(), sha256_hex(b"rustc 1.99.0 (fake)\n"));
    assert_eq!(find(&["java", "-version"]).unwrap(), sha256_hex(b"openjdk version \"21\"\n"));
    assert!(find(&["<builtin:listing>"]).is_some(), "the directory listing");
    assert!(find(&["go", "--version"]).is_none(), "a failed probe is not a source");
    let hashes_ok = commands.iter().all(|(_, h)| h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit()));
    assert!(hashes_ok);
}

#[tokio::test]
async fn test_e7_toolchain_sources_should_follow_the_probe_order() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::new();
    f.version("git", "g");
    f.version("rustc", "r");
    f.version("python3", "p");

    let b = f.snapshot(15).gather().await.unwrap();

    let order: Vec<String> = b
        .sources
        .iter()
        .filter_map(|s| match &s.source {
            SourceRef::Command { argv, .. } if ["rustc", "python3", "git"].contains(&argv[0].as_str()) => Some(argv[0].clone()),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["rustc", "python3", "git"]);
}

#[allow(dead_code)]
fn unused(_: &Path) {}
