//! The `env-snapshot` context source: one block describing the machine and the
//! working directory, gathered once before the first model call.
//!
//! Every probe is a fixed argv (never a shell, never anything from the prompt or
//! the repository), run in parallel with stdin closed, in a fresh empty
//! temporary directory (never inside the repository, so repository files such as
//! `.yarnrc`, `package.json` or `rust-toolchain.toml` cannot redirect a
//! toolchain), with a minimal environment, and found only through absolute
//! `PATH` entries outside the working directory. A probe that is missing, exits
//! non-zero, prints nothing or runs out of time is left out; nothing is ever
//! printed or returned as an error. No probe process outlives its probe.
//! See `docs/plans/env-snapshot-spec.md`.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use hone_proto::{ScoredSource, SourceRef, sha256_hex};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::task::JoinHandle;

use hone_tools::ProcessGroupGuard;

/// How long one probe may run.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// Longest line kept from a probe's output.
const MAX_LINE_CHARS: usize = 120;
/// Most directory entries listed.
const MAX_ENTRIES: usize = 50;
/// Most directory entries read.
const MAX_ENTRIES_READ: usize = 5_000;
/// Longest listed name, in characters.
const MAX_NAME_CHARS: usize = 80;
/// Longest block, in characters.
const MAX_BLOCK_CHARS: usize = 6_000;
/// Most output bytes read from one probe.
const MAX_PROBE_OUTPUT: u64 = 64 * 1024;
/// How long to wait for a probe's pipes to drain after it exited.
const DRAIN_GRACE: Duration = Duration::from_millis(500);
/// Where the system probes (`uname`, `sysctl`, `sw_vers`) are looked up,
/// whatever the toolchain search path says.
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
/// The variables a probe inherits, when the caller has them.
const INHERITED_ENV: &[&str] = &["HOME", "LANG", "LC_ALL", "SYSTEMROOT"];
/// Settings every probe gets, so a toolchain never downloads or runs anything
/// the repository names.
const FIXED_ENV: &[(&str, &str)] = &[
    ("YARN_IGNORE_PATH", "1"),
    ("RUSTUP_AUTO_INSTALL", "0"),
    ("GOTOOLCHAIN", "local"),
    ("COREPACK_ENABLE_NETWORK", "0"),
    ("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"),
    ("npm_config_manage_package_manager_versions", "false"),
];

/// Which stream a probe's answer is on.
#[derive(Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

/// Which line of the answer is kept.
#[derive(Clone, Copy)]
enum Pick {
    /// The first line.
    First,
    /// The first line containing ` version "` (java prints other lines first).
    VersionLine,
}

/// `(program, args, stream, line)`, in output order.
const PROBES: &[(&str, &[&str], Stream, Pick)] = &[
    ("rustc", &["--version"], Stream::Stdout, Pick::First),
    ("cargo", &["--version"], Stream::Stdout, Pick::First),
    ("python3", &["--version"], Stream::Stdout, Pick::First),
    ("node", &["--version"], Stream::Stdout, Pick::First),
    ("npm", &["--version"], Stream::Stdout, Pick::First),
    ("go", &["version"], Stream::Stdout, Pick::First),
    ("java", &["-version"], Stream::Stderr, Pick::VersionLine),
    ("ruby", &["--version"], Stream::Stdout, Pick::First),
    ("deno", &["--version"], Stream::Stdout, Pick::First),
    ("bun", &["--version"], Stream::Stdout, Pick::First),
    ("uv", &["--version"], Stream::Stdout, Pick::First),
    ("pip3", &["--version"], Stream::Stdout, Pick::First),
    ("poetry", &["--version"], Stream::Stdout, Pick::First),
    ("pnpm", &["--version"], Stream::Stdout, Pick::First),
    ("yarn", &["--version"], Stream::Stdout, Pick::First),
    ("git", &["--version"], Stream::Stdout, Pick::First),
    ("make", &["--version"], Stream::Stdout, Pick::First),
    ("cmake", &["--version"], Stream::Stdout, Pick::First),
    ("docker", &["--version"], Stream::Stdout, Pick::First),
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

/// A probe's answer: its chosen line and the hash of its raw output.
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

    /// The `PATH` used to find the probe programs, and given to them as theirs
    /// (only its absolute entries outside the working directory). Without it the
    /// process's own `PATH` is used.
    #[must_use]
    pub fn with_search_path(mut self, path: impl Into<OsString>) -> Self {
        self.search_path = Some(path.into());
        self
    }

    /// The block, or `None` when nothing at all was gathered. Never errors.
    pub async fn gather(&self) -> Option<EnvBlock> {
        let secs = Duration::from_secs(self.timeout_secs.max(1));
        // Each probe's own limit; with the listing's, this bounds the whole gather.
        let limit = PROBE_TIMEOUT.min(secs);
        let home = std::env::var_os("HOME").map(PathBuf::from);

        let raw_path = self
            .search_path
            .clone()
            .or_else(|| std::env::var_os("PATH"))
            .unwrap_or_default();
        let tool_path = usable_path(&raw_path, &self.working_dir, home.as_deref());
        let system_path = usable_path(OsStr::new(SYSTEM_PATH), &self.working_dir, home.as_deref());
        let tmp = probe_temp_parent(&self.working_dir);
        let tmp = tmp.as_path();
        let home_ref = home.as_deref();

        // The listing runs off the async runtime and is dropped if it is late.
        let work = self.working_dir.clone();
        let listing =
            tokio::time::timeout(secs, tokio::task::spawn_blocking(move || list_dir(&work)));

        let toolchains =
            futures::future::join_all(PROBES.iter().map(|(prog, args, stream, pick)| {
                run_probe(prog, args, *stream, *pick, &tool_path, tmp, limit)
            }));
        let kernel = run_probe(
            "uname",
            &["-r"],
            Stream::Stdout,
            Pick::First,
            &system_path,
            tmp,
            limit,
        );
        let product = product_version(&system_path, tmp, limit);
        let memory = memory_gib(&system_path, tmp, limit);
        let (listing, toolchains, kernel, product, memory) =
            tokio::join!(listing, toolchains, kernel, product, memory);
        let listing = listing.ok().and_then(Result::ok).flatten();

        let mut sources: Vec<ScoredSource> = Vec::new();
        let mut lines: Vec<String> = vec!["# Environment".to_owned()];

        let mut os = format!("OS: {} {}", std::env::consts::OS, std::env::consts::ARCH);
        let mut details = Vec::new();
        if let Some((argv, a)) = &product {
            details.push(format!("{} {}", macos_name(), shown(&a.line, home_ref)));
            let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
            sources.push(command(&argv, &a.sha256));
        }
        if let Some(k) = &kernel {
            details.push(format!("kernel {}", shown(&k.line, home_ref)));
            sources.push(command(&["uname", "-r"], &k.sha256));
        }
        if !details.is_empty() {
            os.push_str(&format!(" ({})", details.join(", ")));
        }
        lines.push(os);
        lines.push(format!(
            "Working directory: {}",
            tilde(&self.working_dir.display().to_string(), home.as_deref())
        ));

        if let Some((entries, hash)) = listing {
            lines.push(entries);
            sources.push(command(&["<builtin:listing>"], &hash));
        }

        let mut tool_lines = Vec::new();
        for ((prog, args, _, _), answer) in PROBES.iter().zip(&toolchains) {
            if let Some(a) = answer {
                tool_lines.push(format!("  {prog}: {}", shown(&a.line, home_ref)));
                let mut argv = vec![*prog];
                argv.extend(args.iter().copied());
                sources.push(command(&argv, &a.sha256));
            }
        }
        if !tool_lines.is_empty() {
            lines.push("Toolchains:".to_owned());
            lines.extend(tool_lines);
        }

        if let Some((gib, argv, hash)) = memory {
            lines.push(format!("Memory: {gib} GiB"));
            let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
            sources.push(command(&argv, &hash));
        }

        Some(EnvBlock {
            text: cap_block(lines),
            sources,
        })
    }
}

/// The product name of the OS for the OS line.
fn macos_name() -> &'static str {
    "macOS"
}

/// Join `lines`, dropping lines from the end (replaced by `… (truncated)`) when
/// the block would pass 6,000 characters.
fn cap_block(mut lines: Vec<String>) -> String {
    let marker = "… (truncated)";
    let size = |l: &[String]| l.iter().map(|x| x.chars().count() + 1).sum::<usize>();
    if size(&lines) <= MAX_BLOCK_CHARS {
        return lines.join("\n");
    }
    while lines.len() > 2 && size(&lines) + marker.chars().count() > MAX_BLOCK_CHARS {
        lines.pop();
    }
    lines.push(marker.to_owned());
    lines.join("\n")
}

/// A probe's line as shown: home replaced by `~`, then cut to 120 characters.
fn shown(line: &str, home: Option<&Path>) -> String {
    tilde(line, home).chars().take(MAX_LINE_CHARS).collect()
}

/// Replace the home directory in `text` with `~` where it is a whole path
/// prefix (followed by `/`, a non-path character or the end).
fn tilde(text: &str, home: Option<&Path>) -> String {
    let Some(h) = home.and_then(Path::to_str).map(|h| h.trim_end_matches('/')) else {
        return text.to_owned();
    };
    if h.is_empty() {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(h) {
        let after = &rest[i + h.len()..];
        let boundary = after
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.')));
        out.push_str(&rest[..i]);
        if boundary {
            out.push('~');
        } else {
            out.push_str(h);
        }
        rest = after;
    }
    out.push_str(rest);
    out
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

/// The `PATH` entries a probe may be found through: absolute, not empty, and not
/// inside the working directory (a repository could plant a binary there). The
/// inside check is skipped when the working directory is `/`, `home` or an
/// ancestor of `home`: everything the user installed is then "inside" it.
fn usable_path(path: &OsStr, working_dir: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let work_real = std::fs::canonicalize(working_dir).ok();
    let work_cmp = work_real.as_deref().unwrap_or(working_dir);
    let broad = work_cmp.parent().is_none()
        || home.is_some_and(|h| {
            let h = std::fs::canonicalize(h).unwrap_or_else(|_| h.to_path_buf());
            h.starts_with(work_cmp)
        });
    std::env::split_paths(path)
        .filter(|p| !p.as_os_str().is_empty() && p.is_absolute())
        .filter(|p| {
            if broad {
                return true;
            }
            if p.starts_with(working_dir) {
                return false;
            }
            match (&work_real, std::fs::canonicalize(p)) {
                (Some(w), Ok(real)) => !real.starts_with(w),
                _ => true,
            }
        })
        .collect()
}

/// The directory probes' scratch directories are made in: `TMPDIR` when it is
/// absolute, exists and is not inside the working directory (after resolving
/// symlinks); otherwise the OS default (`/tmp` on unix). Always absolute.
fn probe_temp_parent(working_dir: &Path) -> PathBuf {
    let work = std::fs::canonicalize(working_dir).unwrap_or_else(|_| working_dir.to_path_buf());
    let wanted = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .and_then(|p| std::fs::canonicalize(p).ok())
        .filter(|p| p.is_dir() && !p.starts_with(&work));
    wanted.unwrap_or_else(|| {
        #[cfg(unix)]
        let default = PathBuf::from("/tmp");
        #[cfg(not(unix))]
        let default = std::env::temp_dir();
        std::fs::canonicalize(&default).unwrap_or(default)
    })
}

/// Whether `p` is a file the current user may run.
#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Whether `p` is a file with an extension (execute bits do not exist here).
#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file() && p.extension().is_some()
}

/// Find `program` in `dirs`. On Windows the `PATHEXT` extensions are tried and
/// extensionless files are skipped.
fn find_program(program: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        dirs.iter()
            .map(|d| d.join(program))
            .find(|p| is_executable(p))
    }
    #[cfg(not(unix))]
    {
        let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
        dirs.iter().find_map(|dir| {
            exts.split(';')
                .filter(|e| !e.is_empty())
                .map(|e| dir.join(format!("{program}{e}")))
                .find(|p| is_executable(p))
        })
    }
}

/// Aborts a reader task when dropped.
struct AbortOnDrop(JoinHandle<Vec<u8>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn read_capped<R: tokio::io::AsyncRead + Unpin + Send + 'static>(pipe: Option<R>) -> AbortOnDrop {
    AbortOnDrop(tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(pipe) = pipe {
            let _ = pipe.take(MAX_PROBE_OUTPUT).read_to_end(&mut buf).await;
        }
        buf
    }))
}

/// Run one probe. `None` for anything but a clean, non-empty answer.
async fn run_probe(
    program: &str,
    args: &[&str],
    stream: Stream,
    pick: Pick,
    dirs: &[PathBuf],
    tmp: &Path,
    limit: Duration,
) -> Option<Answer> {
    let exe = find_program(program, dirs)?;
    // A fresh empty directory outside the repository: repository files cannot
    // reach the toolchain.
    let cwd = tempfile::tempdir_in(tmp).ok()?;
    let path = std::env::join_paths(dirs).ok()?;

    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env_clear()
        .env("PATH", path)
        .env("TMPDIR", tmp)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(cwd.path())
        .kill_on_drop(true);
    for key in INHERITED_ENV {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    for (k, v) in FIXED_ENV {
        cmd.env(k, v);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so the whole tree can be taken down.
        cmd.as_std_mut().process_group(0);
    }
    let mut child = cmd.spawn().ok()?;
    // Registered with hone's running-command registry while it runs, so an
    // interrupt or any exit path that stops commands stops probes too. Dropping
    // the guard kills the group.
    let mut group = child.id().map(ProcessGroupGuard::new);
    let out = read_capped(child.stdout.take());
    let err = read_capped(child.stderr.take());

    let status = match tokio::time::timeout(limit, child.wait()).await {
        Ok(Ok(status)) => status,
        // Timed out or failed: the guard kills the group.
        _ => return None,
    };
    // The probe exited: end anything it left behind (a child holding the pipes
    // open), then take what was read.
    if let Some(g) = group.as_mut() {
        g.kill();
    }
    let drain = |h: AbortOnDrop| async move {
        let mut h = h;
        let bytes = tokio::time::timeout(DRAIN_GRACE, &mut h.0)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        drop(h);
        bytes
    };
    let (stdout, stderr) = tokio::join!(drain(out), drain(err));
    if !status.success() {
        return None;
    }
    let raw = match stream {
        Stream::Stdout => stdout,
        Stream::Stderr => stderr,
    };
    let text = String::from_utf8_lossy(&raw);
    let line = match pick {
        Pick::First => text.lines().next(),
        Pick::VersionLine => text.lines().find(|l| l.contains(" version \"")),
    }?
    .trim();
    if line.is_empty() {
        return None;
    }
    Some(Answer {
        line: line.to_owned(),
        sha256: sha256_hex(&raw),
    })
}

/// `sw_vers -productVersion` on macOS: the OS line's product version.
async fn product_version(
    dirs: &[PathBuf],
    tmp: &Path,
    limit: Duration,
) -> Option<(Vec<String>, Answer)> {
    #[cfg(target_os = "macos")]
    {
        let a = run_probe(
            "sw_vers",
            &["-productVersion"],
            Stream::Stdout,
            Pick::First,
            dirs,
            tmp,
            limit,
        )
        .await?;
        Some((vec!["sw_vers".into(), "-productVersion".into()], a))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (dirs, tmp, limit);
        None
    }
}

/// `name` with control and direction-changing characters escaped, cut to 80
/// characters (with `…`): a file name must not become instructions.
fn show_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_control() || matches!(c, '\u{200e}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
            out.push_str(&format!("\\u{{{:x}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    if out.chars().count() > MAX_NAME_CHARS {
        let mut cut: String = out.chars().take(MAX_NAME_CHARS - 1).collect();
        cut.push('…');
        cut
    } else {
        out
    }
}

/// The sorted top-level listing as one `Top-level entries (JSON): [...]` line
/// (hidden entries skipped, directories suffixed, names escaped and cut as data
/// before encoding, at most 50 shown, then a last `… and N more` element; at
/// most 5,000 entries read) and the hash of it. `None` when the directory cannot
/// be read or is empty.
fn list_dir(dir: &Path) -> Option<(String, String)> {
    let mut names: Vec<String> = Vec::new();
    let mut more = false;
    for (read, entry) in std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .enumerate()
    {
        if read >= MAX_ENTRIES_READ {
            more = true;
            break;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        names.push(if is_dir { format!("{name}/") } else { name });
    }
    if names.is_empty() {
        return None;
    }
    names.sort();
    let total = names.len();
    let mut items: Vec<String> = names
        .iter()
        .take(MAX_ENTRIES)
        .map(|n| show_name(n))
        .collect();
    if more {
        items.push("… and more".to_owned());
    } else if total > MAX_ENTRIES {
        items.push(format!("… and {} more", total - MAX_ENTRIES));
    }
    let json = serde_json::to_string(&items).ok()?;
    let line = format!("Top-level entries (JSON): {json}");
    let hash = sha256_hex(line.as_bytes());
    Some((line, hash))
}

/// Total RAM in GiB, with the argv of what told us and the hash of its output.
async fn memory_gib(
    dirs: &[PathBuf],
    tmp: &Path,
    limit: Duration,
) -> Option<(u64, Vec<String>, String)> {
    #[cfg(target_os = "macos")]
    {
        let answer = run_probe(
            "sysctl",
            &["-n", "hw.memsize"],
            Stream::Stdout,
            Pick::First,
            dirs,
            tmp,
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
        let _ = (dirs, tmp, limit);
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
        let _ = (dirs, tmp, limit);
        None
    }
}
