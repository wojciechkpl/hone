//! The `env-snapshot` context source: one block describing the machine and the
//! working directory, gathered once before the first model call.
//!
//! Every probe is a fixed argv (never a shell, never anything from the prompt or
//! the repository) run with stdin closed, in the working directory, in parallel.
//! A probe that is missing, exits non-zero, prints nothing or runs out of time is
//! left out; nothing is ever printed or returned as an error.
//! See `docs/plans/env-snapshot-spec.md`.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use hone_proto::{ScoredSource, SourceRef, sha256_hex};
use tokio::process::Command;

/// How long one probe may run.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// Longest line kept from a probe's output.
const MAX_LINE_CHARS: usize = 120;
/// Most directory entries listed.
const MAX_ENTRIES: usize = 50;
/// Where the system probes (`uname`, `sysctl`) are looked up, whatever the
/// toolchain search path says.
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// Which stream a probe's answer is on.
#[derive(Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

/// `(program, args, stream)`, in output order.
const PROBES: &[(&str, &[&str], Stream)] = &[
    ("rustc", &["--version"], Stream::Stdout),
    ("cargo", &["--version"], Stream::Stdout),
    ("python3", &["--version"], Stream::Stdout),
    ("node", &["--version"], Stream::Stdout),
    ("npm", &["--version"], Stream::Stdout),
    ("go", &["--version"], Stream::Stdout),
    ("java", &["-version"], Stream::Stderr),
    ("ruby", &["--version"], Stream::Stdout),
    ("deno", &["--version"], Stream::Stdout),
    ("bun", &["--version"], Stream::Stdout),
    ("uv", &["--version"], Stream::Stdout),
    ("pip3", &["--version"], Stream::Stdout),
    ("poetry", &["--version"], Stream::Stdout),
    ("pnpm", &["--version"], Stream::Stdout),
    ("yarn", &["--version"], Stream::Stdout),
    ("git", &["--version"], Stream::Stdout),
    ("make", &["--version"], Stream::Stdout),
    ("cmake", &["--version"], Stream::Stdout),
    ("docker", &["--version"], Stream::Stdout),
];

/// The gathered block and where each part came from.
#[derive(Debug, Clone)]
pub struct EnvBlock {
    pub text: String,
    /// One `Command` source per probe that contributed a line, plus the listing.
    pub sources: Vec<ScoredSource>,
}

/// Gathers the environment block.
#[derive(Debug, Clone)]
pub struct EnvSnapshot {
    working_dir: PathBuf,
    timeout_secs: u64,
    search_path: Option<OsString>,
}

/// A probe's answer: its first line and the hash of its raw output.
struct Answer {
    line: String,
    sha256: String,
}

impl EnvSnapshot {
    pub fn new(working_dir: impl Into<PathBuf>, timeout_secs: u64) -> Self {
        Self {
            working_dir: working_dir.into(),
            timeout_secs,
            search_path: None,
        }
    }

    /// The `PATH` used to find the probe programs, and given to them as theirs.
    /// Without it the process's own `PATH` is used.
    #[must_use]
    pub fn with_search_path(mut self, path: impl Into<OsString>) -> Self {
        self.search_path = Some(path.into());
        self
    }

    /// The block, or `None` when nothing at all was gathered. Never errors.
    pub async fn gather(&self) -> Option<EnvBlock> {
        // Every probe is bounded by this, so the whole snapshot is too.
        let limit = PROBE_TIMEOUT.min(Duration::from_secs(self.timeout_secs.max(1)));
        let tool_path = self
            .search_path
            .clone()
            .or_else(|| std::env::var_os("PATH"))
            .unwrap_or_default();

        let toolchains = futures::future::join_all(PROBES.iter().map(|(prog, args, stream)| {
            run_probe(prog, args, *stream, &tool_path, &self.working_dir, limit)
        }));
        let kernel = run_probe(
            "uname",
            &["-r"],
            Stream::Stdout,
            OsStr::new(SYSTEM_PATH),
            &self.working_dir,
            limit,
        );
        let memory = memory_gib(&self.working_dir, limit);
        let (toolchains, kernel, memory) = tokio::join!(toolchains, kernel, memory);

        let mut sources: Vec<ScoredSource> = Vec::new();
        let mut text = String::from("# Environment\n");

        let mut os = format!("OS: {} {}", std::env::consts::OS, std::env::consts::ARCH);
        if let Some(k) = &kernel {
            os.push_str(&format!(" (kernel {})", k.line));
            sources.push(command(&["uname", "-r"], &k.sha256));
        }
        text.push_str(&os);
        text.push('\n');
        text.push_str(&format!(
            "Working directory: {}\n",
            self.working_dir.display()
        ));

        if let Some((listing, hash)) = list_dir(&self.working_dir) {
            text.push_str("Top-level entries:\n");
            text.push_str(&listing);
            sources.push(command(&["<builtin:listing>"], &hash));
        }

        let mut lines = Vec::new();
        for ((prog, args, _), answer) in PROBES.iter().zip(&toolchains) {
            if let Some(a) = answer {
                lines.push(format!("  {prog}: {}", a.line));
                let mut argv = vec![*prog];
                argv.extend(args.iter().copied());
                sources.push(command(&argv, &a.sha256));
            }
        }
        if !lines.is_empty() {
            text.push_str("Toolchains:\n");
            text.push_str(&lines.join("\n"));
            text.push('\n');
        }

        if let Some((gib, argv, hash)) = memory {
            text.push_str(&format!("Memory: {gib} GiB\n"));
            let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
            sources.push(command(&argv, &hash));
        }

        while text.ends_with('\n') {
            text.pop();
        }
        Some(EnvBlock { text, sources })
    }
}

fn command(argv: &[&str], sha256: &str) -> ScoredSource {
    ScoredSource {
        source: SourceRef::Command {
            argv: argv.iter().map(|s| (*s).to_owned()).collect(),
            sha256: sha256.to_owned(),
        },
        score: None,
    }
}

/// Whether `p` is a file the current user may run.
#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Whether `p` is a file (execute bits do not exist here; `.exe` and friends
/// are found by trying the usual extensions).
#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Find `program` in the directories of `path`.
fn find_program(program: &str, path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path).find_map(|dir| {
        let plain = dir.join(program);
        if is_executable(&plain) {
            return Some(plain);
        }
        #[cfg(not(unix))]
        for ext in ["exe", "cmd", "bat"] {
            let candidate = dir.join(format!("{program}.{ext}"));
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
        None
    })
}

/// Run one probe. `None` for anything but a clean, non-empty answer.
async fn run_probe(
    program: &str,
    args: &[&str],
    stream: Stream,
    path: &OsStr,
    cwd: &Path,
    limit: Duration,
) -> Option<Answer> {
    let exe = find_program(program, path)?;
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if cwd.is_dir() {
        cmd.current_dir(cwd);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so a timeout can take the whole tree down.
        cmd.as_std_mut().process_group(0);
    }
    let child = cmd.spawn().ok()?;
    let pid = child.id();
    let output = match tokio::time::timeout(limit, child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        _ => {
            kill_group(pid);
            return None;
        }
    };
    if !output.status.success() {
        return None;
    }
    let raw = match stream {
        Stream::Stdout => output.stdout,
        Stream::Stderr => output.stderr,
    };
    let text = String::from_utf8_lossy(&raw);
    let first = text.lines().next()?.trim();
    if first.is_empty() {
        return None;
    }
    Some(Answer {
        line: first.chars().take(MAX_LINE_CHARS).collect(),
        sha256: sha256_hex(&raw),
    })
}

/// SIGKILL the process group `pid` leads.
#[cfg(unix)]
fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) {
        // SAFETY: `killpg` only signals the group created for this probe
        // (`process_group(0)` makes the child its leader); `pid > 0`.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
}

/// Without process groups the direct child is killed when its handle drops
/// (`kill_on_drop`), which has already happened by the time this runs.
#[cfg(not(unix))]
fn kill_group(_pid: Option<u32>) {}

/// The sorted top-level listing (hidden entries skipped, directories suffixed,
/// at most 50) and the hash of it. `None` when the directory cannot be read.
fn list_dir(dir: &Path) -> Option<(String, String)> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            Some(if is_dir { format!("{name}/") } else { name })
        })
        .collect();
    if names.is_empty() {
        return None;
    }
    names.sort();
    let total = names.len();
    let mut out = String::new();
    for n in names.iter().take(MAX_ENTRIES) {
        out.push_str(&format!("  {n}\n"));
    }
    if total > MAX_ENTRIES {
        out.push_str(&format!("  … and {} more\n", total - MAX_ENTRIES));
    }
    let hash = sha256_hex(out.as_bytes());
    Some((out, hash))
}

/// Total RAM in GiB, with the argv of what told us and the hash of its output.
async fn memory_gib(cwd: &Path, limit: Duration) -> Option<(u64, Vec<String>, String)> {
    #[cfg(target_os = "macos")]
    {
        let answer = run_probe(
            "sysctl",
            &["-n", "hw.memsize"],
            Stream::Stdout,
            OsStr::new(SYSTEM_PATH),
            cwd,
            limit,
        )
        .await?;
        let bytes: u64 = answer.line.trim().parse().ok()?;
        let gib = (bytes as f64 / (1u64 << 30) as f64).round() as u64;
        Some((
            gib,
            vec!["sysctl".into(), "-n".into(), "hw.memsize".into()],
            answer.sha256,
        ))
    }
    #[cfg(target_os = "linux")]
    {
        let _ = (cwd, limit);
        let raw = std::fs::read("/proc/meminfo").ok()?;
        let text = String::from_utf8_lossy(&raw);
        let kb: u64 = text
            .lines()
            .find_map(|l| l.strip_prefix("MemTotal:"))?
            .split_whitespace()
            .next()?
            .parse()
            .ok()?;
        let gib = (kb as f64 / (1u64 << 20) as f64).round() as u64;
        Some((gib, vec!["<builtin:meminfo>".into()], sha256_hex(&raw)))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (cwd, limit);
        None
    }
}
