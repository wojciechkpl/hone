use std::path::PathBuf;

use hone_index::{SemanticIndex, SymbolKind};

fn rs(name: &str) -> PathBuf {
    PathBuf::from(format!("{name}.rs"))
}

fn py(name: &str) -> PathBuf {
    PathBuf::from(format!("{name}.py"))
}

#[test]
fn test_index_rust_file_extracts_functions() {
    let mut idx = SemanticIndex::new();
    let path = rs("my_module");
    let content = "\
pub fn alpha() {}
pub fn beta() {}
fn gamma() {}
";
    idx.index_file(&path, content);

    let symbols = idx.symbols_in_file(&path);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();

    assert!(names.contains(&"alpha"), "expected alpha, got {names:?}");
    assert!(names.contains(&"beta"), "expected beta, got {names:?}");
    assert!(names.contains(&"gamma"), "expected gamma, got {names:?}");
    assert!(symbols.iter().all(|s| s.kind == SymbolKind::Function));
}

#[test]
fn test_index_python_file_extracts_classes() {
    let mut idx = SemanticIndex::new();
    let path = py("models");
    let content = "\
class Dog:
    def bark(self):
        pass

class Cat:
    def meow(self):
        pass
";
    idx.index_file(&path, content);

    let symbols = idx.symbols_in_file(&path);
    let classes: Vec<&str> = symbols
        .iter()
        .filter(|s| s.kind == SymbolKind::Class)
        .map(|s| s.name.as_str())
        .collect();

    assert!(classes.contains(&"Dog"), "expected Dog, got {classes:?}");
    assert!(classes.contains(&"Cat"), "expected Cat, got {classes:?}");
}

#[test]
fn test_search_returns_relevant_results() {
    let mut idx = SemanticIndex::new();
    let path = rs("lib");
    let content = "\
pub fn compute_total() {}
pub fn compute_average() {}
pub fn format_output() {}
";
    idx.index_file(&path, content);

    let results = idx.search("compute");
    assert_eq!(results.len(), 2, "expected 2 results for 'compute'");

    let names: Vec<&str> = results.iter().map(|r| r.symbol.name.as_str()).collect();
    assert!(names.contains(&"compute_total"), "missing compute_total");
    assert!(names.contains(&"compute_average"), "missing compute_average");
}

#[test]
fn test_symbol_at_position_returns_correct_symbol() {
    let mut idx = SemanticIndex::new();
    let path = rs("positions");
    // fn first is at line 1, fn second at line 2.
    let content = "fn first() {}\nfn second() {}\n";
    idx.index_file(&path, content);

    // Querying line 2 should return `second`.
    let sym = idx.symbol_at(&path, 2, 3);
    assert!(sym.is_some(), "expected a symbol at line 2");
    assert_eq!(sym.unwrap().name, "second");

    // Querying line 1 should return `first`.
    let sym = idx.symbol_at(&path, 1, 0);
    assert!(sym.is_some(), "expected a symbol at line 1");
    assert_eq!(sym.unwrap().name, "first");
}
