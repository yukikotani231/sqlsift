//! Output of `sqlsift baseline stats`: the entries of a baseline file counted by
//! rule and by file

use std::collections::BTreeMap;
use std::fmt::Write;

use sqlsift_core::baseline::Baseline;

use crate::args::BaselineFormat;

/// (name, entries) pairs
type Counts<'a> = Vec<(&'a str, usize)>;

/// Entry counts of a baseline file
pub struct BaselineStats<'a> {
    /// Path of the baseline file, as given
    pub path: &'a str,
    pub baseline: &'a Baseline,
}

impl BaselineStats<'_> {
    /// (code, entries) by code, and (file, entries) by number of entries and
    /// then file name
    fn counts(&self) -> (Counts<'_>, Counts<'_>) {
        let mut rules: BTreeMap<&str, usize> = BTreeMap::new();
        let mut files: BTreeMap<&str, usize> = BTreeMap::new();
        for entry in &self.baseline.entries {
            *rules.entry(&entry.code).or_default() += 1;
            *files.entry(&entry.file).or_default() += 1;
        }
        let mut files: Vec<_> = files.into_iter().collect();
        files.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        (rules.into_iter().collect(), files)
    }

    pub fn render(&self, format: BaselineFormat) -> String {
        match format {
            BaselineFormat::Human => self.render_human(),
            BaselineFormat::Json => self.render_json(),
        }
    }

    fn render_human(&self) -> String {
        let (rules, files) = self.counts();
        let total = self.baseline.entries.len();
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{}: {total} entr{} in {} file(s)",
            self.path,
            if total == 1 { "y" } else { "ies" },
            files.len()
        );
        if total == 0 {
            return out;
        }
        let width = files
            .iter()
            .map(|(_, n)| n.to_string().len())
            .max()
            .unwrap_or(1);
        let _ = writeln!(out, "\nBy rule:");
        for (code, n) in &rules {
            let name = sqlsift_core::rules::find_rule(code).map_or("", |rule| rule.name);
            let _ = writeln!(out, "  {n:>width$}  {code} {name}");
        }
        let _ = writeln!(out, "\nBy file:");
        for (file, n) in &files {
            let _ = writeln!(out, "  {n:>width$}  {file}");
        }
        out
    }

    fn render_json(&self) -> String {
        let (rules, files) = self.counts();
        let rules: Vec<_> = rules
            .iter()
            .map(|(code, n)| {
                serde_json::json!({
                    "code": code,
                    "name": sqlsift_core::rules::find_rule(code).map(|rule| rule.name),
                    "entries": n,
                })
            })
            .collect();
        let files: Vec<_> = files
            .iter()
            .map(|(file, n)| serde_json::json!({ "file": file, "entries": n }))
            .collect();
        let value = serde_json::json!({
            "baseline": self.path,
            "version": self.baseline.version,
            "entries": self.baseline.entries.len(),
            "rules": rules,
            "files": files,
        });
        let mut json = serde_json::to_string_pretty(&value).unwrap_or_default();
        json.push('\n');
        json
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlsift_core::baseline::BaselineEntry;

    fn entry(file: &str, code: &str) -> BaselineEntry {
        BaselineEntry {
            file: file.into(),
            code: code.into(),
            statement_hash: "0".into(),
            message: String::new(),
        }
    }

    #[test]
    fn counts_by_rule_and_by_file() {
        let baseline = Baseline {
            entries: vec![
                entry("a.sql", "E0002"),
                entry("b.sql", "E0002"),
                entry("b.sql", "E0001"),
            ],
            ..Baseline::default()
        };
        let stats = BaselineStats {
            path: "sqlsift-baseline.json",
            baseline: &baseline,
        };
        assert_eq!(
            stats.render(BaselineFormat::Human),
            "sqlsift-baseline.json: 3 entries in 2 file(s)\n\nBy rule:\n  1  E0001 table-not-found\n  2  E0002 column-not-found\n\nBy file:\n  2  b.sql\n  1  a.sql\n"
        );
        let json: serde_json::Value =
            serde_json::from_str(&stats.render(BaselineFormat::Json)).unwrap();
        assert_eq!(json["entries"], 3);
        assert_eq!(json["rules"][1]["name"], "column-not-found");
        assert_eq!(json["files"][0]["file"], "b.sql");
    }
}
