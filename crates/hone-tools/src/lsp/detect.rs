use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// The programming language associated with a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Go,
    Java,
    C,
    Cpp,
    Unknown,
}

/// Detect the language of a file by extension, falling back to a shebang
/// check for extensionless files.
pub fn detect_language(path: &Path) -> Language {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        match ext {
            "rs" => return Language::Rust,
            "py" | "pyw" => return Language::Python,
            "ts" | "tsx" | "mts" | "cts" => return Language::TypeScript,
            "js" | "jsx" | "mjs" | "cjs" => return Language::JavaScript,
            "go" => return Language::Go,
            "java" => return Language::Java,
            "c" | "h" => return Language::C,
            "cpp" | "cxx" | "cc" | "hpp" | "hxx" => return Language::Cpp,
            _ => {}
        }
    }

    // Shebang fallback for extensionless files.
    if let Ok(file) = File::open(path) {
        let mut reader = BufReader::new(file);
        let mut first_line = String::new();
        if reader.read_line(&mut first_line).is_ok() {
            let line = first_line.trim();
            if let Some(shebang) = line.strip_prefix("#!") {
                if shebang.contains("python") {
                    return Language::Python;
                }
                if shebang.contains("node") {
                    return Language::JavaScript;
                }
            }
        }
    }

    Language::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn path(name: &str) -> PathBuf {
        PathBuf::from(name)
    }

    #[test]
    fn detects_rust() {
        assert_eq!(detect_language(&path("main.rs")), Language::Rust);
    }

    #[test]
    fn detects_python() {
        assert_eq!(detect_language(&path("script.py")), Language::Python);
    }

    #[test]
    fn detects_typescript() {
        assert_eq!(detect_language(&path("app.ts")), Language::TypeScript);
        assert_eq!(detect_language(&path("comp.tsx")), Language::TypeScript);
    }

    #[test]
    fn detects_javascript() {
        assert_eq!(detect_language(&path("index.js")), Language::JavaScript);
    }

    #[test]
    fn detects_go() {
        assert_eq!(detect_language(&path("main.go")), Language::Go);
    }

    #[test]
    fn detects_java() {
        assert_eq!(detect_language(&path("Main.java")), Language::Java);
    }

    #[test]
    fn detects_c() {
        assert_eq!(detect_language(&path("foo.c")), Language::C);
        assert_eq!(detect_language(&path("foo.h")), Language::C);
    }

    #[test]
    fn detects_cpp() {
        assert_eq!(detect_language(&path("foo.cpp")), Language::Cpp);
        assert_eq!(detect_language(&path("foo.hpp")), Language::Cpp);
    }

    #[test]
    fn unknown_extension() {
        assert_eq!(detect_language(&path("data.xyz")), Language::Unknown);
    }
}
