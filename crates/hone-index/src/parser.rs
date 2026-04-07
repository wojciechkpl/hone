use std::path::Path;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor};

use crate::types::{Symbol, SymbolKind};

// ---------------------------------------------------------------------------
// SymbolExtractor
// ---------------------------------------------------------------------------

/// Extracts symbols from source files using tree-sitter AST parsing.
pub struct SymbolExtractor {
    rust_parser: Parser,
    python_parser: Parser,
    rust_language: Language,
    python_language: Language,
}

impl SymbolExtractor {
    pub fn new() -> Self {
        let rust_language: Language = tree_sitter_rust::LANGUAGE.into();
        let python_language: Language = tree_sitter_python::LANGUAGE.into();

        let mut rust_parser = Parser::new();
        rust_parser
            .set_language(&rust_language)
            .expect("failed to load Rust grammar");

        let mut python_parser = Parser::new();
        python_parser
            .set_language(&python_language)
            .expect("failed to load Python grammar");

        Self {
            rust_parser,
            python_parser,
            rust_language,
            python_language,
        }
    }

    pub fn extract(&mut self, path: &Path, content: &str) -> Vec<Symbol> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match ext {
            "rs" => self.extract_rust(path, content),
            "py" => self.extract_python(path, content),
            _ => Vec::new(),
        }
    }

    fn extract_rust(&mut self, path: &Path, content: &str) -> Vec<Symbol> {
        let tree = match self.rust_parser.parse(content, None) {
            Some(t) => t,
            None => return Vec::new(),
        };

        let query_source = "
            (function_item name: (identifier) @function)
            (struct_item name: (type_identifier) @struct)
            (enum_item name: (type_identifier) @enum)
            (trait_item name: (type_identifier) @trait)
            (impl_item type: (type_identifier) @impl)
            (mod_item name: (identifier) @module)
            (const_item name: (identifier) @constant)
            (static_item name: (identifier) @constant)
            (type_item name: (type_identifier) @type_alias)
        ";

        let query = Query::new(&self.rust_language, query_source)
            .expect("valid Rust query");

        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, tree.root_node(), content.as_bytes());

        let capture_names = query.capture_names();
        let mut symbols = Vec::new();

        while let Some(m) = matches.next() {
            for cap in m.captures {
                let capture_name = capture_names[cap.index as usize];
                let kind = match capture_name {
                    "function" => SymbolKind::Function,
                    "struct" => SymbolKind::Struct,
                    "enum" => SymbolKind::Enum,
                    "trait" => SymbolKind::Trait,
                    "impl" => SymbolKind::Impl,
                    "module" => SymbolKind::Module,
                    "constant" => SymbolKind::Constant,
                    "type_alias" => SymbolKind::TypeAlias,
                    _ => continue,
                };

                let name = &content[cap.node.byte_range()];
                let start = cap.node.start_position();

                symbols.push(Symbol {
                    name: name.to_string(),
                    kind,
                    file: path.to_path_buf(),
                    line: start.row as u32 + 1,
                    col: start.column as u32,
                });
            }
        }

        symbols
    }

    fn extract_python(&mut self, path: &Path, content: &str) -> Vec<Symbol> {
        let tree = match self.python_parser.parse(content, None) {
            Some(t) => t,
            None => return Vec::new(),
        };

        // The decorated_definition patterns duplicate names that are already
        // captured by the inner function_definition / class_definition patterns,
        // so we deduplicate by (name, line) after collection.
        let query_source = "
            (function_definition name: (identifier) @function)
            (class_definition name: (identifier) @class)
            (decorated_definition (function_definition name: (identifier) @function))
            (decorated_definition (class_definition name: (identifier) @class))
        ";

        let query = Query::new(&self.python_language, query_source)
            .expect("valid Python query");

        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, tree.root_node(), content.as_bytes());

        let capture_names = query.capture_names();
        let mut symbols: Vec<Symbol> = Vec::new();

        while let Some(m) = matches.next() {
            for cap in m.captures {
                let capture_name = capture_names[cap.index as usize];
                let kind = match capture_name {
                    "function" => SymbolKind::Function,
                    "class" => SymbolKind::Class,
                    _ => continue,
                };

                let name = &content[cap.node.byte_range()];
                let start = cap.node.start_position();
                let line = start.row as u32 + 1;
                let col = start.column as u32;

                // Skip duplicates produced by matching both the top-level and
                // decorated variants of the same declaration.
                if symbols
                    .iter()
                    .any(|s| s.name == name && s.kind == kind && s.line == line)
                {
                    continue;
                }

                symbols.push(Symbol {
                    name: name.to_string(),
                    kind,
                    file: path.to_path_buf(),
                    line,
                    col,
                });
            }
        }

        symbols
    }
}

impl Default for SymbolExtractor {
    fn default() -> Self {
        Self::new()
    }
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
        // tree-sitter captures the self-type (Foo), not the trait name.
        // The impl_item query matches `type:` which is the implementing type.
        // For `impl Display for MyStruct`, the type node is `MyStruct`.
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
        // Tree-sitter will not produce symbols from within comment text.
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
        // A comment line cannot produce Python AST nodes for functions/classes.
        let s = py("# def not_real():\n");
        assert!(s.is_empty());
    }

    #[test]
    fn unknown_ext_returns_empty() {
        let s = SymbolExtractor::new().extract(&PathBuf::from("foo.cpp"), "fn x() {}");
        assert!(s.is_empty());
    }
}
