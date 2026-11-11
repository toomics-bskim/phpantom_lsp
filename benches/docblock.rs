use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use phpantom_lsp::docblock::parse_docblock_for_tags;
use std::fmt::Write as _;
use std::hint::black_box;

fn commented_aliases(fields_per_alias: usize) -> String {
    let mut doc = String::from("/**\n");
    for alias in 0..40 {
        writeln!(doc, " * @psalm-type Config{alias} = array{{").unwrap();
        for field in 0..fields_per_alias {
            writeln!(
                doc,
                " *     option{field}?: array{{enabled?: bool, value?: string|null}}, // Default: null"
            )
            .unwrap();
        }
        doc.push_str(" * }\n");
    }
    doc.push_str(" */");
    doc
}

fn bench_commented_aliases(c: &mut Criterion) {
    let mut group = c.benchmark_group("docblock_commented_aliases");
    for fields in [10, 20, 40, 80] {
        let doc = commented_aliases(fields);
        group.throughput(Throughput::Bytes(doc.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(fields), &doc, |b, doc| {
            b.iter(|| parse_docblock_for_tags(black_box(doc)));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_commented_aliases);
criterion_main!(benches);
