//! Policy subjects: what a tool call touches (`docs/plans/policy-p2.2-spec.md` §3, §4).
//!
//! Contains the best-effort shell parser and path canonicalisation used by
//! [`crate::Tool::subjects`]. Command rules are a speed bump; the sandbox is the
//! security boundary.

use std::path::{Component, Path, PathBuf};

use hone_config::policy::{PathOp, Subject};

/// Maximum nesting depth of `$(…)` / backtick substitutions.
const MAX_DEPTH: usize = 8;
/// Longest shell command that is parsed.
const MAX_INPUT_BYTES: usize = 64 * 1024;
/// Caps that keep policy evaluation cheap on hostile input; exceeding one marks
/// the parse as `unparsed`.
const MAX_COMMANDS: usize = 1000;
const MAX_PATHS: usize = 1000;

/// What a tool call would touch.
#[derive(Debug, Clone, Default)]
pub struct ToolSubjects {
    /// Subjects to evaluate against the policy.
    pub subjects: Vec<Subject>,
    /// True when a shell command could not be parsed.
    pub unparsed: bool,
    /// True when a non-first pipeline stage runs a shell or interpreter that reads
    /// its program from standard input (`curl … | sh`).
    pub pipe_to_interpreter: bool,
    /// Per subject, extra context a remembered approval must match (leading
    /// assignments, wrapper words, working directory). May be shorter than
    /// `subjects`; a missing entry means no context.
    pub contexts: Vec<String>,
    /// True when a path-position argument holds a parameter expansion or a command
    /// substitution, so the real path is unknown.
    pub var_path: bool,
    /// The same paths as written (symlinks not resolved), where that differs from
    /// the canonical path in `subjects`. Evaluated after `subjects`.
    pub alt_subjects: Vec<Subject>,
    /// True when the call runs a nested command string, a heredoc script or `source`:
    /// no remembered approval may match it, and it cannot be remembered.
    pub no_remember: bool,
}

impl ToolSubjects {
    /// Context string of subject `i` (empty when none).
    pub fn context_of(&self, i: usize) -> &str {
        self.contexts.get(i).map_or("", String::as_str)
    }

    /// Add the canonical path for `raw` and, when it differs, the path as written.
    pub(crate) fn add_raw_path(&mut self, op: PathOp, raw: &str, sctx: &SubjectContext) {
        let canonical = subject_path(raw, sctx);
        let lexical = lexical_subject_path(raw, sctx);
        self.add_path(op, canonical.clone());
        if lexical != canonical {
            let s = Subject::Path { path: lexical, op };
            if !self.alt_subjects.contains(&s) {
                self.alt_subjects.push(s);
            }
        }
    }

    /// Add a path subject unless an identical one is already present.
    pub(crate) fn add_path(&mut self, op: PathOp, path: PathBuf) {
        let s = Subject::Path { path, op };
        if !self.subjects.contains(&s) {
            self.subjects.push(s);
        }
    }

    /// Path subjects for `raw` with each op in `ops`.
    pub(crate) fn paths(raw: Option<&str>, ops: &[PathOp], sctx: &SubjectContext) -> Self {
        let mut ts = ToolSubjects::default();
        if let Some(raw) = raw {
            for op in ops {
                ts.add_raw_path(*op, raw, sctx);
            }
        }
        ts
    }

    /// Read of a search root (`grep`/`glob`); defaults to the workdir.
    pub(crate) fn search_root(path: Option<&str>, sctx: &SubjectContext) -> Self {
        let mut ts = ToolSubjects::default();
        match path {
            Some(p) => ts.add_raw_path(PathOp::Read, p, sctx),
            None => ts.add_path(PathOp::Read, sctx.workdir.clone()),
        }
        ts
    }

    /// Read + write for each distinct `edits[].path`.
    pub(crate) fn multi_edit(input: &serde_json::Value, sctx: &SubjectContext) -> Self {
        let mut ts = ToolSubjects::default();
        let edits = input.get("edits").and_then(|e| e.as_array());
        for path in edits
            .into_iter()
            .flatten()
            .filter_map(|e| e.get("path")?.as_str())
        {
            ts.add_raw_path(PathOp::Read, path, sctx);
            ts.add_raw_path(PathOp::Write, path, sctx);
        }
        ts
    }

    /// Read + write for each target named in a unified diff's headers.
    pub(crate) fn patch(diff: Option<&str>, sctx: &SubjectContext) -> Self {
        let mut ts = ToolSubjects::default();
        let Some(Ok(files)) = diff.map(crate::file::patch::parse_unified_diff) else {
            return ts;
        };
        for f in files {
            for path in [f.source_path, f.target_path].into_iter().flatten() {
                let raw = path.to_string_lossy();
                ts.add_raw_path(PathOp::Read, &raw, sctx);
                ts.add_raw_path(PathOp::Write, &raw, sctx);
            }
        }
        ts
    }

    /// A single `git <sub>[ <arg>]` command subject.
    pub(crate) fn git(tool_name: &str, arg: Option<&str>) -> Self {
        let sub = tool_name.strip_prefix("git_").unwrap_or(tool_name);
        let cmd = match arg {
            Some(a) => format!("git {sub} {a}"),
            None => format!("git {sub}"),
        };
        ToolSubjects {
            subjects: vec![Subject::Command(cmd)],
            ..Default::default()
        }
    }

    /// A host subject for a URL, taken from the URL parser.
    pub(crate) fn host_of_url(url: &str) -> Self {
        let host = reqwest::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| url.to_string());
        ToolSubjects {
            subjects: vec![Subject::Host(host)],
            ..Default::default()
        }
    }
}

/// What subject extraction needs; built from the policy context.
#[derive(Debug, Clone)]
pub struct SubjectContext {
    /// Canonical working directory.
    pub workdir: PathBuf,
    /// Home directory, if known.
    pub home: Option<PathBuf>,
    /// Fold argv[0] to lowercase (case-insensitive filesystems).
    pub case_insensitive: bool,
}

/// Result of [`parse_shell`].
#[derive(Debug, Clone, Default)]
pub struct ShellParse {
    /// Normalised argv per simple command.
    pub commands: Vec<Vec<String>>,
    /// Paths touched, in source order, via [`subject_path`].
    pub paths: Vec<(PathOp, PathBuf)>,
    /// True when the input could not be fully parsed.
    pub unparsed: bool,
    /// See [`ToolSubjects::pipe_to_interpreter`].
    pub pipe_to_interpreter: bool,
    /// Context per entry of `commands`; see [`ToolSubjects::contexts`].
    pub contexts: Vec<String>,
    /// See [`ToolSubjects::var_path`].
    pub var_path: bool,
    /// Paths as written, in source order; see [`ToolSubjects::alt_subjects`].
    pub alt_paths: Vec<(PathOp, PathBuf)>,
    /// See [`ToolSubjects::no_remember`].
    pub no_remember: bool,
}

impl ShellParse {
    /// Convert to policy subjects (`Command` = argv joined by single spaces).
    pub fn into_subjects(self) -> ToolSubjects {
        let mut subjects: Vec<Subject> = self
            .commands
            .iter()
            .map(|argv| Subject::Command(argv.join(" ")))
            .collect();
        subjects.extend(
            self.paths
                .into_iter()
                .map(|(op, path)| Subject::Path { path, op }),
        );
        ToolSubjects {
            subjects,
            unparsed: self.unparsed,
            pipe_to_interpreter: self.pipe_to_interpreter,
            contexts: self.contexts,
            var_path: self.var_path,
            alt_subjects: self
                .alt_paths
                .into_iter()
                .map(|(op, path)| Subject::Path { path, op })
                .collect(),
            no_remember: self.no_remember,
        }
    }
}

// -- paths --------------------------------------------------------------------

/// Resolve a raw path to the form the policy matches against.
///
/// Expands `~`, joins relative paths onto the workdir, canonicalises the longest
/// existing prefix (symlinks, on-disk case) with OS `..` semantics, follows
/// dangling symlinks (up to 40 hops), and appends the rest lexically. No
/// containment check.
pub fn subject_path(raw: &str, sctx: &SubjectContext) -> PathBuf {
    let expanded = match &sctx.home {
        Some(h) if raw == "~" => h.clone(),
        Some(h) if raw.starts_with("~/") => h.join(&raw[2..]),
        _ => PathBuf::from(raw),
    };
    let abs = if expanded.is_absolute() {
        expanded
    } else {
        sctx.workdir.join(expanded)
    };
    resolve_path(&abs)
}

/// The path as written: `~` expanded, relative paths joined onto the workdir, `.`
/// and `..` folded lexically. Symlinks are not resolved.
pub fn lexical_subject_path(raw: &str, sctx: &SubjectContext) -> PathBuf {
    lexical_in(&sctx.workdir, raw, sctx)
}

/// True if `src` looks like it defines a function in any form: `name()`, `name ()`,
/// `function name`. Conservative (a false positive only makes loop variables unknown).
fn has_function_def(src: &str) -> bool {
    let b = src.as_bytes();
    let is_word = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b':');
    for (i, &c) in b.iter().enumerate() {
        if c == b'(' {
            // `(` + optional blanks + `)` right after a name.
            let mut j = i + 1;
            while j < b.len() && matches!(b[j], b' ' | b'\t') {
                j += 1;
            }
            if b.get(j) == Some(&b')') {
                let mut k = i;
                while k > 0 && matches!(b[k - 1], b' ' | b'\t') {
                    k -= 1;
                }
                if k > 0 && is_word(b[k - 1]) {
                    return true;
                }
            }
        }
    }
    // The `function` keyword as a word.
    src.match_indices("function").any(|(i, _)| {
        let before_ok = i == 0 || !is_word(b[i - 1]);
        let after = b.get(i + "function".len());
        before_ok && after.is_some_and(|a| matches!(a, b' ' | b'\t'))
    })
}

/// Fold `.` and `..` lexically in an absolute path.
fn fold_lexical(abs: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn lexical_in(base: &Path, raw: &str, sctx: &SubjectContext) -> PathBuf {
    let expanded = match &sctx.home {
        Some(h) if raw == "~" => h.clone(),
        Some(h) if raw.starts_with("~/") => h.join(&raw[2..]),
        _ => PathBuf::from(raw),
    };
    let abs = if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Maximum symlink hops followed while resolving a path.
const MAX_HOPS: usize = 40;

enum Piece {
    Root(std::ffi::OsString),
    Cur,
    Parent,
    Normal(std::ffi::OsString),
}

fn pieces(p: &Path) -> impl Iterator<Item = Piece> + '_ {
    p.components().map(|c| match c {
        Component::Prefix(_) | Component::RootDir => Piece::Root(c.as_os_str().to_os_string()),
        Component::CurDir => Piece::Cur,
        Component::ParentDir => Piece::Parent,
        Component::Normal(s) => Piece::Normal(s.to_os_string()),
    })
}

/// Canonicalise the longest existing prefix of an absolute path, following
/// dangling symlinks, and append the rest lexically. Never fails.
pub(crate) fn resolve_path(p: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    let mut queue: std::collections::VecDeque<Piece> = pieces(p).collect();
    let mut cur = PathBuf::new();
    // Number of trailing components of `cur` that do not exist.
    let mut missing = 0usize;
    let mut hops = 0usize;
    while let Some(piece) = queue.pop_front() {
        match piece {
            Piece::Root(r) => {
                cur.push(r);
                missing = 0;
            }
            Piece::Cur => {}
            Piece::Parent => {
                cur.pop();
                missing = missing.saturating_sub(1);
            }
            Piece::Normal(s) => {
                cur.push(s);
                if missing > 0 {
                    missing += 1;
                    continue;
                }
                if let Ok(c) = std::fs::canonicalize(&cur) {
                    cur = c;
                    continue;
                }
                let is_link =
                    std::fs::symlink_metadata(&cur).is_ok_and(|m| m.file_type().is_symlink());
                let target = if is_link {
                    std::fs::read_link(&cur).ok()
                } else {
                    None
                };
                match target {
                    Some(t) => {
                        hops += 1;
                        if hops > MAX_HOPS {
                            return cur;
                        }
                        cur.pop();
                        for piece in pieces(&t).collect::<Vec<_>>().into_iter().rev() {
                            queue.push_front(piece);
                        }
                    }
                    None => missing = 1,
                }
            }
        }
    }
    cur
}

// -- shell parser -------------------------------------------------------------

struct Fail;
type Res<T> = Result<T, Fail>;

#[derive(Clone, Copy, PartialEq)]
enum Term {
    Eof,
    Paren,
    Backtick,
    Arith,
}

type Lit = (char, bool);

/// A `for` variable bound over literal words or the matches of a literal glob.
struct BoundVar {
    name: String,
    values: Vec<String>,
    valid: bool,
    /// Function nesting when the loop was entered; function bodies see no outer bindings.
    fn_level: usize,
}

/// Most combinations of bound values substituted into one word.
const MAX_COMBINATIONS: usize = 32;

/// Result of substituting bound `for` variables into a word.
enum Subst {
    /// The word does not (only) refer to bound variables.
    NotBound,
    /// More than [`MAX_COMBINATIONS`] combinations.
    TooMany,
    /// One word per combination.
    Words(Vec<Word>),
}

/// Text fed to a command's stdin by a heredoc or here-string.
struct StdinText {
    text: String,
    /// Unknown before it runs: an unquoted body with a substitution, or a here-string
    /// with an expansion.
    denied: bool,
}

/// Maximum words produced by brace expansion, and glob matches per word.
const MAX_BRACE_WORDS: usize = 64;
const MAX_GLOB_MATCHES: usize = 256;
const MAX_DIR_ENTRIES: usize = 4096;
const MAX_BASES: usize = 16;

#[derive(Default, Clone)]
struct Word {
    text: String,
    /// Per char of `text`: true if it was quoted or escaped (a literal).
    lit: Vec<bool>,
    started: bool,
    /// Contains an unexpanded variable or substitution.
    var: bool,
    /// Contains an unquoted glob character.
    glob: bool,
    /// Began with an unquoted `~`.
    tilde: bool,
    /// Any quoting or escaping was used.
    quoted: bool,
    /// Chars of a leading `$HOME` / `${HOME}` at the start of `text`.
    home_chars: usize,
    start: usize,
    /// A process substitution `<(…)` used as an argument.
    procsub: bool,
}

impl Word {
    fn push(&mut self, c: char, lit: bool) {
        self.text.push(c);
        self.lit.push(lit);
        self.started = true;
        if !lit && matches!(c, '*' | '?' | '[') {
            self.glob = true;
        }
    }

    fn lits(&self) -> Vec<Lit> {
        self.text.chars().zip(self.lit.iter().copied()).collect()
    }
}

#[derive(Default)]
struct Cmd {
    words: Vec<Word>,
    /// This command is a later stage of a pipeline (`a | THIS`).
    piped: bool,
}

struct Parser<'a> {
    chars: Vec<char>,
    pos: usize,
    sctx: &'a SubjectContext,
    commands: Vec<Vec<String>>,
    paths: Vec<(usize, PathOp, PathBuf)>,
    /// Lexical (as-written) alternatives of `paths`.
    alt_paths: Vec<(usize, PathOp, PathBuf)>,
    /// Lexical forms found while expanding the current word.
    alt_buf: std::cell::RefCell<Vec<PathBuf>>,
    unparsed: bool,
    pipe_to_interpreter: bool,
    /// Context per entry of `commands`.
    contexts: Vec<String>,
    var_path: bool,
    /// The command being parsed has a here-string with a substitution.
    here_subst: bool,
    /// Heredoc or here-string text fed to the command being parsed.
    stdin_text: Option<StdinText>,
    /// Where the next heredoc body starts (after the previous one on this line).
    heredoc_scan: Option<usize>,
    /// The first command of a nested parse is a later pipeline stage.
    inherit_piped: bool,
    /// A nested string, heredoc script or `source` was seen.
    no_remember: bool,
    /// Variables exported earlier (`export`, `declare -x`, `set -a`, subshell assignments).
    exports: Vec<String>,
    auto_export: bool,
    subshell: usize,
    /// Active `for` loops, innermost last (`None` for loops that bind nothing).
    loops: Vec<Option<BoundVar>>,
    /// The `for` header seen, waiting for its `do`.
    pending_for: Option<BoundVar>,
    /// After `cd` to a target that isn't literal, relative paths are unknown (sticky).
    cwd_unknown: bool,
    /// The line defines a function somewhere: no `for` binding is trusted.
    line_has_function: bool,
    /// `{` nesting, the brace depths at which function bodies started, and whether
    /// the next `{` opens a function body.
    brace_depth: usize,
    func_brace: Vec<usize>,
    func_pending: bool,
    in_backtick: bool,
    /// Inside `$((…))` / `((…))`: `<` and `>` are operators, not redirects.
    arith: bool,
    /// Directories relative paths may resolve against (workdir plus `cd` targets).
    bases: Vec<PathBuf>,
    depth: usize,
    glob_words: std::cell::Cell<usize>,
    glob_overflow: std::cell::Cell<bool>,
    /// Pending heredocs: (delimiter, strip leading tabs).
    heredocs: Vec<(Vec<char>, bool)>,
}

/// Parse a shell command line, best effort.
pub fn parse_shell(cmd: &str, sctx: &SubjectContext) -> ShellParse {
    if cmd.len() > MAX_INPUT_BYTES {
        return ShellParse {
            unparsed: true,
            ..ShellParse::default()
        };
    }
    let mut p = Parser::new(cmd, sctx, vec![sctx.workdir.clone()]);
    if p.parse_list(Term::Eof, 0).is_err() {
        p.unparsed = true;
    }
    p.unparsed |= p.glob_overflow.get();
    let mut paths = p.paths;
    paths.sort_by_key(|(pos, _, _)| *pos);
    let mut alt_paths = p.alt_paths;
    alt_paths.sort_by_key(|(pos, _, _)| *pos);
    ShellParse {
        alt_paths: alt_paths.into_iter().map(|(_, op, path)| (op, path)).collect(),
        commands: p.commands,
        paths: paths.into_iter().map(|(_, op, path)| (op, path)).collect(),
        unparsed: p.unparsed,
        pipe_to_interpreter: p.pipe_to_interpreter,
        contexts: p.contexts,
        var_path: p.var_path,
        no_remember: p.no_remember,
    }
}

fn is_blank(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r')
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_assignment(text: &str) -> bool {
    match text.split_once('=') {
        Some((name, _)) => {
            let mut cs = name.chars();
            cs.next().is_some_and(is_name_start)
                && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

const WRITERS: &[&str] = &[
    "rm", "rmdir", "mv", "cp", "touch", "mkdir", "ln", "chmod", "chown", "tee", "truncate",
    "install",
];

const RESERVED: &[&str] = &[
    "{", "}", "!", "if", "then", "else", "elif", "fi", "while", "until", "do", "done", "time",
];

/// How many leading words a wrapper command (and its options) occupies.
fn wrapper_skip(words: &[Word], name: &str) -> Option<usize> {
    let (take, assigns, extra): (&[&str], bool, usize) = match name {
        "env" => (
            &["-u", "-C", "-S", "--unset", "--chdir", "--split-string"],
            true,
            0,
        ),
        "command" | "builtin" | "nohup" => (&[], false, 0),
        "exec" => (&["-a"], false, 0),
        "sudo" => (
            &["-u", "-g", "-h", "-p", "-C", "-D", "-r", "-t", "-U", "-T"],
            true,
            0,
        ),
        "doas" => (&["-u", "-C"], false, 0),
        "timeout" => (&["-s", "-k", "--signal", "--kill-after"], false, 1),
        "nice" => (&["-n", "--adjustment"], false, 0),
        "stdbuf" => (&["-i", "-o", "-e"], false, 0),
        "time" => (&["-f", "-o", "--format", "--output"], false, 0),
        "arch" => (&["-c"], false, 0),
        "caffeinate" => (&["-t", "-w"], false, 0),
        "setsid" | "coproc" | "busybox" | "toybox" => (&[], false, 0),
        "ionice" => (&["-c", "-n", "-p"], false, 0),
        "unbuffer" => (&[], false, 0),
        "flock" => (&["-w", "-E", "--timeout", "--conflict-exit-code"], false, 1),
        "chroot" => (&["--userspec", "--groups"], false, 1),
        "script" => {
            // `script -c CMD FILE` (util-linux) has a command string; BSD
            // `script [-q] FILE CMD...` runs the command that follows the file.
            if words
                .iter()
                .skip(1)
                .any(|w| is_c_bundle(&w.text) || w.text.starts_with("--command"))
            {
                return None;
            }
            (&[], false, 1)
        }
        "taskset" => (&["-c"], false, 1),
        "strace" => (&["-e", "-o", "-p", "-s", "-u", "-E", "-S", "-a"], false, 0),
        "ltrace" => (&["-e", "-o", "-p", "-s", "-u", "-n"], false, 0),
        "xargs" => (
            &["-I", "-n", "-P", "-s", "-d", "-E", "-L", "-a", "-J"],
            false,
            0,
        ),
        _ => return None,
    };
    let mut i = 1;
    while let Some(w) = words.get(i) {
        let t = w.text.as_str();
        if t == "--" {
            i += 1;
            break;
        }
        if t.starts_with('-') && t.len() > 1 {
            i += if take.contains(&t) { 2 } else { 1 };
        } else if assigns && is_assignment(t) {
            i += 1;
        } else {
            break;
        }
    }
    let extra = if name == "taskset"
        && words[1..i.min(words.len())]
            .iter()
            .any(|w| matches!(w.text.as_str(), "-c" | "--cpu-list"))
    {
        0
    } else {
        extra
    };
    Some(i + extra)
}

/// True if a `sed` argument asks for in-place editing (`-i`, `-i.bak`, `-ni`, `--in-place`).
fn is_sed_in_place(t: &str) -> bool {
    if t == "--in-place" || t.starts_with("--in-place=") {
        return true;
    }
    if t.starts_with("--") || !t.starts_with('-') || t.len() < 2 {
        return false;
    }
    t.starts_with("-i") || (t[1..].chars().all(|c| c.is_ascii_alphabetic()) && t.contains('i'))
}

/// Commands whose path arguments are read or written (spec §15.4).
const FILE_CMDS: &[&str] = &[
    "cat", "less", "more", "head", "tail", "cp", "mv", "rm", "ln", "tee", "truncate", "touch",
    "base64", "xxd", "od", "strings", "hexdump", "file", "source", ".", "chmod", "chown", "zip",
    "unzip", "scp", "rsync", "install",
];

/// Options of `cmd` that take a value (the value is not a path).
fn value_options(cmd: &str) -> &'static [&'static str] {
    match cmd {
        "head" | "tail" => &["-n", "-c"],
        "truncate" => &["-s"],
        "od" => &["-N", "-j", "-t", "-A"],
        "xxd" => &["-l", "-s", "-c", "-g"],
        "strings" => &["-n"],
        "hexdump" => &["-n", "-s"],
        "install" => &["-m", "-o", "-g"],
        "rsync" => &["-e"],
        "scp" => &["-P", "-o", "-F"],
        "file" => &["-m"],
        _ => &[],
    }
}

/// Indices (into `inner`) of the words that are path arguments of a file-consuming
/// command, or the script argument of a shell or interpreter.
fn file_arg_indices(
    argv0: &str,
    inner: &[Word],
    sed_in_place: bool,
    script_idx: Option<usize>,
) -> Vec<usize> {
    let mut out = Vec::new();
    match argv0 {
        "dd" => {
            for (i, w) in inner.iter().enumerate().skip(1) {
                if w.text.starts_with("if=") || w.text.starts_with("of=") {
                    out.push(i);
                }
            }
        }
        "tar" => {
            for (i, w) in inner.iter().enumerate().skip(1) {
                let t = w.text.as_str();
                let prev = inner.get(i - 1).map_or("", |p| p.text.as_str());
                let after_flag = matches!(prev, "-f" | "-C" | "--file" | "--directory")
                    || (prev.starts_with('-')
                        && !prev.starts_with("--")
                        && (prev.ends_with('f') || prev.ends_with('C')));
                let attached = t.starts_with("--file=")
                    || t.starts_with("--directory=")
                    || (t.starts_with("-f") && t.len() > 2)
                    || (t.starts_with("-C") && t.len() > 2);
                if (after_flag && i > 1) || attached {
                    out.push(i);
                }
            }
        }
        "sed" if sed_in_place => {
            let mut script_seen = inner
                .iter()
                .skip(1)
                .any(|w| w.text == "-e" || w.text == "-f" || w.text.starts_with("--expression"));
            for (i, w) in inner.iter().enumerate().skip(1) {
                if w.text.starts_with('-') {
                    continue;
                }
                if !script_seen {
                    script_seen = true;
                    continue;
                }
                out.push(i);
            }
        }
        a if FILE_CMDS.contains(&a) => {
            let values = value_options(a);
            let mut skip_next = false;
            let mut first = true;
            for (i, w) in inner.iter().enumerate().skip(1) {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                let t = w.text.as_str();
                if t.starts_with('-') && t.len() > 1 {
                    skip_next = values.contains(&t);
                    continue;
                }
                if first && matches!(a, "chmod" | "chown") {
                    first = false;
                    continue;
                }
                first = false;
                out.push(i);
            }
        }
        _ => {
            if let Some(i) = script_idx {
                out.push(i);
            }
        }
    }
    out
}

/// Where a shell or interpreter reads its program from.
#[derive(Clone, Copy, PartialEq)]
enum Program {
    /// Standard input (no script, `-`, `/dev/stdin`, `/dev/fd/N`).
    Stdin,
    /// A process substitution `<(…)`.
    Procsub,
    /// A script file or inline code.
    Other,
}

fn is_stdin_path(t: &str) -> bool {
    t == "-"
        || t == "/dev/stdin"
        || t.starts_with("/dev/fd/")
        || t.starts_with("/proc/self/fd/")
        || (t.starts_with("/proc/") && t.contains("/fd/"))
}

fn program_of_word(w: &Word) -> Program {
    if w.procsub {
        Program::Procsub
    } else if is_stdin_path(&w.text) {
        Program::Stdin
    } else {
        Program::Other
    }
}

/// The program named by the first non-option argument; none means stdin.
fn first_positional(args: &[Word]) -> Program {
    match args.iter().find(|w| w.procsub || !w.text.starts_with('-') || w.text == "-") {
        Some(w) => program_of_word(w),
        None => Program::Stdin,
    }
}

/// Option tables of the interpreters whose program can come from a stream.
struct InterpSpec {
    /// Short flags that mean inline code or a module (the program is not stdin).
    code: &'static str,
    /// Short flags that take a value (attached or the next argument).
    valued: &'static str,
    /// Long flags that mean inline code.
    long_code: &'static [&'static str],
    /// Long flags that take the next argument.
    long_valued: &'static [&'static str],
    /// With no program argument the interpreter reads stdin.
    stdin_default: bool,
}

/// The program an interpreter runs and, when it is a script argument, that
/// argument's index in `args`. `kind` is the lowercase name without a version suffix.
fn interpreter_program(kind: &str, args: &[Word]) -> Option<(Program, Option<usize>)> {
    let spec = match kind {
        "python" => {
            InterpSpec { code: "cm", valued: "WXQ", long_code: &[], long_valued: &[], stdin_default: true }
        }
        "perl" => InterpSpec { code: "eE", valued: "IMm", long_code: &[], long_valued: &[], stdin_default: true },
        "ruby" => InterpSpec { code: "e", valued: "IrCE", long_code: &[], long_valued: &[], stdin_default: true },
        "node" => InterpSpec {
            code: "ep",
            valued: "r",
            long_code: &["--eval", "--print"],
            long_valued: &["--require"],
            stdin_default: true,
        },
        "php" => InterpSpec { code: "rf", valued: "dcz", long_code: &[], long_valued: &[], stdin_default: true },
        "lua" | "luajit" => InterpSpec { code: "e", valued: "l", long_code: &[], long_valued: &[], stdin_default: true },
        "tclsh" => InterpSpec { code: "", valued: "", long_code: &[], long_valued: &[], stdin_default: true },
        "osascript" => InterpSpec { code: "e", valued: "l", long_code: &[], long_valued: &[], stdin_default: true },
        "rscript" => InterpSpec { code: "e", valued: "", long_code: &[], long_valued: &[], stdin_default: false },
        "pwsh" | "powershell" => return Some((powershell_program(args), None)),
        "deno" | "bun" => return Some((runner_program(args), None)),
        "awk" | "gawk" | "mawk" => return Some((awk_program(args), None)),
        _ => return None,
    };
    let mut i = 0;
    while let Some(w) = args.get(i) {
        let t = w.text.as_str();
        if w.procsub {
            return Some((Program::Procsub, None));
        }
        if t == "-" || (!t.starts_with('-') && is_stdin_path(t)) {
            return Some((Program::Stdin, None));
        }
        if !t.starts_with('-') {
            return Some((Program::Other, Some(i)));
        }
        if t.starts_with("--") {
            let name = t.split('=').next().unwrap_or(t);
            if spec.long_code.contains(&name) {
                return Some((Program::Other, None));
            }
            i += if spec.long_valued.contains(&t) { 2 } else { 1 };
            continue;
        }
        let flags = &t[1..];
        let mut consumed_next = false;
        for (k, c) in flags.char_indices() {
            if spec.code.contains(c) {
                return Some((Program::Other, None));
            }
            if spec.valued.contains(c) {
                consumed_next = k + c.len_utf8() == flags.len();
                break;
            }
        }
        i += if consumed_next { 2 } else { 1 };
    }
    Some((
        if spec.stdin_default {
            Program::Stdin
        } else {
            Program::Other
        },
        None,
    ))
}

fn powershell_program(args: &[Word]) -> Program {
    for (i, w) in args.iter().enumerate() {
        let t = w.text.to_ascii_lowercase();
        if matches!(t.as_str(), "-c" | "-command") && args.get(i + 1).is_some_and(|n| n.text == "-")
        {
            return Program::Stdin;
        }
        if w.procsub {
            return Program::Procsub;
        }
        if t == "-" {
            return Program::Stdin;
        }
        if matches!(
            t.as_str(),
            "-c" | "-command" | "-f" | "-file" | "-e" | "-ec" | "-encodedcommand"
        ) {
            return Program::Other;
        }
        if !t.starts_with('-') {
            return program_of_word(w);
        }
    }
    Program::Stdin
}

/// `deno run -`, `bun run -`: only the `run` subcommand reads a script argument.
fn runner_program(args: &[Word]) -> Program {
    if args.first().map(|w| w.text.as_str()) != Some("run") {
        return Program::Other;
    }
    for w in &args[1..] {
        if w.procsub || w.text == "-" || !w.text.starts_with('-') {
            return program_of_word(w);
        }
    }
    Program::Other
}

/// `awk -f FILE`: stdin only when FILE is a stdin path.
fn awk_program(args: &[Word]) -> Program {
    for (i, w) in args.iter().enumerate() {
        if w.text == "-f" {
            return args.get(i + 1).map_or(Program::Other, program_of_word);
        }
    }
    Program::Other
}

/// True for `-c`, `-lc`, `-ec` and similar option bundles.
fn is_c_bundle(t: &str) -> bool {
    t.starts_with('-')
        && !t.starts_with("--")
        && t.len() > 1
        && t[1..].chars().all(|c| c.is_ascii_alphabetic())
        && t.contains('c')
}

fn has_unquoted_glob(chars: &[Lit]) -> bool {
    chars
        .iter()
        .any(|(c, l)| !l && matches!(c, '*' | '?' | '['))
}

/// Maximum nesting of brace groups that is expanded; deeper groups stay literal.
const MAX_BRACE_NESTING: usize = 8;
/// Maximum glob-bearing words expanded against the filesystem per parse.
const MAX_GLOB_WORDS: usize = 64;
/// Maximum nested wrapper commands (`sudo env nice …`).
const MAX_WRAPPERS: usize = 8;

struct BraceInfo {
    /// For each unquoted `{`: index of its matching `}`.
    close: Vec<Option<usize>>,
    /// For each unquoted `{`: indices of its top-level commas.
    commas: Vec<Vec<usize>>,
}

fn brace_info(word: &[Lit]) -> BraceInfo {
    let mut info = BraceInfo {
        close: vec![None; word.len()],
        commas: vec![Vec::new(); word.len()],
    };
    let mut stack: Vec<usize> = Vec::new();
    for (i, (c, lit)) in word.iter().enumerate() {
        if *lit {
            continue;
        }
        match c {
            '{' => stack.push(i),
            '}' => {
                if let Some(open) = stack.pop() {
                    info.close[open] = Some(i);
                }
            }
            ',' => {
                if let Some(open) = stack.last() {
                    info.commas[*open].push(i);
                }
            }
            _ => {}
        }
    }
    info
}

/// Combine the partial results `acc` with alternatives `alts`, capped at `cap`.
fn combine(
    acc: Vec<Vec<Lit>>,
    mut alts: Vec<Vec<Lit>>,
    cap: usize,
    trunc: &mut bool,
) -> Vec<Vec<Lit>> {
    if acc.is_empty() || alts.is_empty() {
        return acc;
    }
    let allowed = (cap / acc.len()).max(1);
    if alts.len() > allowed {
        *trunc = true;
    }
    alts.truncate(allowed);
    if alts.len() == 1 {
        let mut acc = acc;
        for r in &mut acc {
            r.extend_from_slice(&alts[0]);
        }
        return acc;
    }
    let mut out = Vec::with_capacity(acc.len() * alts.len());
    for r in &acc {
        for a in &alts {
            let mut w = r.clone();
            w.extend_from_slice(a);
            out.push(w);
        }
    }
    out
}

fn expand_range(
    word: &[Lit],
    info: &BraceInfo,
    lo: usize,
    hi: usize,
    nesting: usize,
    cap: usize,
    trunc: &mut bool,
) -> Vec<Vec<Lit>> {
    let mut acc: Vec<Vec<Lit>> = vec![Vec::new()];
    let mut i = lo;
    while i < hi {
        let group = match (word[i], info.close[i]) {
            (('{', false), Some(close))
                if close < hi && !info.commas[i].is_empty() && nesting < MAX_BRACE_NESTING =>
            {
                Some(close)
            }
            _ => None,
        };
        if let Some(close) = group {
            let mut bounds = vec![i];
            bounds.extend(info.commas[i].iter().copied());
            bounds.push(close);
            let mut alts: Vec<Vec<Lit>> = Vec::new();
            for pair in bounds.windows(2) {
                if alts.len() >= cap {
                    *trunc = true;
                    break;
                }
                alts.extend(expand_range(
                    word,
                    info,
                    pair[0] + 1,
                    pair[1],
                    nesting + 1,
                    cap,
                    trunc,
                ));
            }
            if alts.len() > cap {
                alts.truncate(cap);
                *trunc = true;
            }
            acc = combine(acc, alts, cap, trunc);
            i = close + 1;
        } else {
            for r in &mut acc {
                r.push(word[i]);
            }
            i += 1;
        }
    }
    acc
}

/// Brace expansion over unquoted `{a,b}` groups, capped at `cap` words.
fn expand_braces(word: &[Lit], cap: usize, out: &mut Vec<Vec<Lit>>) -> bool {
    let info = brace_info(word);
    let mut trunc = false;
    let all = expand_range(word, &info, 0, word.len(), 0, cap, &mut trunc);
    if all.len() > cap {
        trunc = true;
    }
    out.extend(all.into_iter().take(cap));
    trunc
}

/// Compile one glob path segment; `None` means treat it as a literal.
fn segment_matcher(seg: &[Lit]) -> Option<globset::GlobMatcher> {
    let mut pat = String::new();
    for (c, lit) in seg {
        let meta_always = matches!(c, '{' | '}' | '\\');
        let meta_glob = matches!(c, '*' | '?' | '[' | ']');
        if meta_always || (*lit && meta_glob) {
            pat.push('\\');
        }
        pat.push(*c);
    }
    globset::GlobBuilder::new(&pat)
        .backslash_escape(true)
        .build()
        .ok()
        .map(|g| g.compile_matcher())
}

/// Options that widen globbing or change how directories resolve.
const WIDENING_GLOB_OPTIONS: &[&str] = &[
    "dotglob",
    "extglob",
    "globstar",
    "nocaseglob",
    "nullglob",
    "failglob",
    "globdots",
    "extendedglob",
    "globstarshort",
    "kshglob",
];

/// The string argument of `env -S` / `--split-string`, if present.
fn env_split_string(words: &[Word]) -> Option<String> {
    for (i, w) in words.iter().enumerate().skip(1) {
        let t = w.text.as_str();
        if t == "-S" || t == "--split-string" {
            return words.get(i + 1).map(|n| n.text.clone());
        }
        if let Some(rest) = t.strip_prefix("--split-string=") {
            return Some(rest.to_string());
        }
        if !t.starts_with("--")
            && let Some(rest) = t.strip_prefix("-S")
            && !rest.is_empty()
        {
            return Some(rest.to_string());
        }
    }
    None
}

impl<'a> Parser<'a> {
    fn new(src: &str, sctx: &'a SubjectContext, bases: Vec<PathBuf>) -> Self {
        Parser {
            chars: src.chars().collect(),
            pos: 0,
            sctx,
            commands: Vec::new(),
            paths: Vec::new(),
            alt_paths: Vec::new(),
            alt_buf: std::cell::RefCell::new(Vec::new()),
            unparsed: false,
            pipe_to_interpreter: false,
            contexts: Vec::new(),
            var_path: false,
            here_subst: false,
            stdin_text: None,
            heredoc_scan: None,
            inherit_piped: false,
            no_remember: false,
            exports: Vec::new(),
            auto_export: false,
            subshell: 0,
            loops: Vec::new(),
            pending_for: None,
            cwd_unknown: false,
            line_has_function: has_function_def(src),
            brace_depth: 0,
            func_brace: Vec::new(),
            func_pending: false,
            in_backtick: false,
            arith: false,
            bases,
            depth: 0,
            glob_words: std::cell::Cell::new(0),
            glob_overflow: std::cell::Cell::new(false),
            heredocs: Vec::new(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, off: usize) -> Option<char> {
        self.chars.get(self.pos + off).copied()
    }

    fn skip_blanks(&mut self) {
        while self.peek().is_some_and(is_blank) {
            self.pos += 1;
        }
    }

    fn is_word_end(&self, c: char) -> bool {
        is_blank(c)
            || matches!(c, '\n' | ';' | '&' | '|' | '(' | ')')
            || (!self.arith && matches!(c, '<' | '>'))
    }

    fn check_caps(&mut self) -> Res<()> {
        if self.commands.len() >= MAX_COMMANDS || self.paths.len() >= MAX_PATHS {
            return Err(Fail);
        }
        Ok(())
    }

    fn push_path(&mut self, pos: usize, op: PathOp, path: PathBuf) {
        if self.paths.len() >= MAX_PATHS {
            self.unparsed = true;
            return;
        }
        self.paths.push((pos, op, path));
    }

    /// Parse a command list until `term`.
    fn parse_list(&mut self, term: Term, depth: usize) -> Res<()> {
        let saved_depth = std::mem::replace(&mut self.depth, depth);
        let mut cmd = Cmd {
            piped: std::mem::take(&mut self.inherit_piped),
            ..Cmd::default()
        };
        let r = self.parse_list_inner(&mut cmd, term, depth);
        if r.is_err() {
            self.end_cmd(&mut cmd);
        }
        self.depth = saved_depth;
        r
    }

    fn parse_list_inner(&mut self, cmd: &mut Cmd, term: Term, depth: usize) -> Res<()> {
        let mut parens = 0usize;
        // Directory state to restore when a `( … )` subshell ends.
        let mut saved_dirs: Vec<(Vec<PathBuf>, bool)> = Vec::new();
        loop {
            self.check_caps()?;
            self.skip_blanks();
            let Some(c) = self.peek() else {
                self.end_cmd(cmd);
                if term != Term::Eof || !self.heredocs.is_empty() {
                    return Err(Fail);
                }
                return Ok(());
            };
            match c {
                '\n' => {
                    self.pos += 1;
                    self.end_cmd(cmd);
                    self.skip_heredoc_bodies()?;
                }
                '|' => {
                    // `||` is a list operator; `|` and `|&` pipe into the next command.
                    let list_op = self.peek_at(1) == Some('|');
                    self.pos += if list_op || self.peek_at(1) == Some('&') {
                        2
                    } else {
                        1
                    };
                    self.end_cmd(cmd);
                    cmd.piped = !list_op;
                }
                ';' => {
                    self.pos += 1;
                    self.end_cmd(cmd);
                }
                '&' if !self.arith && self.peek_at(1) == Some('>') => self.redirect(depth)?,
                '&' => {
                    self.pos += 1;
                    self.end_cmd(cmd);
                }
                '(' => {
                    if cmd.words.is_empty() && self.peek_at(1) == Some('(') {
                        // `((expr))` arithmetic command.
                        let from = self.pos + 2;
                        self.pos += 2;
                        self.substitute(Term::Arith, depth)?;
                        // `((f=1))` and friends reassign variables.
                        let text: String = self
                            .chars
                            .get(from..self.pos.saturating_sub(2).max(from))
                            .unwrap_or(&[])
                            .iter()
                            .collect();
                        for name in text
                            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                            .filter(|n| !n.is_empty())
                        {
                            self.invalidate(name);
                        }
                    } else if self.function_def_ahead(cmd) {
                        cmd.words.clear();
                    } else {
                        self.pos += 1;
                        parens += 1;
                        self.subshell += 1;
                        self.end_cmd(cmd);
                        saved_dirs.push((self.bases.clone(), self.cwd_unknown));
                    }
                }
                ')' => {
                    if term == Term::Arith && parens == 0 {
                        self.pos += 1;
                        if self.peek() == Some(')') {
                            self.pos += 1;
                            self.end_cmd(cmd);
                            return Ok(());
                        }
                        continue;
                    }
                    self.pos += 1;
                    self.end_cmd(cmd);
                    if parens > 0 {
                        parens -= 1;
                        self.subshell = self.subshell.saturating_sub(1);
                        // A `cd` in a subshell never changes the outer directory state.
                        if let Some((bases, unknown)) = saved_dirs.pop() {
                            self.bases = bases;
                            self.cwd_unknown = unknown;
                        }
                    } else if term == Term::Paren {
                        return Ok(());
                    }
                }
                '`' if term == Term::Backtick => {
                    self.pos += 1;
                    self.end_cmd(cmd);
                    return Ok(());
                }
                '#' => {
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.pos += 1;
                    }
                }
                '<' if !self.arith && self.peek_at(1) == Some('(') => {
                    // Process substitution `<(…)`: its commands are parsed, and the
                    // result is an argument of the current command.
                    let at = self.pos;
                    self.pos += 2;
                    self.substitute(Term::Paren, depth)?;
                    let mut w = Word::default();
                    for ch in "<(…)".chars() {
                        w.push(ch, true);
                    }
                    w.var = true;
                    w.procsub = true;
                    w.quoted = true;
                    w.start = at;
                    cmd.words.push(w);
                }
                '>' | '<' if !self.arith => self.redirect(depth)?,
                c if !self.arith && c.is_ascii_digit() && self.fd_redirect_ahead() => {
                    self.redirect(depth)?
                }
                _ => {
                    let w = self.read_word(depth)?;
                    if w.started {
                        // Loop bodies are scoped at token time, so redirections on the
                        // same command already see the binding.
                        let fn_header = cmd.words.len() == 2
                            && cmd.words[0].text == "function"
                            && !cmd.words[0].quoted;
                        if (cmd.words.is_empty() || fn_header) && !w.quoted && w.text == "{" {
                            self.brace_depth += 1;
                            if self.func_pending || fn_header {
                                self.func_brace.push(self.brace_depth);
                                self.func_pending = false;
                            }
                        }
                        if cmd.words.is_empty() && !w.quoted && w.text == "}" {
                            if self.func_brace.last() == Some(&self.brace_depth) {
                                self.func_brace.pop();
                            }
                            self.brace_depth = self.brace_depth.saturating_sub(1);
                        }
                        if cmd.words.is_empty() && !w.quoted && !w.var {
                            match w.text.as_str() {
                                "do" => {
                                    let mut b = self.pending_for.take();
                                    if let Some(nb) = b.as_mut() {
                                        nb.fn_level = self.func_brace.len();
                                        // bash keeps the inner value after the inner loop:
                                        // the outer binding is not restored.
                                        let name = nb.name.clone();
                                        for old in self.loops.iter_mut().flatten() {
                                            if old.name == name {
                                                old.valid = false;
                                            }
                                        }
                                    }
                                    self.loops.push(b);
                                }
                                "done" => {
                                    self.loops.pop();
                                }
                                _ => {}
                            }
                        }
                        cmd.words.push(w);
                    }
                }
            }
        }
    }

    /// At `(`: is this `NAME()` or `function NAME()`? If so, consume `()`.
    fn function_def_ahead(&mut self, cmd: &Cmd) -> bool {
        // Leading reserved words (`do`, `then`, `{`, …) belong to the enclosing construct.
        let lead = cmd
            .words
            .iter()
            .take_while(|w| !w.quoted && !w.var && RESERVED.contains(&w.text.as_str()))
            .count();
        let named = match &cmd.words[lead..] {
            [w] => !w.quoted && !w.var,
            [f, n] => f.text == "function" && !f.quoted && !n.var,
            _ => false,
        };
        if !named {
            return false;
        }
        let mut i = self.pos + 1;
        while self.chars.get(i).copied().is_some_and(is_blank) {
            i += 1;
        }
        if self.chars.get(i) == Some(&')') {
            self.pos = i + 1;
            self.func_pending = true;
            true
        } else {
            false
        }
    }

    /// True if the digits at `pos` are a file descriptor before `<` or `>`.
    fn fd_redirect_ahead(&self) -> bool {
        let mut i = self.pos;
        while self.chars.get(i).is_some_and(char::is_ascii_digit) {
            i += 1;
        }
        matches!(self.chars.get(i), Some('<' | '>'))
    }

    fn redirect(&mut self, depth: usize) -> Res<()> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        let mut op = PathOp::Write;
        let mut dup = false;
        match self.peek() {
            Some('&') => {
                // `&>` or `&>>`
                self.pos += 2;
                if self.peek() == Some('>') {
                    self.pos += 1;
                }
            }
            Some('>') => {
                self.pos += 1;
                match self.peek() {
                    Some('>' | '|') => self.pos += 1,
                    Some('&') => {
                        self.pos += 1;
                        dup = true;
                    }
                    _ => {}
                }
            }
            _ => {
                // '<'
                self.pos += 1;
                match self.peek() {
                    Some('<') => {
                        self.pos += 1;
                        return self.heredoc_or_herestring(depth);
                    }
                    Some('&') => {
                        self.pos += 1;
                        dup = true;
                        op = PathOp::Read;
                    }
                    Some('>') => self.pos += 1,
                    _ => op = PathOp::Read,
                }
            }
        }
        self.skip_blanks();
        let w = self.read_word(depth)?;
        let is_fd = dup
            && (w.text == "-"
                || (!w.text.is_empty() && w.text.bytes().all(|b| b.is_ascii_digit())));
        if w.started && !is_fd {
            if w.var && !w.procsub {
                match self.substitute_word(&w) {
                    Subst::Words(ws) => {
                        for nw in ws {
                            for (p, prefix) in self.word_paths(&nw) {
                                self.push_path(start, if prefix { PathOp::Read } else { op }, p);
                            }
                            self.flush_alts(start, &[op]);
                        }
                        return Ok(());
                    }
                    _ => self.var_path = true,
                }
            } else if self.cwd_unknown && Self::is_relative_word(&w) {
                self.var_path = true;
            }
            for (p, prefix) in self.word_paths(&w) {
                self.push_path(start, if prefix { PathOp::Read } else { op }, p);
            }
            self.flush_alts(start, &[op]);
        }
        Ok(())
    }

    fn heredoc_or_herestring(&mut self, depth: usize) -> Res<()> {
        if self.peek() == Some('<') {
            // here-string: the word is data, and a program when fed to a shell.
            self.pos += 1;
            self.skip_blanks();
            let w = self.read_word(depth)?;
            self.stdin_text = Some(StdinText {
                text: w.text.clone(),
                denied: w.var,
            });
            return Ok(());
        }
        let strip = self.peek() == Some('-');
        if strip {
            self.pos += 1;
        }
        self.skip_blanks();
        let w = self.read_word(depth)?;
        if !w.started || w.text.is_empty() {
            return Err(Fail);
        }
        let delim: Vec<char> = w.text.chars().collect();
        let (body, next) = self.heredoc_body(&delim, strip)?;
        self.heredoc_scan = Some(next);
        // `$name`, `${…}`, `$(…)` and backticks: the outer shell splices the value into
        // the script text before the inner shell parses it.
        let substituting = body.contains('`')
            || body.char_indices().any(|(i, c)| {
                c == '$'
                    && body[i + 1..].chars().next().is_some_and(|n| {
                        n.is_ascii_alphanumeric() || matches!(n, '_' | '{' | '(' | '@' | '*' | '#' | '?' | '!' | '$' | '-')
                    })
            });
        self.stdin_text = Some(StdinText {
            text: body,
            denied: !w.quoted && substituting,
        });
        self.heredocs.push((delim, strip));
        Ok(())
    }

    /// The values a `for` list binds: literal words (with `~`/`$HOME` resolved) and the
    /// matches of literal globs.
    fn for_values(&self, list: &[Word]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for w in list {
            if w.glob {
                let found: Vec<(PathBuf, bool)> = self.word_paths(w);
                let alts = std::mem::take(&mut *self.alt_buf.borrow_mut());
                for (p, prefix) in found {
                    if !prefix {
                        out.push(p.to_string_lossy().into_owned());
                    }
                }
                for p in alts {
                    out.push(p.to_string_lossy().into_owned());
                }
                continue;
            }
            let home = self.sctx.home.as_ref().map(|h| h.to_string_lossy().into_owned());
            let text = if w.home_chars > 0 {
                match &home {
                    Some(h) => format!("{h}{}", &w.text[w.home_chars..]),
                    None => w.text.clone(),
                }
            } else if w.tilde && (w.text == "~" || w.text.starts_with("~/")) {
                match &home {
                    Some(h) => format!("{h}{}", &w.text[1..]),
                    None => w.text.clone(),
                }
            } else {
                w.text.clone()
            };
            out.push(text);
        }
        out.dedup();
        out
    }

    /// Look ahead for the body of a heredoc whose delimiter was just read: the text
    /// from the next line (or after the previous heredoc on this line) up to the
    /// delimiter line, and the offset after that line.
    fn heredoc_body(&self, delim: &[char], strip: bool) -> Res<(String, usize)> {
        let mut p = match self.heredoc_scan {
            Some(p) => p,
            None => {
                let mut i = self.pos;
                while self.chars.get(i).is_some_and(|c| *c != '\n') {
                    i += 1;
                }
                i + 1
            }
        };
        let start = p;
        loop {
            if p >= self.chars.len() {
                return Err(Fail);
            }
            let mut eol = p;
            while self.chars.get(eol).is_some_and(|c| *c != '\n') {
                eol += 1;
            }
            let mut line = &self.chars[p..eol];
            if strip {
                while let ['\t', rest @ ..] = line {
                    line = rest;
                }
            }
            if line == delim {
                let body: String = self.chars[start..p].iter().collect();
                return Ok((body, (eol + 1).min(self.chars.len())));
            }
            p = eol + 1;
        }
    }

    /// Skip the bodies of pending heredocs, starting at the current position
    /// (just after a newline).
    fn skip_heredoc_bodies(&mut self) -> Res<()> {
        let pending = std::mem::take(&mut self.heredocs);
        for (delim, strip) in pending {
            loop {
                if self.pos >= self.chars.len() {
                    return Err(Fail);
                }
                let mut eol = self.pos;
                while self.chars.get(eol).is_some_and(|c| *c != '\n') {
                    eol += 1;
                }
                let mut line = &self.chars[self.pos..eol];
                if strip {
                    while let ['\t', rest @ ..] = line {
                        line = rest;
                    }
                }
                let done = line == delim.as_slice();
                self.pos = (eol + 1).min(self.chars.len());
                if done {
                    break;
                }
            }
        }
        self.heredoc_scan = None;
        Ok(())
    }

    /// Read one shell word starting at `pos`.
    fn read_word(&mut self, depth: usize) -> Res<Word> {
        let mut w = Word {
            start: self.pos,
            ..Word::default()
        };
        while let Some(c) = self.peek() {
            if self.is_word_end(c) || (c == '`' && self.in_backtick) {
                break;
            }
            match c {
                '\\' => {
                    self.pos += 1;
                    w.quoted = true;
                    match self.peek() {
                        Some('\n') => self.pos += 1,
                        Some(n) => {
                            w.push(n, true);
                            self.pos += 1;
                        }
                        None => {}
                    }
                }
                '\'' => {
                    self.pos += 1;
                    w.started = true;
                    w.quoted = true;
                    loop {
                        match self.peek() {
                            None => return Err(Fail),
                            Some('\'') => {
                                self.pos += 1;
                                break;
                            }
                            Some(ch) => {
                                w.push(ch, true);
                                self.pos += 1;
                            }
                        }
                    }
                }
                '"' => {
                    self.pos += 1;
                    w.started = true;
                    w.quoted = true;
                    self.read_double_quoted(&mut w, depth)?;
                }
                '$' => self.read_dollar(&mut w, depth, false)?,
                '`' => {
                    let start = self.pos;
                    self.pos += 1;
                    self.substitute(Term::Backtick, depth)?;
                    self.push_source(&mut w, start);
                }
                _ => {
                    if c == '~' && !w.started {
                        w.tilde = true;
                    }
                    w.push(c, false);
                    self.pos += 1;
                }
            }
        }
        Ok(w)
    }

    fn push_source(&mut self, w: &mut Word, start: usize) {
        for i in start..self.pos {
            w.push(self.chars[i], true);
        }
        w.started = true;
        w.var = true;
    }

    fn substitute(&mut self, term: Term, depth: usize) -> Res<()> {
        if depth + 1 > MAX_DEPTH {
            return Err(Fail);
        }
        let saved_bt = self.in_backtick;
        let saved_arith = self.arith;
        self.in_backtick = term == Term::Backtick;
        self.arith = term == Term::Arith;
        // A `cd` inside `$( … )`, backticks or `<( … )` runs in a subshell.
        let saved_dirs = (self.bases.clone(), self.cwd_unknown);
        let r = self.parse_list(term, depth + 1);
        self.bases = saved_dirs.0;
        self.cwd_unknown = saved_dirs.1;
        self.in_backtick = saved_bt;
        self.arith = saved_arith;
        r
    }

    fn read_double_quoted(&mut self, w: &mut Word, depth: usize) -> Res<()> {
        loop {
            let Some(c) = self.peek() else {
                return Err(Fail);
            };
            match c {
                '"' => {
                    self.pos += 1;
                    return Ok(());
                }
                '\\' => {
                    self.pos += 1;
                    match self.peek() {
                        None => return Err(Fail),
                        Some('\n') => self.pos += 1,
                        Some(n @ ('"' | '\\' | '$' | '`')) => {
                            w.push(n, true);
                            self.pos += 1;
                        }
                        Some(_) => w.push('\\', true),
                    }
                }
                '$' => self.read_dollar(w, depth, true)?,
                '`' => {
                    if self.in_backtick {
                        return Err(Fail);
                    }
                    let start = self.pos;
                    self.pos += 1;
                    self.substitute(Term::Backtick, depth)?;
                    self.push_source(w, start);
                }
                _ => {
                    w.push(c, true);
                    self.pos += 1;
                }
            }
        }
    }

    /// Handle a `$` at `pos` (outside single quotes).
    fn read_dollar(&mut self, w: &mut Word, depth: usize, in_dq: bool) -> Res<()> {
        let start = self.pos;
        let next = self.peek_at(1);
        match next {
            Some('(') if self.peek_at(2) == Some('(') => {
                self.pos += 3;
                self.substitute(Term::Arith, depth)?;
                self.push_source(w, start);
            }
            Some('(') => {
                self.pos += 2;
                self.substitute(Term::Paren, depth)?;
                self.push_source(w, start);
            }
            Some('\'') if !in_dq => {
                self.pos += 2;
                w.started = true;
                w.quoted = true;
                self.read_ansi_c(w)?;
            }
            Some('{') => {
                // `${HOME}` or another parameter expansion.
                let mut i = self.pos + 2;
                while self.chars.get(i).is_some_and(|c| *c != '}') {
                    i += 1;
                }
                let end = (i + 1).min(self.chars.len());
                let name: String = self.chars[self.pos + 2..i.min(self.chars.len())]
                    .iter()
                    .collect();
                self.expand_var(w, start, end, &name);
            }
            Some(c) if is_name_start(c) => {
                let mut i = self.pos + 1;
                while self
                    .chars
                    .get(i)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
                {
                    i += 1;
                }
                let name: String = self.chars[self.pos + 1..i].iter().collect();
                self.expand_var(w, start, i, &name);
            }
            Some(c)
                if c.is_ascii_digit() || matches!(c, '@' | '*' | '#' | '?' | '$' | '!' | '-') =>
            {
                self.expand_var(w, start, self.pos + 2, "");
            }
            _ => {
                w.push('$', true);
                self.pos += 1;
            }
        }
        Ok(())
    }

    fn expand_var(&mut self, w: &mut Word, start: usize, end: usize, name: &str) {
        let at_text_start = w.text.is_empty();
        for i in start..end {
            w.push(self.chars[i], true);
        }
        w.started = true;
        self.pos = end;
        if name == "HOME" && at_text_start {
            w.home_chars = end - start;
        } else {
            w.var = true;
        }
    }

    fn read_ansi_c(&mut self, w: &mut Word) -> Res<()> {
        loop {
            let Some(c) = self.peek() else {
                return Err(Fail);
            };
            self.pos += 1;
            match c {
                '\'' => return Ok(()),
                '\\' => {
                    let Some(e) = self.peek() else {
                        return Err(Fail);
                    };
                    self.pos += 1;
                    match e {
                        'a' => w.push('\u{7}', true),
                        'b' => w.push('\u{8}', true),
                        'e' | 'E' => w.push('\u{1b}', true),
                        'f' => w.push('\u{c}', true),
                        'n' => w.push('\n', true),
                        'r' => w.push('\r', true),
                        't' => w.push('\t', true),
                        'v' => w.push('\u{b}', true),
                        '\\' | '\'' | '"' | '?' => w.push(e, true),
                        'x' => self.numeric_escape(w, 16, 2, e),
                        'u' => self.numeric_escape(w, 16, 4, e),
                        'U' => self.numeric_escape(w, 16, 8, e),
                        '0'..='7' => {
                            self.pos -= 1;
                            self.numeric_escape(w, 8, 3, e);
                        }
                        _ => {
                            w.push('\\', true);
                            w.push(e, true);
                        }
                    }
                }
                _ => w.push(c, true),
            }
        }
    }

    fn numeric_escape(&mut self, w: &mut Word, radix: u32, max: usize, marker: char) {
        let mut val = 0u32;
        let mut n = 0;
        while n < max {
            match self.peek().and_then(|c| c.to_digit(radix)) {
                Some(d) => {
                    val = val.saturating_mul(radix).saturating_add(d);
                    self.pos += 1;
                    n += 1;
                }
                None => break,
            }
        }
        if n == 0 {
            w.push('\\', true);
            w.push(marker, true);
        } else {
            w.push(char::from_u32(val).unwrap_or('\u{fffd}'), true);
        }
    }

    // -- path subjects ----------------------------------------------------------

    /// Resolve `raw` against `base` (relative paths only; `~` and absolute paths
    /// ignore the base).
    fn resolve_in(&self, base: &Path, raw: &str) -> PathBuf {
        if raw.starts_with('/') || raw == "~" || raw.starts_with("~/") || base == self.sctx.workdir
        {
            subject_path(raw, self.sctx)
        } else {
            resolve_path(&base.join(raw))
        }
    }

    /// Candidate resolutions of a path string: once if absolute, once per base
    /// otherwise.
    fn candidates(&self, raw: &str) -> Vec<PathBuf> {
        let absolute = raw.starts_with('/') || raw == "~" || raw.starts_with("~/");
        let mut out: Vec<PathBuf> = Vec::new();
        let bases: &[PathBuf] = if absolute {
            &self.bases[..1.min(self.bases.len())]
        } else {
            &self.bases
        };
        for b in bases {
            let p = self.resolve_in(b, raw);
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out
    }

    /// Like [`Self::candidates`] but folded lexically (symlinks not resolved).
    fn lexical_candidates(&self, raw: &str) -> Vec<PathBuf> {
        let absolute = raw.starts_with('/') || raw == "~" || raw.starts_with("~/");
        let bases: &[PathBuf] = if absolute {
            &self.bases[..1.min(self.bases.len())]
        } else {
            &self.bases
        };
        let mut out: Vec<PathBuf> = Vec::new();
        for b in bases {
            let p = lexical_in(b, raw, self.sctx);
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out
    }

    /// Record the as-written forms collected by [`Self::word_paths`] as alternatives
    /// of a path at `pos`.
    fn flush_alts(&mut self, pos: usize, ops: &[PathOp]) {
        let alts = std::mem::take(&mut *self.alt_buf.borrow_mut());
        for p in alts {
            for op in ops {
                if self.alt_paths.len() < MAX_PATHS {
                    self.alt_paths.push((pos, *op, p.clone()));
                }
            }
        }
    }

    /// Does `w` hold an expansion whose value is unknown? `$HOME` and `~` are resolved,
    /// and so are plain references to bound `for` variables (they are substituted).
    fn is_varish(&self, w: &Word) -> bool {
        w.var && !w.procsub && !matches!(self.substitute_word(w), Subst::Words(_))
    }

    /// A relative path word (after a `cd` to an unknown directory it is unknown).
    fn is_relative_word(w: &Word) -> bool {
        !w.text.is_empty()
            && !w.text.starts_with('/')
            && !w.text.starts_with('~')
            && w.home_chars == 0
            && !w.procsub
    }

    /// Variable-derived path check for a path position.
    fn flag_unknown_path(&mut self, w: &Word) {
        if self.is_varish(w) || (self.cwd_unknown && Self::is_relative_word(w)) {
            self.var_path = true;
        }
    }

    fn lookup(&self, name: &str) -> Option<&BoundVar> {
        let found = self.loops.iter().rev().flatten().find(|b| b.name == name)?;
        (found.valid && found.fn_level == self.func_brace.len() && !self.line_has_function)
            .then_some(found)
    }

    /// Stop trusting every binding (`eval`, `source`, namerefs).
    fn invalidate_all(&mut self) {
        for b in self.loops.iter_mut().flatten().chain(self.pending_for.iter_mut()) {
            b.valid = false;
        }
    }

    /// Stop trusting the binding of `name` (it was reassigned).
    fn invalidate(&mut self, name: &str) {
        for b in self.loops.iter_mut().flatten().chain(self.pending_for.iter_mut()) {
            if b.name == name {
                b.valid = false;
            }
        }
    }

    /// Expand plain `$name` / `${name}` references to bound `for` variables by
    /// substituting each combination of their values.
    fn substitute_word(&self, w: &Word) -> Subst {
        if !w.var || w.procsub || self.loops.is_empty() {
            return Subst::NotBound;
        }
        let chars: Vec<char> = w.text.chars().collect();
        let mut refs: Vec<(usize, usize, String)> = Vec::new();
        let mut i = w.home_chars; // a leading $HOME is resolved elsewhere
        while i < chars.len() {
            if chars[i] != '$' {
                i += 1;
                continue;
            }
            let braced = chars.get(i + 1) == Some(&'{');
            let mut j = i + 1 + usize::from(braced);
            let name_start = j;
            while chars
                .get(j)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
            {
                j += 1;
            }
            let name: String = chars[name_start..j].iter().collect();
            if name.is_empty() || self.lookup(&name).is_none() {
                return Subst::NotBound;
            }
            if braced {
                if chars.get(j) != Some(&'}') {
                    return Subst::NotBound;
                }
                j += 1;
            }
            refs.push((i, j, name));
            i = j;
        }
        if refs.is_empty() {
            return Subst::NotBound;
        }
        let mut names: Vec<&str> = refs.iter().map(|r| r.2.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        let mut combos = 1usize;
        for n in &names {
            let len = self.lookup(n).map_or(0, |b| b.values.len());
            combos = combos.saturating_mul(len);
        }
        if combos > MAX_COMBINATIONS {
            return Subst::TooMany;
        }
        let mut out = Vec::with_capacity(combos);
        for idx in 0..combos {
            // Mixed-radix choice of one value per distinct name.
            let mut rem = idx;
            let mut choice: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
            for n in &names {
                let vals = self.lookup(n).map(|b| &b.values[..]).unwrap_or(&[]);
                choice.insert(n, vals[rem % vals.len()].as_str());
                rem /= vals.len();
            }
            let mut nw = Word {
                started: true,
                glob: w.glob,
                start: w.start,
                quoted: w.quoted,
                home_chars: w.home_chars,
                tilde: w.tilde && refs.first().is_none_or(|r| r.0 > 0),
                ..Word::default()
            };
            let mut at = 0usize;
            for (rs, re, name) in &refs {
                for (k, c) in chars.iter().enumerate().take(*rs).skip(at) {
                    nw.text.push(*c);
                    nw.lit.push(w.lit.get(k).copied().unwrap_or(true));
                }
                for c in choice.get(name.as_str()).copied().unwrap_or("").chars() {
                    nw.text.push(c);
                    nw.lit.push(true);
                }
                at = *re;
            }
            for (k, c) in chars.iter().enumerate().skip(at) {
                nw.text.push(*c);
                nw.lit.push(w.lit.get(k).copied().unwrap_or(true));
            }
            out.push(nw);
        }
        Subst::Words(out)
    }

    /// Emit the read (and, for writers, write) subjects of an already expanded word.
    fn emit_word_paths(&mut self, w: &Word, writer: bool) {
        for (p, prefix) in self.word_paths(w) {
            self.push_path(w.start, PathOp::Read, p.clone());
            if writer && !prefix {
                self.push_path(w.start, PathOp::Write, p);
            }
        }
        if writer {
            self.flush_alts(w.start, &[PathOp::Read, PathOp::Write]);
        } else {
            self.flush_alts(w.start, &[PathOp::Read]);
        }
    }

    /// A shell, `source` or similar reads its program from stdin.
    fn stdin_program(&mut self, text: Option<StdinText>, piped: bool, pos: usize, merge: bool) {
        match text {
            Some(t) if !piped && !t.denied => {
                // The heredoc or here-string is the script: gate it like a `-c` string.
                self.parse_nested(&t.text, pos, false, merge);
            }
            Some(_) => {
                self.unparsed = true;
                self.pipe_to_interpreter = true;
            }
            None => {
                self.unparsed = true;
                self.pipe_to_interpreter |= piped;
            }
        }
    }

    fn handle_program(
        &mut self,
        prog: Program,
        text: Option<StdinText>,
        piped: bool,
        pos: usize,
        merge: bool,
    ) {
        match prog {
            Program::Procsub => self.pipe_to_interpreter = true,
            Program::Stdin => self.stdin_program(text, piped, pos, merge),
            Program::Other => {}
        }
    }

    /// Record the effect of a program source on `pipe_to_interpreter`.
    fn note_program(&mut self, prog: Program, fed: bool) {
        match prog {
            Program::Procsub => self.pipe_to_interpreter = true,
            Program::Stdin if fed => self.pipe_to_interpreter = true,
            _ => {}
        }
    }

    /// Paths a word names, as `(path, is_directory_prefix)`.
    fn word_paths(&self, w: &Word) -> Vec<(PathBuf, bool)> {
        let mut out = Vec::new();
        if w.var || w.text.is_empty() {
            return out;
        }
        let mut words = Vec::new();
        if expand_braces(&w.lits(), MAX_BRACE_WORDS, &mut words) {
            self.glob_overflow.set(true);
        }
        for chars in words {
            self.expand_one(w, chars, &mut out);
        }
        out
    }

    fn expand_one(&self, w: &Word, mut chars: Vec<Lit>, out: &mut Vec<(PathBuf, bool)>) {
        // Home forms.
        let home: Option<Vec<Lit>> = self
            .sctx
            .home
            .as_ref()
            .map(|h| h.to_string_lossy().chars().map(|c| (c, true)).collect());
        if w.home_chars > 0 && chars.len() >= w.home_chars {
            let mut rest: Vec<Lit> = chars.split_off(w.home_chars);
            match &home {
                Some(h) => {
                    chars = h.clone();
                    chars.append(&mut rest);
                }
                None => {
                    chars = vec![('~', false)];
                    chars.append(&mut rest);
                }
            }
        } else if w.tilde && chars.first() == Some(&('~', false)) {
            let tilde_home = chars.len() == 1 || chars.get(1).is_some_and(|c| c.0 == '/');
            if let (true, Some(h)) = (tilde_home, &home) {
                let mut rest = chars.split_off(1);
                chars = h.clone();
                chars.append(&mut rest);
            }
        } else if chars.first() == Some(&('~', true)) {
            chars.insert(0, ('/', true));
            chars.insert(0, ('.', true));
        }

        if !has_unquoted_glob(&chars) {
            let s: String = chars.iter().map(|(c, _)| *c).collect();
            for p in self.candidates(&s) {
                out.push((p, false));
            }
            // The path as written, in case a component is a symlink.
            for p in self.lexical_candidates(&s) {
                if !out.iter().any(|(q, _)| *q == p) {
                    self.alt_buf.borrow_mut().push(p);
                }
            }
            return;
        }
        if self.glob_words.get() >= MAX_GLOB_WORDS {
            self.glob_overflow.set(true);
            return;
        }
        self.glob_words.set(self.glob_words.get() + 1);
        let absolute = chars.first().is_some_and(|c| c.0 == '/');
        let bases: &[PathBuf] = if absolute {
            &self.bases[..1.min(self.bases.len())]
        } else {
            &self.bases
        };
        for b in bases {
            self.glob_expand(b, &chars, absolute, out);
        }
    }

    fn glob_expand(
        &self,
        base: &Path,
        chars: &[Lit],
        absolute: bool,
        out: &mut Vec<(PathBuf, bool)>,
    ) {
        let mut cur: Vec<PathBuf> = vec![if absolute {
            PathBuf::from("/")
        } else {
            base.to_path_buf()
        }];
        let mut literal_segments = 0usize;
        let mut seen_glob = false;
        for seg in chars.split(|(c, _)| *c == '/') {
            if seg.is_empty() {
                continue;
            }
            if has_unquoted_glob(seg) {
                if !seen_glob {
                    seen_glob = true;
                    if (absolute || literal_segments > 0)
                        && let Some(first) = cur.first()
                    {
                        out.push((resolve_path(first), true));
                    }
                }
                let matcher = segment_matcher(seg);
                let dot_ok = seg.first().is_some_and(|c| c.0 == '.');
                let mut next: Vec<PathBuf> = Vec::new();
                'dirs: for dir in &cur {
                    let Ok(rd) = std::fs::read_dir(dir) else {
                        continue;
                    };
                    for (n, entry) in rd.flatten().enumerate() {
                        if n >= MAX_DIR_ENTRIES {
                            self.glob_overflow.set(true);
                            break;
                        }
                        let name = entry.file_name();
                        let Some(name_s) = name.to_str() else {
                            continue;
                        };
                        if name_s.starts_with('.') && !dot_ok {
                            continue;
                        }
                        let hit = match &matcher {
                            Some(m) => m.is_match(name_s),
                            None => {
                                let lit: String = seg.iter().map(|(c, _)| *c).collect();
                                lit == name_s
                            }
                        };
                        if hit {
                            if next.len() >= MAX_GLOB_MATCHES {
                                self.glob_overflow.set(true);
                                break 'dirs;
                            }
                            next.push(dir.join(&name));
                        }
                    }
                }
                cur = next;
                if cur.is_empty() {
                    return;
                }
            } else {
                let lit: String = seg.iter().map(|(c, _)| *c).collect();
                for p in &mut cur {
                    p.push(&lit);
                }
                if seen_glob {
                    cur.retain(|p| p.symlink_metadata().is_ok());
                } else {
                    literal_segments += 1;
                }
            }
        }
        for p in cur {
            let resolved = resolve_path(&p);
            let written = fold_lexical(&p);
            if written != resolved {
                self.alt_buf.borrow_mut().push(written);
            }
            out.push((resolved, false));
        }
    }

    fn argv0_of(&self, w: &Word) -> String {
        let base = w
            .text
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or(&w.text);
        if self.sctx.case_insensitive {
            base.to_lowercase()
        } else {
            base.to_string()
        }
    }

    fn argv_of(&self, words: &[Word]) -> Vec<String> {
        let mut argv = Vec::with_capacity(words.len());
        argv.push(self.argv0_of(&words[0]));
        argv.extend(words[1..].iter().map(|w| w.text.clone()));
        argv
    }

    /// Parse `src` (a command string) recursively; its commands and paths are
    /// merged into this parse.
    fn parse_nested(&mut self, src: &str, pos: usize, piped: bool, merge_cwd: bool) {
        let depth = self.depth + 1;
        self.no_remember = true;
        if depth > MAX_DEPTH || src.len() > MAX_INPUT_BYTES {
            self.unparsed = true;
            return;
        }
        let mut child = Parser::new(src, self.sctx, self.bases.clone());
        child.inherit_piped = piped;
        if child.parse_list(Term::Eof, depth).is_err() {
            child.unparsed = true;
        }
        self.unparsed |= child.unparsed | child.glob_overflow.get();
        self.pipe_to_interpreter |= child.pipe_to_interpreter;
        self.var_path |= child.var_path;
        self.no_remember |= child.no_remember;
        if merge_cwd {
            // `eval` and `source` run in this shell: their `cd`s are ours.
            self.bases = child.bases;
            self.cwd_unknown |= child.cwd_unknown;
        }
        for (argv, ctx) in child.commands.into_iter().zip(child.contexts) {
            if self.commands.len() >= MAX_COMMANDS {
                self.unparsed = true;
                break;
            }
            self.commands.push(argv);
            self.contexts.push(ctx);
        }
        for (_, op, p) in child.paths {
            self.push_path(pos, op, p);
        }
        for (_, op, p) in child.alt_paths {
            self.alt_paths.push((pos, op, p));
        }
    }

    /// Finish the current simple command: record its argv and path subjects.
    fn end_cmd(&mut self, cmd: &mut Cmd) {
        if self.arith {
            cmd.words.clear();
            return;
        }
        let mut words = std::mem::take(&mut cmd.words);
        // A pipe applies to the next command that has words (it survives `a |\n b`).
        let (mut piped, stdin_text) = if words.is_empty() {
            (false, None)
        } else {
            (
                std::mem::take(&mut cmd.piped),
                self.stdin_text.take(),
            )
        };
        self.here_subst = false;
        // Settings that change how later words expand cannot be modelled.
        if words.iter().any(|w| {
            w.text
                .split_once('=')
                .is_some_and(|(n, _)| matches!(n, "GLOBIGNORE" | "CDPATH"))
        }) {
            self.unparsed = true;
        }
        // Reserved words, grouping and leading assignments.
        let mut skip = 0;
        loop {
            while words.get(skip).is_some_and(|w| is_assignment(&w.text)) {
                skip += 1;
            }
            match words.get(skip) {
                Some(w)
                    if !w.quoted
                        && !w.var
                        && RESERVED.contains(&w.text.as_str())
                        && !(w.text == "time"
                            && words.get(skip + 1).is_some_and(|n| n.text.starts_with('-'))) =>
                {
                    skip += 1
                }
                Some(w) if !w.quoted && w.text == "function" => {
                    skip += 1;
                    if words.get(skip).is_some() {
                        skip += 1;
                    }
                }
                _ => break,
            }
        }
        // Leading `f=…` / `f+=…` reassign `f`.
        for w in &words[..(skip + 1).min(words.len())] {
            if let Some((name, _)) = w.text.split_once('=') {
                let name = name.strip_suffix('+').unwrap_or(name);
                // `name[0]=…` assigns an element of `name`.
                let name = name.split('[').next().unwrap_or(name);
                if name.chars().next().is_some_and(is_name_start)
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    self.invalidate(name);
                }
            }
        }
        let assigns: Vec<String> = words[..skip.min(words.len())]
            .iter()
            .filter(|w| is_assignment(&w.text))
            .map(|w| w.text.clone())
            .collect();
        words.drain(..skip.min(words.len()));
        if words.is_empty() {
            // Assignment-only command: inside a subshell, or after `set -a`, it
            // changes the environment of what follows.
            if !assigns.is_empty() && (self.auto_export || self.subshell > 0) {
                self.exports.extend(assigns);
            }
            return;
        }
        if self.commands.len() >= MAX_COMMANDS {
            self.unparsed = true;
            return;
        }

        // Wrappers: keep each wrapper's own subject, then the wrapped command.
        let mut start = 0;
        let mut levels = 0;
        loop {
            levels += 1;
            if levels > MAX_WRAPPERS {
                self.unparsed = true;
                break;
            }
            let argv = self.argv_of(&words[start..]);
            let name = argv[0].clone();
            if self.commands.len() >= MAX_COMMANDS {
                self.unparsed = true;
                return;
            }
            // What a remembered approval must also match: leading assignments, the
            // wrapper words in front of this command, and the directory.
            let mut parts: Vec<String> = Vec::new();
            if !assigns.is_empty() {
                parts.push(format!("a:{}", assigns.join(" ")));
            }
            if start > 0 {
                let wrappers: Vec<&str> = words[..start].iter().map(|w| w.text.as_str()).collect();
                parts.push(format!("w:{}", wrappers.join(" ")));
            }
            if !self.exports.is_empty() {
                parts.push(format!("e:{}", self.exports.join(" ")));
            }
            if self.bases.len() > 1 || self.bases.first() != Some(&self.sctx.workdir) {
                let dirs: Vec<String> = self
                    .bases
                    .iter()
                    .map(|b| b.to_string_lossy().into_owned())
                    .collect();
                parts.push(format!("d:{}", dirs.join(":")));
            }
            self.commands.push(argv);
            self.contexts.push(parts.join("\u{1}"));
            if name == "xargs" {
                // xargs gives its child no pipe on stdin.
                piped = false;
            }
            let skip = wrapper_skip(&words[start..], &name);
            if let (Some(n), "env") = (skip, name.as_str())
                && let Some(src) = env_split_string(&words[start..(start + n).min(words.len())])
            {
                self.parse_nested(&src, words[start].start, piped, false);
            }
            match skip {
                Some(n) if start + n < words.len() => start += n,
                _ => break,
            }
        }
        let inner = &words[start..];
        let argv0 = self.argv0_of(&inner[0]);

        // Command names built at run time cannot be matched.
        if inner[0].var {
            self.unparsed = true;
        } else {
            let mut alts = Vec::new();
            expand_braces(&inner[0].lits(), 2, &mut alts);
            if alts.len() > 1 {
                self.unparsed = true;
            }
        }

        // Options that widen globbing or change directory resolution.
        if matches!(argv0.as_str(), "shopt" | "setopt" | "unsetopt" | "set")
            && inner[1..].iter().any(|w| {
                let n: String = w
                    .text
                    .chars()
                    .filter(|c| *c != '_')
                    .map(|c| c.to_ascii_lowercase())
                    .collect();
                WIDENING_GLOB_OPTIONS.contains(&n.as_str())
            })
        {
            self.unparsed = true;
        }

        // Command strings.
        let fed = piped || stdin_text.as_ref().is_some_and(|t| t.denied);
        let base_name = argv0
            .trim_end_matches(|c: char| c.is_ascii_digit() || c == '.')
            .to_ascii_lowercase();
        let kind = base_name.as_str();
        let mut string_idx: Option<usize> = None;
        let mut script_idx: Option<usize> = None;
        let pos0 = inner[0].start;
        if matches!(
            kind,
            "sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" | "mksh" | "yash" | "tcsh" | "csh"
                | "elvish" | "nu" | "xonsh"
        ) {
            let mut i = 1;
            let mut script_arg = false;
            let mut stdin_flag = false;
            while let Some(w) = inner.get(i) {
                let t = w.text.as_str();
                if is_c_bundle(t) {
                    if inner.get(i + 1).is_some() {
                        string_idx = Some(i + 1);
                    }
                    break;
                }
                if matches!(t, "--version" | "--help") {
                    // Informational: no commands are read.
                    script_arg = true;
                    break;
                }
                if matches!(t, "--rcfile" | "--init-file") {
                    i += 2;
                    continue;
                }
                let short_bundle = (t.starts_with('-') || t.starts_with('+'))
                    && !t.starts_with("--")
                    && t.len() > 1
                    && t[1..].chars().all(|c| c.is_ascii_alphabetic());
                if short_bundle && t[1..].contains(['o', 'O']) {
                    // `-o NAME`, `-euo NAME`: the option consumes the next word.
                    i += 2;
                } else if t.starts_with('-') || t.starts_with('+') {
                    if t.starts_with('-') && !t.starts_with("--") && t[1..].contains('s') {
                        stdin_flag = true;
                    }
                    i += 1;
                } else {
                    script_arg = true;
                    script_idx = Some(i);
                    break;
                }
            }
            let script_prog = script_idx.map(|i| program_of_word(&inner[i]));
            if string_idx.is_none() {
                if stdin_flag || !script_arg || script_prog == Some(Program::Stdin) {
                    self.stdin_program(stdin_text, piped, pos0, false);
                } else if script_prog == Some(Program::Procsub) {
                    self.pipe_to_interpreter = true;
                }
            }
            if let Some(i) = string_idx {
                let src = inner[i].text.clone();
                self.parse_nested(&src, inner[i].start, fed, false);
            }
        } else if kind == "fish" {
            let code = inner[1..]
                .iter()
                .any(|w| is_c_bundle(&w.text) || w.text.starts_with("--command"));
            if !code {
                let prog = first_positional(&inner[1..]);
                self.handle_program(prog, stdin_text, piped, pos0, false);
            }
        } else if argv0 == "source" || argv0 == "." {
            self.no_remember = true;
            let prog = first_positional(&inner[1..]);
            self.handle_program(prog, stdin_text, piped, pos0, true);
        } else if let Some((prog, idx)) = interpreter_program(kind, &inner[1..]) {
            script_idx = idx.map(|i| i + 1);
            self.note_program(prog, fed);
        } else if matches!(kind, "script" | "su" | "sg") {
            let found = inner.iter().enumerate().find_map(|(i, w)| {
                let t = w.text.as_str();
                if is_c_bundle(t) || t == "--command" {
                    inner.get(i + 1).map(|n| (i + 1, n.text.clone()))
                } else {
                    t.strip_prefix("--command=").map(|s| (i, s.to_string()))
                }
            });
            if let Some((i, src)) = found {
                string_idx = Some(i);
                self.parse_nested(&src, inner[i].start, piped, false);
            }
        } else if argv0 == "eval" && inner.len() > 1 {
            let src = inner[1..]
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            // An expansion in the eval string means the directory it names is unknown.
            if inner[1..].iter().any(|w| w.var) {
                self.cwd_unknown = true;
            }
            self.parse_nested(&src, inner[1].start, piped, true);
        } else if matches!(kind, "parallel" | "expect") {
            // They run commands built from their input: nothing to inspect.
            self.unparsed = true;
        }

        // Environment changes that later commands inherit.
        match argv0.as_str() {
            "export" => self.exports.extend(
                inner[1..]
                    .iter()
                    .filter(|w| is_assignment(&w.text))
                    .map(|w| w.text.clone()),
            ),
            "declare" | "typeset"
                if inner[1..].iter().any(|w| {
                    w.text.starts_with('-') && !w.text.starts_with("--") && w.text.contains('x')
                }) =>
            {
                self.exports.extend(
                    inner[1..]
                        .iter()
                        .filter(|w| is_assignment(&w.text))
                        .map(|w| w.text.clone()),
                );
            }
            "set" if inner[1..].iter().any(|w| w.text == "-a") => self.auto_export = true,
            // `for NAME in WORDS`: NAME is bound to literal words or glob matches.
            "for" if inner.len() >= 4
                && inner[2].text == "in"
                && inner[3..].iter().all(|w| !w.var) =>
            {
                let values = self.for_values(&inner[3..]);
                self.pending_for = Some(BoundVar {
                    name: inner[1].text.clone(),
                    values,
                    valid: true,
                    fn_level: self.func_brace.len(),
                });
            }
            _ => {}
        }

        let sed_in_place = argv0 == "sed" && inner[1..].iter().any(|w| is_sed_in_place(&w.text));
        let writer = WRITERS.contains(&argv0.as_str()) || sed_in_place;
        // Commands that assign to the variables they name.
        if matches!(
            argv0.as_str(),
            "read" | "mapfile" | "readarray" | "declare" | "local" | "typeset" | "export"
                | "readonly" | "unset" | "getopts" | "printf" | "let"
        ) {
            // A nameref can write any variable.
            if matches!(argv0.as_str(), "declare" | "local" | "typeset")
                && inner[1..].iter().any(|w| {
                    w.text.starts_with('-')
                        && !w.text.starts_with("--")
                        && w.text[1..].contains('n')
                })
            {
                self.invalidate_all();
            }
            let names: Vec<String> = inner[1..]
                .iter()
                .filter(|w| !w.text.starts_with('-'))
                .map(|w| w.text.split('=').next().unwrap_or("").to_owned())
                .collect();
            for n in names {
                self.invalidate(&n);
            }
        }
        if matches!(argv0.as_str(), "eval" | "source" | ".") {
            self.invalidate_all();
        }
        let file_args = file_arg_indices(&argv0, inner, sed_in_place, script_idx);
        // The `for` header only binds; its words are not path subjects.
        if argv0 == "for" {
            return;
        }

        for (i, w) in inner.iter().enumerate().skip(1) {
            if self.paths.len() >= MAX_PATHS {
                self.unparsed = true;
                break;
            }
            if Some(i) == string_idx {
                continue;
            }
            if argv0 == "dd" {
                // `if=`/`of=` operands; a bound `for` variable is substituted.
                let variants: Vec<Word> = if w.var {
                    match self.substitute_word(w) {
                        Subst::Words(ws) => ws,
                        _ => {
                            if file_args.contains(&i) {
                                self.var_path = true;
                            }
                            continue;
                        }
                    }
                } else {
                    vec![w.clone()]
                };
                for v in variants {
                    let (op, rest) = if let Some(r) = v.text.strip_prefix("of=") {
                        (PathOp::Write, r)
                    } else if let Some(r) = v.text.strip_prefix("if=") {
                        (PathOp::Read, r)
                    } else {
                        continue;
                    };
                    if v.glob || rest.is_empty() {
                        continue;
                    }
                    if self.cwd_unknown && !rest.starts_with('/') && !rest.starts_with('~') {
                        self.var_path = true;
                    }
                    let canonical = self.candidates(rest);
                    for p in self.lexical_candidates(rest) {
                        if !canonical.contains(&p) {
                            self.alt_paths.push((v.start, op, p));
                        }
                    }
                    for p in canonical {
                        self.push_path(v.start, op, p);
                    }
                }
                continue;
            }
            if file_args.contains(&i) {
                match self.substitute_word(w) {
                    Subst::Words(ws) => {
                        // A bound `for` variable: each value is a path, with the real op.
                        for nw in ws {
                            self.emit_word_paths(&nw, writer);
                        }
                        continue;
                    }
                    _ => self.flag_unknown_path(w),
                }
            }
            if w.text.starts_with('-') {
                continue;
            }
            self.emit_word_paths(w, writer);
        }

        // `cd` tracking: later relative paths also resolve against the new directory.
        if argv0 == "cd" || argv0 == "pushd" {
            let target = inner[1..]
                .iter()
                .find(|w| !w.text.starts_with('-') || w.text == "-");
            let news: Vec<PathBuf> = match target {
                None => self.sctx.home.iter().cloned().collect(),
                Some(w) if w.text == "-" || w.glob => {
                    self.cwd_unknown = true;
                    Vec::new()
                }
                Some(w) if w.text.is_empty() => Vec::new(),
                Some(w) if w.var => match self.substitute_word(w) {
                    Subst::Words(ws) => ws.iter().flat_map(|nw| self.candidates(&nw.text)).collect(),
                    _ => {
                        self.cwd_unknown = true;
                        Vec::new()
                    }
                },
                Some(w) => {
                    let raw = if w.home_chars > 0 {
                        format!("~{}", &w.text[w.home_chars..])
                    } else {
                        w.text.clone()
                    };
                    self.candidates(&raw)
                }
            };
            if !news.is_empty() {
                let mut bases = news;
                for b in std::mem::take(&mut self.bases) {
                    if !bases.contains(&b) {
                        bases.push(b);
                    }
                }
                bases.truncate(MAX_BASES);
                self.bases = bases;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn sctx() -> SubjectContext {
        SubjectContext {
            workdir: PathBuf::from("/work/proj"),
            home: Some(PathBuf::from("/home/u")),
            case_insensitive: false,
        }
    }

    /// Deterministic fuzz: the parser must never panic or hang.
    #[test]
    fn fuzz_never_panics() {
        let alphabet: Vec<char> = "ab $()`'\"\\;&|<>\n\t#~*?[]{}=-/.0123$HOME<<EOF\u{e9}\u{1b}"
            .chars()
            .collect();
        let vocab = [
            "sh -c ",
            "bash -lc ",
            "eval ",
            "sudo ",
            "env ",
            "cd ",
            "cd ~; ",
            "$((",
            "))",
            "((",
            "{a,b}",
            "*",
            "[a-z]",
            "<<E\n",
            "E\n",
            "rm ",
            "cat ",
            "function f() { ",
            "} ",
            "if x; then ",
            "fi",
            "$(",
            ")",
            "`",
            "'",
            "\"",
            "$HOME/",
            "xargs ",
            "timeout 5 ",
            "dd of=",
        ];
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..1_500 {
            let len = (next() % 60) as usize;
            let s: String = (0..len)
                .map(|_| alphabet[(next() as usize) % alphabet.len()])
                .collect();
            let _ = parse_shell(&s, &sctx());
        }
        for _ in 0..1_500 {
            let n = (next() % 12) as usize;
            let s: String = (0..n)
                .map(|_| vocab[(next() as usize) % vocab.len()])
                .collect();
            let _ = parse_shell(&s, &sctx());
        }
    }

    #[test]
    fn long_inputs_are_linear_enough() {
        let c = sctx();
        for pat in [
            "a;", "$(", "'", "<<E\n", "a b ", "\\\n", "> x ", "{a,b}", "cd x;", "sh -c '", "$((",
            "* ", "sudo ", "eval ",
        ] {
            let s = pat.repeat(MAX_INPUT_BYTES / pat.len());
            let start = std::time::Instant::now();
            let _ = parse_shell(&s, &c);
            assert!(start.elapsed().as_secs() < 5, "{pat:?} too slow");
        }
    }
}
