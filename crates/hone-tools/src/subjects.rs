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
}

impl ToolSubjects {
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
            let p = subject_path(raw, sctx);
            for op in ops {
                ts.add_path(*op, p.clone());
            }
        }
        ts
    }

    /// Read of a search root (`grep`/`glob`); defaults to the workdir.
    pub(crate) fn search_root(path: Option<&str>, sctx: &SubjectContext) -> Self {
        let p = match path {
            Some(p) => subject_path(p, sctx),
            None => sctx.workdir.clone(),
        };
        let mut ts = ToolSubjects::default();
        ts.add_path(PathOp::Read, p);
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
            let p = subject_path(path, sctx);
            ts.add_path(PathOp::Read, p.clone());
            ts.add_path(PathOp::Write, p);
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
                let p = subject_path(&path.to_string_lossy(), sctx);
                ts.add_path(PathOp::Read, p.clone());
                ts.add_path(PathOp::Write, p);
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
            unparsed: false,
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
            unparsed: false,
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

/// Maximum words produced by brace expansion, and glob matches per word.
const MAX_BRACE_WORDS: usize = 64;
const MAX_GLOB_MATCHES: usize = 256;
const MAX_DIR_ENTRIES: usize = 4096;
const MAX_BASES: usize = 16;

#[derive(Default)]
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
}

struct Parser<'a> {
    chars: Vec<char>,
    pos: usize,
    sctx: &'a SubjectContext,
    commands: Vec<Vec<String>>,
    paths: Vec<(usize, PathOp, PathBuf)>,
    unparsed: bool,
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
    ShellParse {
        commands: p.commands,
        paths: paths.into_iter().map(|(_, op, path)| (op, path)).collect(),
        unparsed: p.unparsed,
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
        "setsid" | "coproc" => (&[], false, 0),
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
            unparsed: false,
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
        let mut cmd = Cmd::default();
        let r = self.parse_list_inner(&mut cmd, term, depth);
        if r.is_err() {
            self.end_cmd(&mut cmd);
        }
        self.depth = saved_depth;
        r
    }

    fn parse_list_inner(&mut self, cmd: &mut Cmd, term: Term, depth: usize) -> Res<()> {
        let mut parens = 0usize;
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
                ';' | '|' => {
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
                        self.pos += 2;
                        self.substitute(Term::Arith, depth)?;
                    } else if self.function_def_ahead(cmd) {
                        cmd.words.clear();
                    } else {
                        self.pos += 1;
                        parens += 1;
                        self.end_cmd(cmd);
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
                '>' | '<' if !self.arith => self.redirect(depth)?,
                c if !self.arith && c.is_ascii_digit() && self.fd_redirect_ahead() => {
                    self.redirect(depth)?
                }
                _ => {
                    let w = self.read_word(depth)?;
                    if w.started {
                        cmd.words.push(w);
                    }
                }
            }
        }
    }

    /// At `(`: is this `NAME()` or `function NAME()`? If so, consume `()`.
    fn function_def_ahead(&mut self, cmd: &Cmd) -> bool {
        let named = match cmd.words.as_slice() {
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
            for (p, prefix) in self.word_paths(&w) {
                self.push_path(start, if prefix { PathOp::Read } else { op }, p);
            }
        }
        Ok(())
    }

    fn heredoc_or_herestring(&mut self, depth: usize) -> Res<()> {
        if self.peek() == Some('<') {
            // here-string: the word is data, not a path.
            self.pos += 1;
            self.skip_blanks();
            self.read_word(depth)?;
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
        self.heredocs.push((w.text.chars().collect(), strip));
        Ok(())
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
        let r = self.parse_list(term, depth + 1);
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
            out.push((resolve_path(&p), false));
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
    fn parse_nested(&mut self, src: &str, pos: usize) {
        let depth = self.depth + 1;
        if depth > MAX_DEPTH || src.len() > MAX_INPUT_BYTES {
            self.unparsed = true;
            return;
        }
        let mut child = Parser::new(src, self.sctx, self.bases.clone());
        if child.parse_list(Term::Eof, depth).is_err() {
            child.unparsed = true;
        }
        self.unparsed |= child.unparsed | child.glob_overflow.get();
        for argv in child.commands {
            if self.commands.len() >= MAX_COMMANDS {
                self.unparsed = true;
                break;
            }
            self.commands.push(argv);
        }
        for (_, op, p) in child.paths {
            self.push_path(pos, op, p);
        }
    }

    /// Finish the current simple command: record its argv and path subjects.
    fn end_cmd(&mut self, cmd: &mut Cmd) {
        if self.arith {
            cmd.words.clear();
            return;
        }
        let mut words = std::mem::take(&mut cmd.words);
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
        words.drain(..skip.min(words.len()));
        if words.is_empty() {
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
            self.commands.push(argv);
            let skip = wrapper_skip(&words[start..], &name);
            if let (Some(n), "env") = (skip, name.as_str())
                && let Some(src) = env_split_string(&words[start..(start + n).min(words.len())])
            {
                self.parse_nested(&src, words[start].start);
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
        let mut string_idx: Option<usize> = None;
        if matches!(argv0.as_str(), "sh" | "bash" | "zsh" | "dash" | "ksh") {
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
                    break;
                }
            }
            if string_idx.is_none() && (stdin_flag || !script_arg) {
                // Commands come from stdin: nothing to inspect.
                self.unparsed = true;
            }
            if let Some(i) = string_idx {
                let src = inner[i].text.clone();
                self.parse_nested(&src, inner[i].start);
            }
        } else if argv0 == "su" {
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
                self.parse_nested(&src, inner[i].start);
            }
        } else if argv0 == "eval" && inner.len() > 1 {
            let src = inner[1..]
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            self.parse_nested(&src, inner[1].start);
        }

        let sed_in_place = argv0 == "sed" && inner[1..].iter().any(|w| is_sed_in_place(&w.text));
        let writer = WRITERS.contains(&argv0.as_str()) || sed_in_place;

        for (i, w) in inner.iter().enumerate().skip(1) {
            if self.paths.len() >= MAX_PATHS {
                self.unparsed = true;
                break;
            }
            if Some(i) == string_idx {
                continue;
            }
            if argv0 == "dd" {
                let (op, rest) = if let Some(r) = w.text.strip_prefix("of=") {
                    (PathOp::Write, r)
                } else if let Some(r) = w.text.strip_prefix("if=") {
                    (PathOp::Read, r)
                } else {
                    continue;
                };
                if w.var || w.glob || rest.is_empty() {
                    continue;
                }
                for p in self.candidates(rest) {
                    self.push_path(w.start, op, p);
                }
                continue;
            }
            if w.text.starts_with('-') {
                continue;
            }
            for (p, prefix) in self.word_paths(w) {
                self.push_path(w.start, PathOp::Read, p.clone());
                if writer && !prefix {
                    self.push_path(w.start, PathOp::Write, p);
                }
            }
        }

        // `cd` tracking: later relative paths also resolve against the new directory.
        if argv0 == "cd" || argv0 == "pushd" {
            let target = inner[1..]
                .iter()
                .find(|w| !w.text.starts_with('-') || w.text == "-");
            let news: Vec<PathBuf> = match target {
                None => self.sctx.home.iter().cloned().collect(),
                Some(w) if w.var || w.glob || w.text == "-" || w.text.is_empty() => Vec::new(),
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
