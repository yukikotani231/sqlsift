//! Times this parser against sqlparser on statements both parse to the same AST.
//!
//! `cargo run --release --example bench -- files.txt` (one SQL file path per line);
//! `REPS` sets the repetitions (default 20).

#![allow(clippy::cast_precision_loss, clippy::unwrap_used)]

use sqlparser::dialect::PostgreSqlDialect;
use sqlsift_parser::diff::{compare_source, Outcome};
use sqlsift_parser::lower::lower_statement;
use sqlsift_parser::{parse, Arena};
use std::time::Instant;

fn main() {
    let list = std::env::args().nth(1).unwrap();
    let mut stmts = vec![];
    let mut files = vec![];
    for f in std::fs::read_to_string(list).unwrap().lines() {
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        for s in compare_source(&src) {
            if matches!(s.outcome, Outcome::Same) {
                stmts.push(s.text);
            }
        }
        files.push(src);
    }
    let bytes: usize = stmts.iter().map(String::len).sum();
    println!("{} statements (same AST), {} KB", stmts.len(), bytes / 1024);
    let reps: u32 = std::env::var("REPS").map_or(20, |v| v.parse().unwrap());
    let t = Instant::now();
    for _ in 0..reps {
        for s in &stmts {
            std::hint::black_box(
                sqlparser::parser::Parser::parse_sql(&PostgreSqlDialect {}, s).unwrap(),
            );
        }
    }
    let theirs = t.elapsed().as_secs_f64() / f64::from(reps);
    // sqlparser's AST converted to the analyzer's, as sqlsift reads it without this parser
    let t = Instant::now();
    for _ in 0..reps {
        for s in &stmts {
            let parsed = sqlparser::parser::Parser::parse_sql(&PostgreSqlDialect {}, s).unwrap();
            std::hint::black_box(sqlsift_ast::from_sqlparser::statement(&parsed[0]));
        }
    }
    let converted = t.elapsed().as_secs_f64() / f64::from(reps);
    // One arena, reset before each statement, as the analyzer does per file.
    let mut arena = Arena::new();
    let t = Instant::now();
    for _ in 0..reps {
        for s in &stmts {
            arena.reset();
            std::hint::black_box(parse(&arena, s));
        }
    }
    let ours = t.elapsed().as_secs_f64() / f64::from(reps);
    let t = Instant::now();
    for _ in 0..reps {
        for s in &stmts {
            arena.reset();
            let p = parse(&arena, s);
            let stmt = p.statements().next().unwrap();
            std::hint::black_box(lower_statement(s, stmt).unwrap());
        }
    }
    let lowered = t.elapsed().as_secs_f64() / f64::from(reps);
    let mb = bytes as f64 / 1e6;
    println!(
        "sqlparser       {:7.1} ms  {:6.1} MB/s",
        theirs * 1e3,
        mb / theirs
    );
    println!(
        "sqlparser + conversion {:7.1} ms  {:6.1} MB/s",
        converted * 1e3,
        mb / converted
    );
    println!(
        "ours (tree)     {:7.1} ms  {:6.1} MB/s",
        ours * 1e3,
        mb / ours
    );
    println!(
        "ours + lowering {:7.1} ms  {:6.1} MB/s",
        lowered * 1e3,
        mb / lowered
    );
    let fbytes: usize = files.iter().map(String::len).sum();
    let t = Instant::now();
    for _ in 0..reps {
        for f in &files {
            arena.reset();
            std::hint::black_box(parse(&arena, f));
        }
    }
    let whole = t.elapsed().as_secs_f64() / f64::from(reps);
    println!(
        "ours, all {} files ({} KB, DDL included): {:.1} ms, {:.1} MB/s",
        files.len(),
        fbytes / 1024,
        whole * 1e3,
        fbytes as f64 / 1e6 / whole
    );
}
