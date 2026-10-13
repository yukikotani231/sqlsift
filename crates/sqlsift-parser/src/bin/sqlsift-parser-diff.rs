//! Compares the in-house parser with sqlparser on SQL files.
//!
//! ```text
//! cargo run -p sqlsift-parser --bin sqlsift-parser-diff -- [-v] <file or directory>...
//! ```
//!
//! Prints a count per outcome, the most common reasons for unsupported statements, and
//! each statement whose AST differs (with `-v`, also those whose spans differ; with `-e`,
//! an example statement per unsupported reason).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sqlsift_parser::diff::{compare_source, first_difference, Outcome};

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        entries.sort();
        for entry in entries {
            collect(&entry, out);
        }
    } else if path.extension().is_some_and(|e| e == "sql") {
        out.push(path.to_path_buf());
    }
}

fn main() {
    let mut verbose = false;
    let mut examples = false;
    let mut files = vec![];
    for arg in std::env::args().skip(1) {
        if arg == "-v" {
            verbose = true;
        } else if arg == "-e" {
            examples = true;
        } else {
            collect(Path::new(&arg), &mut files);
        }
    }
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    let mut example: BTreeMap<String, String> = BTreeMap::new();
    for file in &files {
        let Ok(source) = std::fs::read_to_string(file) else {
            continue;
        };
        for stmt in compare_source(&source) {
            *counts.entry(stmt.outcome.label()).or_default() += 1;
            let line = source[..stmt.offset].matches('\n').count() + 1;
            match &stmt.outcome {
                Outcome::Unsupported(reason) => {
                    *reasons.entry(reason.clone()).or_default() += 1;
                    example
                        .entry(reason.clone())
                        .or_insert_with(|| stmt.text.trim().replace('\n', " "));
                }
                Outcome::Different { ours, theirs } => {
                    println!(
                        "DIFFERENT {}:{line}\n{}\n{}",
                        file.display(),
                        stmt.text.trim(),
                        first_difference(ours, theirs)
                    );
                }
                Outcome::OnlyOurs if verbose => {
                    println!(
                        "ONLY OURS {}:{line}\n{}\n",
                        file.display(),
                        stmt.text.trim()
                    );
                }
                Outcome::SpansDiffer { ours, theirs } if verbose => {
                    println!(
                        "SPANS {}:{line}\n{}\n{}",
                        file.display(),
                        stmt.text.trim(),
                        first_difference(ours, theirs)
                    );
                }
                _ => {}
            }
        }
    }
    println!("files: {}", files.len());
    for (label, n) in &counts {
        println!("{label}: {n}");
    }
    let mut reasons: Vec<_> = reasons.into_iter().collect();
    reasons.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    println!("unsupported, by reason:");
    for (reason, n) in reasons.iter().take(30) {
        println!("  {n:5}  {reason}");
        if examples {
            let text = &example[reason];
            println!("         {}", text.chars().take(200).collect::<String>());
        }
    }
}
