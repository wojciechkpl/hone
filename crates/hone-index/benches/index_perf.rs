use criterion::{Criterion, black_box, criterion_group, criterion_main};
use hone_index::{SemanticIndex, parser::SymbolExtractor};
use std::path::Path;

fn bench_extract_rust_file(c: &mut Criterion) {
    let content = include_str!("../src/index.rs");
    let path = Path::new("test.rs");

    c.bench_function("extract_rust_symbols", |b| {
        let mut extractor = SymbolExtractor::new();
        b.iter(|| black_box(extractor.extract(black_box(path), black_box(content))));
    });
}

fn bench_index_search(c: &mut Criterion) {
    let mut index = SemanticIndex::new();
    let content = include_str!("../src/index.rs");
    index.index_file(Path::new("test.rs"), content);

    c.bench_function("search_index", |b| {
        b.iter(|| black_box(index.search(black_box("Index"))));
    });
}

criterion_group!(benches, bench_extract_rust_file, bench_index_search);
criterion_main!(benches);
