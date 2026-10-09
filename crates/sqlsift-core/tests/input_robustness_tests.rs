// Inputs that used to crash or confuse the analyzer: very long operator chains
// (stack overflow) and a UTF-8 byte order mark at the start of a file (#116).
use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::error::{Diagnostic, DiagnosticKind};
use sqlsift_core::schema::{Catalog, SchemaBuilder};
use sqlsift_core::stack::with_analysis_stack;

fn catalog() -> Catalog {
    let mut builder = SchemaBuilder::new();
    builder
        .parse("CREATE TABLE users (id INTEGER NOT NULL, name TEXT);")
        .unwrap();
    builder.build().0
}

/// `SELECT id FROM users WHERE id = 0 OR id = 1 OR ...` with `terms` terms
fn or_chain(terms: usize, op: &str) -> String {
    let filter: Vec<String> = (0..terms).map(|i| format!("id = {i}")).collect();
    format!("SELECT id FROM users WHERE {};", filter.join(op))
}

fn kinds(diagnostics: &[Diagnostic]) -> Vec<DiagnosticKind> {
    diagnostics.iter().map(|d| d.kind).collect()
}

// ---------------------------------------------------------------------------
// Long operator chains
// ---------------------------------------------------------------------------

#[test]
fn long_or_chain_is_analyzed_on_a_default_thread_stack() {
    // Spawned threads (like the CLI's workers used to be) get a 2 MiB stack;
    // a 3000-term chain overflowed it in the name resolution walk
    let catalog = catalog();
    let sql = or_chain(3000, " OR ");
    let diagnostics = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn_scoped(scope, || Analyzer::new(&catalog).analyze(&sql))
            .unwrap()
            .join()
            .unwrap()
    });
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn very_long_or_chain_is_analyzed_with_the_analysis_stack() {
    let catalog = catalog();
    for op in [" OR ", " AND "] {
        let sql = or_chain(20_000, op);
        let diagnostics = with_analysis_stack(|| Analyzer::new(&catalog).analyze(&sql));
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }
}

#[test]
fn errors_inside_a_long_or_chain_are_still_reported() {
    let catalog = catalog();
    let sql = or_chain(20_000, " OR ").replace("id = 19999;", "nme = 'x' OR id = 'abc';");
    let diagnostics = with_analysis_stack(|| Analyzer::new(&catalog).analyze(&sql));
    assert_eq!(
        kinds(&diagnostics),
        [DiagnosticKind::ColumnNotFound, DiagnosticKind::TypeMismatch],
        "{diagnostics:?}"
    );
}

#[test]
fn long_arithmetic_chain_is_type_checked() {
    let catalog = catalog();
    let terms = vec!["id"; 5000].join(" + ");
    let ok = format!("SELECT {terms} FROM users WHERE {terms} > 0;");
    let bad = format!("SELECT {terms} + name FROM users;");
    let (ok, bad) = with_analysis_stack(|| {
        let mut analyzer = Analyzer::new(&catalog);
        (analyzer.analyze(&ok), analyzer.analyze(&bad))
    });
    assert!(ok.is_empty(), "{ok:?}");
    assert_eq!(kinds(&bad), [DiagnosticKind::TypeMismatch], "{bad:?}");
}

#[test]
fn analysis_stack_propagates_panics() {
    let result = std::panic::catch_unwind(|| with_analysis_stack(|| panic!("boom")));
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// UTF-8 byte order mark
// ---------------------------------------------------------------------------

const BOM: &str = "\u{feff}";

#[test]
fn bom_at_start_of_query_is_ignored() {
    let catalog = catalog();
    let diagnostics = Analyzer::new(&catalog).analyze(&format!("{BOM}SELECT 1;"));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn bom_keeps_locations_relative_to_the_original_text() {
    let catalog = catalog();
    let sql = format!("{BOM}SELECT nme FROM users;\nSELECT bogus FROM users;\n");
    let diagnostics = Analyzer::new(&catalog).analyze(&sql);
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");

    // Columns are what an editor shows (the BOM is invisible); byte offsets
    // index the text as given
    let first = diagnostics[0].span.unwrap();
    assert_eq!((first.line, first.column), (1, 8));
    assert!(sql[first.offset..].starts_with("nme"));
    let second = diagnostics[1].span.unwrap();
    assert_eq!((second.line, second.column), (2, 8));
    assert!(sql[second.offset..].starts_with("bogus"));
}

#[test]
fn bom_before_a_syntax_error() {
    let catalog = catalog();
    let sql = format!("{BOM}SELEC id FROM users;");
    let diagnostics = Analyzer::new(&catalog).analyze(&sql);
    assert_eq!(kinds(&diagnostics), [DiagnosticKind::ParseError]);
    let span = diagnostics[0].span.unwrap();
    assert_eq!((span.line, span.column), (1, 1));
    assert!(sql[span.offset..].starts_with("SELEC"));
}

#[test]
fn bom_with_inline_directive_on_first_line() {
    let catalog = catalog();
    let sql = format!("{BOM}-- sqlsift:disable-file E0002\nSELECT bogus FROM users;");
    let diagnostics = Analyzer::new(&catalog).analyze(&sql);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn bom_in_embedded_sql_source() {
    let catalog = catalog();
    let source = format!("{BOM}const r = sql`SELECT nme FROM users`;");
    let diagnostics = Analyzer::new(&catalog).analyze_embedded(&source, &["sql"]);
    assert_eq!(kinds(&diagnostics), [DiagnosticKind::ColumnNotFound]);
    let span = diagnostics[0].span.unwrap();
    assert_eq!((span.line, span.column), (1, 22));
    assert!(source[span.offset..].starts_with("nme"));
}

#[test]
fn bom_at_start_of_schema_is_ignored() {
    let mut builder = SchemaBuilder::new();
    builder
        .parse(&format!("{BOM}CREATE TABLE users (id INTEGER);"))
        .unwrap();
    let (catalog, warnings) = builder.build();
    assert!(warnings.is_empty(), "{warnings:?}");
    let diagnostics = Analyzer::new(&catalog).analyze("SELECT id FROM users");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn bom_before_dbmate_up_marker_in_schema() {
    let mut builder = SchemaBuilder::new();
    builder
        .parse(&format!(
            "{BOM}-- migrate:up\nCREATE TABLE users (id INTEGER);\n-- migrate:down\nDROP TABLE users;\n"
        ))
        .unwrap();
    let (catalog, _) = builder.build();
    let diagnostics = Analyzer::new(&catalog).analyze("SELECT id FROM users");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}
