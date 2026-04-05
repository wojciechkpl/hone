use std::path::Path;

use crate::types::{Symbol, SymbolKind};

/// Extracts symbols from source files using keyword-based heuristics.
pub struct SymbolExtractor;

impl SymbolExtractor {
    pub fn new() -> Self {
        Self
    }

    pub fn extract(&self, path: &Path, content: &str) -> Vec<Symbol> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match ext {
            "rs" => extract_rust(path, content),
            "py" => extract_python(path, content),
            _ => vec![],
        }
    }
}

impl Default for SymbolExtractor {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Rust extractor
// ---------------------------------------------------------------------------

/// Keywords and their associated `SymbolKind`, ordered so that the first match
/// wins (more specific keywords come first).
const RUST_KEYWORDS: &[(&str, SymbolKind)] = &[
    ("fn ", SymbolKind::Function),
    ("struct ", SymbolKind::Struct),
    ("enum ", SymbolKind::Enum),
    ("trait ", SymbolKind::Trait),
    ("impl ", SymbolKind::Impl),
    ("const ", SymbolKind::Constant),
    ("type ", SymbolKind::TypeAlias),
    ("mod ", SymbolKind::Module),
];

fn extract_rust(path: &Path, content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();

    for (line_idx, raw_line) in content.lines().enumerate() {
        let line_number = line_idx as u32 + 1;

        // Strip leading whitespace for matching, but remember original offset.
        let trimmed = raw_line.trim_start();

        // Skip comment lines.
        if trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }

        // Strip visibility and `async` prefixes so the keyword check works
        // regardless of `pub`, `pub(crate)`, `pub(super)`, `async fn`, etc.
        let stripped = strip_rust_prefixes(trimmed);

        for &(keyword, kind) in RUST_KEYWORDS {
            if let Some(after_kw) = stripped.strip_prefix(keyword) {
                // For `impl` we want the last type name (after an optional
                // `Trait for`) rather than a generic parameter.
                let name = if kind == SymbolKind::Impl {
                    parse_impl_name(after_kw)
                } else {
                    parse_ident(after_kw)
                };

                if let Some(name) = name {
                    // Find the column of the identifier in the original line.
                    let col = find_col(raw_line, &name);
                    symbols.push(Symbol {
                        name,
                        kind,
                        file: path.to_path_buf(),
                        line: line_number,
                        col,
                    });
                }
                break; // Only the first matching keyword wins per line.
            }
        }
    }

    symbols
}

/// Strip `pub`, `pub(...)`, `async`, `unsafe`, `extern "C"` and combinations
/// so that the resulting string starts with the actual declaration keyword.
fn strip_rust_prefixes(s: &str) -> &str {
    let mut rest = s;
    loop {
        let prev = rest;
        rest = rest.trim_start();
        if let Some(r) = rest.strip_prefix("pub") {
            // pub(crate), pub(super), pub(in ...) or bare pub
            let r = r.trim_start();
            rest = if r.starts_with('(') {
                // skip to matching ')'
                match r.find(')') {
                    Some(idx) => &r[idx + 1..],
                    None => r,
                }
            } else {
                r
            };
            continue;
        }
        for prefix in &["async ", "unsafe ", "extern \"C\" ", "extern \"Rust\" "] {
            if let Some(r) = rest.strip_prefix(prefix) {
                rest = r;
            }
        }
        if rest == prev {
            break;
        }
    }
    rest
}

/// Extract the last meaningful type name from an `impl` declaration.
/// Handles: `impl Foo`, `impl<T> Foo<T>`, `impl Trait for Foo`.
fn parse_impl_name(after_impl: &str) -> Option<String> {
    // Skip over generic parameters attached to `impl` itself: `impl<T, U>`
    let after_generics = skip_angle_brackets(after_impl.trim_start());

    // If there is a `for` keyword the target type comes after it.
    let target = if let Some(idx) = find_keyword_for(after_generics) {
        after_generics[idx + 4..].trim_start()
    } else {
        after_generics.trim_start()
    };

    parse_ident(target)
}

/// Find the index of ` for ` acting as the trait-impl separator,
/// but only at brace depth 0 (not inside angle brackets).
fn find_keyword_for(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth: usize = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => depth += 1,
            b'>' if depth > 0 => depth -= 1,
            b' ' if depth == 0 => {
                if s[i..].starts_with(" for ") {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Skip a leading `<...>` block (handling nesting) and return the rest.
fn skip_angle_brackets(s: &str) -> &str {
    if !s.starts_with('<') {
        return s;
    }
    let bytes = s.as_bytes();
    let mut depth: usize = 0;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    return &s[i + 1..];
                }
            }
            _ => {}
        }
    }
    s
}

/// Parse a Rust identifier from the start of `s`, stopping at the first
/// non-identifier character.
fn parse_ident(s: &str) -> Option<String> {
    let s = s.trim_start();
    let end = s
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(s.len());
    let ident = &s[..end];
    if ident.is_empty() {
        None
    } else {
        Some(ident.to_owned())
    }
}

// ---------------------------------------------------------------------------
// Python extractor
// ---------------------------------------------------------------------------

const PYTHON_KEYWORDS: &[(&str, SymbolKind)] = &[
    ("def ", SymbolKind::Function),
    ("async def ", SymbolKind::Function),
    ("class ", SymbolKind::Class),
];

fn extract_python(path: &Path, content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();

    for (line_idx, raw_line) in content.lines().enumerate() {
        let line_number = line_idx as u32 + 1;
        let trimmed = raw_line.trim_start();

        if trimmed.starts_with('#') {
            continue;
        }

        for &(keyword, kind) in PYTHON_KEYWORDS {
            if let Some(after_kw) = trimmed.strip_prefix(keyword) {
                if let Some(name) = parse_python_name(after_kw) {
                    let col = find_col(raw_line, &name);
                    symbols.push(Symbol {
                        name,
                        kind,
                        file: path.to_path_buf(),
                        line: line_number,
                        col,
                    });
                }
                break;
            }
        }
    }

    symbols
}

fn parse_python_name(s: &str) -> Option<String> {
    let end = s
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(s.len());
    let name = &s[..end];
    if name.is_empty() { None } else { Some(name.to_owned()) }
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// Return the 0-based column of the first occurrence of `name` in `line`,
/// or 0 if not found.
fn find_col(line: &str, name: &str) -> u32 {
    line.find(name).unwrap_or(0) as u32
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn rs(content: &str) -> Vec<Symbol> {
        SymbolExtractor::new().extract(&PathBuf::from("test.rs"), content)
    }

    fn py(content: &str) -> Vec<Symbol> {
        SymbolExtractor::new().extract(&PathBuf::from("test.py"), content)
    }

    fn has(symbols: &[Symbol], name: &str, kind: SymbolKind) -> bool {
        symbols.iter().any(|s| s.name == name && s.kind == kind)
    }

    #[test]
    fn rust_bare_fn() {
        let s = rs("fn hello() {}");
        assert!(has(&s, "hello", SymbolKind::Function));
    }

    #[test]
    fn rust_pub_fn() {
        let s = rs("pub fn hello() {}");
        assert!(has(&s, "hello", SymbolKind::Function));
    }

    #[test]
    fn rust_pub_async_fn() {
        let s = rs("pub async fn run() {}");
        assert!(has(&s, "run", SymbolKind::Function));
    }

    #[test]
    fn rust_struct() {
        let s = rs("pub struct MyStruct {}");
        assert!(has(&s, "MyStruct", SymbolKind::Struct));
    }

    #[test]
    fn rust_enum() {
        let s = rs("pub enum Color { Red }");
        assert!(has(&s, "Color", SymbolKind::Enum));
    }

    #[test]
    fn rust_trait() {
        let s = rs("pub trait Drawable {}");
        assert!(has(&s, "Drawable", SymbolKind::Trait));
    }

    #[test]
    fn rust_impl_plain() {
        let s = rs("impl Foo {}");
        assert!(has(&s, "Foo", SymbolKind::Impl));
    }

    #[test]
    fn rust_impl_trait_for() {
        let s = rs("impl Display for MyStruct {}");
        assert!(has(&s, "MyStruct", SymbolKind::Impl));
    }

    #[test]
    fn rust_const() {
        let s = rs("pub const MAX: usize = 100;");
        assert!(has(&s, "MAX", SymbolKind::Constant));
    }

    #[test]
    fn rust_type_alias() {
        let s = rs("pub type Result<T> = std::result::Result<T, Error>;");
        assert!(has(&s, "Result", SymbolKind::TypeAlias));
    }

    #[test]
    fn rust_mod() {
        let s = rs("pub mod utils {}");
        assert!(has(&s, "utils", SymbolKind::Module));
    }

    #[test]
    fn rust_skips_comments() {
        let s = rs("// fn not_a_symbol() {}");
        assert!(s.is_empty());
    }

    #[test]
    fn python_def() {
        let s = py("def compute(x):\n    pass\n");
        assert!(has(&s, "compute", SymbolKind::Function));
    }

    #[test]
    fn python_async_def() {
        let s = py("async def fetch():\n    pass\n");
        assert!(has(&s, "fetch", SymbolKind::Function));
    }

    #[test]
    fn python_class() {
        let s = py("class Animal:\n    pass\n");
        assert!(has(&s, "Animal", SymbolKind::Class));
    }

    #[test]
    fn python_skips_comments() {
        let s = py("# def not_real():\n    pass\n");
        assert!(s.is_empty());
    }

    #[test]
    fn unknown_ext_returns_empty() {
        let s = SymbolExtractor::new().extract(&PathBuf::from("foo.cpp"), "fn x() {}");
        assert!(s.is_empty());
    }
}
