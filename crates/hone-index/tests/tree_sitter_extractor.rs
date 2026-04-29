use std::path::PathBuf;

use hone_index::{SymbolExtractor, SymbolKind};

fn rs_path() -> PathBuf {
    PathBuf::from("test.rs")
}

fn py_path() -> PathBuf {
    PathBuf::from("test.py")
}

fn has(symbols: &[hone_index::Symbol], name: &str, kind: SymbolKind) -> bool {
    symbols.iter().any(|s| s.name == name && s.kind == kind)
}

#[test]
fn test_extract_rust_function_with_lifetime() {
    let code = "fn foo<'a>(x: &'a str) -> &'a str { x }";
    let mut ex = SymbolExtractor::new();
    let symbols = ex.extract(&rs_path(), code);
    assert!(
        has(&symbols, "foo", SymbolKind::Function),
        "expected function 'foo', got {symbols:?}"
    );
}

#[test]
fn test_extract_rust_generic_struct() {
    let code = "struct Foo<T> { value: T }";
    let mut ex = SymbolExtractor::new();
    let symbols = ex.extract(&rs_path(), code);
    assert!(
        has(&symbols, "Foo", SymbolKind::Struct),
        "expected struct 'Foo', got {symbols:?}"
    );
}

#[test]
fn test_extract_rust_async_function() {
    let code = "async fn bar() -> u32 { 42 }";
    let mut ex = SymbolExtractor::new();
    let symbols = ex.extract(&rs_path(), code);
    assert!(
        has(&symbols, "bar", SymbolKind::Function),
        "expected function 'bar', got {symbols:?}"
    );
}

#[test]
fn test_extract_rust_pub_const() {
    let code = "pub const MAX: u32 = 100;";
    let mut ex = SymbolExtractor::new();
    let symbols = ex.extract(&rs_path(), code);
    assert!(
        has(&symbols, "MAX", SymbolKind::Constant),
        "expected constant 'MAX', got {symbols:?}"
    );
}

#[test]
fn test_extract_rust_static() {
    let code = "static COUNTER: u64 = 0;";
    let mut ex = SymbolExtractor::new();
    let symbols = ex.extract(&rs_path(), code);
    assert!(
        has(&symbols, "COUNTER", SymbolKind::Constant),
        "expected constant 'COUNTER' from static item, got {symbols:?}"
    );
}

#[test]
fn test_extract_rust_type_alias() {
    let code = "type Bytes = Vec<u8>;";
    let mut ex = SymbolExtractor::new();
    let symbols = ex.extract(&rs_path(), code);
    assert!(
        has(&symbols, "Bytes", SymbolKind::TypeAlias),
        "expected type alias 'Bytes', got {symbols:?}"
    );
}

#[test]
fn test_extract_rust_trait_impl() {
    let code = "impl std::fmt::Display for MyError { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, \"err\") } }";
    let mut ex = SymbolExtractor::new();
    let symbols = ex.extract(&rs_path(), code);
    // The impl target type is MyError; fmt is an inner function.
    assert!(
        has(&symbols, "MyError", SymbolKind::Impl),
        "expected impl 'MyError', got {symbols:?}"
    );
}

#[test]
fn test_extract_rust_multiple_kinds() {
    let code = "\
pub struct Config { pub port: u16 }
pub enum Mode { Debug, Release }
pub trait Configurable { fn configure(&self); }
impl Configurable for Config { fn configure(&self) {} }
pub fn build(cfg: Config) -> Mode { Mode::Debug }
pub const DEFAULT_PORT: u16 = 8080;
pub mod server {}
pub type Result<T> = std::result::Result<T, String>;
";
    let mut ex = SymbolExtractor::new();
    let symbols = ex.extract(&rs_path(), code);

    assert!(
        has(&symbols, "Config", SymbolKind::Struct),
        "missing Config struct"
    );
    assert!(has(&symbols, "Mode", SymbolKind::Enum), "missing Mode enum");
    assert!(
        has(&symbols, "Configurable", SymbolKind::Trait),
        "missing Configurable trait"
    );
    assert!(
        has(&symbols, "Config", SymbolKind::Impl),
        "missing Config impl"
    );
    assert!(
        has(&symbols, "build", SymbolKind::Function),
        "missing build fn"
    );
    assert!(
        has(&symbols, "DEFAULT_PORT", SymbolKind::Constant),
        "missing DEFAULT_PORT const"
    );
    assert!(
        has(&symbols, "server", SymbolKind::Module),
        "missing server mod"
    );
    assert!(
        has(&symbols, "Result", SymbolKind::TypeAlias),
        "missing Result type alias"
    );
}

#[test]
fn test_extract_python_class_with_methods() {
    let code = "\
class Animal:
    def __init__(self, name: str):
        self.name = name

    def speak(self) -> str:
        pass

    def describe(self) -> str:
        return f'Animal: {self.name}'
";
    let mut ex = SymbolExtractor::new();
    let symbols = ex.extract(&py_path(), code);

    assert!(
        has(&symbols, "Animal", SymbolKind::Class),
        "expected class 'Animal', got {symbols:?}"
    );
    assert!(
        has(&symbols, "__init__", SymbolKind::Function),
        "expected method '__init__', got {symbols:?}"
    );
    assert!(
        has(&symbols, "speak", SymbolKind::Function),
        "expected method 'speak', got {symbols:?}"
    );
    assert!(
        has(&symbols, "describe", SymbolKind::Function),
        "expected method 'describe', got {symbols:?}"
    );
}

#[test]
fn test_extract_python_decorated_function() {
    let code = "\
@cached
def foo():
    pass
";
    let mut ex = SymbolExtractor::new();
    let symbols = ex.extract(&py_path(), code);
    assert!(
        has(&symbols, "foo", SymbolKind::Function),
        "expected decorated function 'foo', got {symbols:?}"
    );
    // Must appear exactly once despite matching both decorated and plain patterns.
    let count = symbols
        .iter()
        .filter(|s| s.name == "foo" && s.kind == SymbolKind::Function)
        .count();
    assert_eq!(
        count, 1,
        "decorated function 'foo' should appear exactly once"
    );
}

#[test]
fn test_extract_python_decorated_class() {
    let code = "\
@dataclass
class Point:
    x: float
    y: float
";
    let mut ex = SymbolExtractor::new();
    let symbols = ex.extract(&py_path(), code);
    assert!(
        has(&symbols, "Point", SymbolKind::Class),
        "expected decorated class 'Point', got {symbols:?}"
    );
    let count = symbols
        .iter()
        .filter(|s| s.name == "Point" && s.kind == SymbolKind::Class)
        .count();
    assert_eq!(
        count, 1,
        "decorated class 'Point' should appear exactly once"
    );
}

#[test]
fn test_extract_python_async_function() {
    let code = "\
async def fetch_data(url: str):
    pass
";
    let mut ex = SymbolExtractor::new();
    let symbols = ex.extract(&py_path(), code);
    assert!(
        has(&symbols, "fetch_data", SymbolKind::Function),
        "expected async function 'fetch_data', got {symbols:?}"
    );
}

#[test]
fn test_extract_handles_unparseable_code() {
    // Severely malformed Rust — tree-sitter is error-tolerant and will still
    // produce a (partial) tree rather than returning None, so we only assert
    // that the call does not panic and returns a Vec.
    let garbage = "fn @@@((({{{{ }}})))";
    let mut ex = SymbolExtractor::new();
    let symbols = ex.extract(&rs_path(), garbage);
    // No assertion on count — just no panic.
    let _ = symbols;
}

#[test]
fn test_extract_line_numbers_are_one_based() {
    let code = "\nfn second_line() {}\n";
    let mut ex = SymbolExtractor::new();
    let symbols = ex.extract(&rs_path(), code);
    let sym = symbols
        .iter()
        .find(|s| s.name == "second_line")
        .expect("expected symbol 'second_line'");
    assert_eq!(sym.line, 2, "function on second line should have line=2");
}

#[test]
fn test_extract_unknown_extension_returns_empty() {
    let mut ex = SymbolExtractor::new();
    let symbols = ex.extract(&PathBuf::from("main.cpp"), "void foo() {}");
    assert!(
        symbols.is_empty(),
        "unknown extension should yield no symbols"
    );
}
