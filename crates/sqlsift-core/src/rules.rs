//! Rule registry and per-rule configuration
//!
//! Every diagnostic sqlsift reports belongs to a rule. Rules are grouped into
//! categories (as in oxlint): the category decides a rule's default level, and
//! both categories and individual rules can be configured as `off`, `warn` or
//! `error`. A rule setting overrides the setting of its category.
//!
//! ```
//! use sqlsift_core::rules::{RuleConfig, RuleLevel};
//! use sqlsift_core::DiagnosticKind;
//!
//! let mut rules = RuleConfig::default();
//! rules.configure("ambiguous-column", RuleLevel::Warn).unwrap();
//! rules.configure("E0002", RuleLevel::Off).unwrap();
//! assert_eq!(rules.level(DiagnosticKind::AmbiguousColumn), RuleLevel::Warn);
//! assert_eq!(rules.level(DiagnosticKind::ColumnNotFound), RuleLevel::Off);
//! assert_eq!(rules.level(DiagnosticKind::TableNotFound), RuleLevel::Error);
//! ```

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{Diagnostic, DiagnosticKind, Severity};

/// Rule category: what kind of problem a rule finds. Decides the default level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleCategory {
    /// Code that is wrong: it fails or does something unintended (default: error)
    Correctness,
    /// Code that is most likely wrong (default: warn)
    Suspicious,
    /// Stricter checks that may have false positives (default: off)
    Pedantic,
    /// Conventions and readability (default: off)
    Style,
    /// Bans on features that are fine in general but unwanted in some codebases (default: off)
    Restriction,
}

impl RuleCategory {
    /// All categories
    pub const ALL: [RuleCategory; 5] = [
        RuleCategory::Correctness,
        RuleCategory::Suspicious,
        RuleCategory::Pedantic,
        RuleCategory::Style,
        RuleCategory::Restriction,
    ];

    /// Category name as used in configuration
    pub fn name(&self) -> &'static str {
        match self {
            RuleCategory::Correctness => "correctness",
            RuleCategory::Suspicious => "suspicious",
            RuleCategory::Pedantic => "pedantic",
            RuleCategory::Style => "style",
            RuleCategory::Restriction => "restriction",
        }
    }

    /// Level of the category's rules unless configured otherwise
    pub fn default_level(&self) -> RuleLevel {
        match self {
            RuleCategory::Correctness => RuleLevel::Error,
            RuleCategory::Suspicious => RuleLevel::Warn,
            RuleCategory::Pedantic | RuleCategory::Style | RuleCategory::Restriction => {
                RuleLevel::Off
            }
        }
    }
}

impl fmt::Display for RuleCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How a rule is reported
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleLevel {
    /// Not reported
    Off,
    /// Reported as a warning (doesn't fail `sqlsift check`)
    Warn,
    /// Reported as an error
    Error,
}

impl RuleLevel {
    /// Level name as used in configuration
    pub fn name(&self) -> &'static str {
        match self {
            RuleLevel::Off => "off",
            RuleLevel::Warn => "warn",
            RuleLevel::Error => "error",
        }
    }
}

impl fmt::Display for RuleLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for RuleLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "off" | "allow" => Ok(RuleLevel::Off),
            "warn" | "warning" => Ok(RuleLevel::Warn),
            "error" | "deny" => Ok(RuleLevel::Error),
            _ => Err(format!(
                "invalid rule level '{}' (expected 'off', 'warn' or 'error')",
                s
            )),
        }
    }
}

/// Metadata of a rule
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    /// Diagnostic kind the rule reports
    pub kind: DiagnosticKind,
    /// Stable code (e.g. `E0002`)
    pub code: &'static str,
    /// Name (e.g. `column-not-found`)
    pub name: &'static str,
    pub category: RuleCategory,
    /// One-line description
    pub summary: &'static str,
}

impl Rule {
    /// Level of the rule unless configured otherwise
    pub fn default_level(&self) -> RuleLevel {
        self.category.default_level()
    }
}

/// All rules, in the order of their [`DiagnosticKind`] variants
pub const RULES: &[Rule] = &[
    Rule {
        kind: DiagnosticKind::TableNotFound,
        code: "E0001",
        name: "table-not-found",
        category: RuleCategory::Correctness,
        summary: "Referenced table does not exist in schema",
    },
    Rule {
        kind: DiagnosticKind::ColumnNotFound,
        code: "E0002",
        name: "column-not-found",
        category: RuleCategory::Correctness,
        summary: "Referenced column does not exist in table",
    },
    Rule {
        kind: DiagnosticKind::TypeMismatch,
        code: "E0003",
        name: "type-mismatch",
        category: RuleCategory::Correctness,
        summary: "Type incompatibility in expression",
    },
    Rule {
        kind: DiagnosticKind::PotentialNullViolation,
        code: "E0004",
        name: "potential-null-violation",
        category: RuleCategory::Correctness,
        summary: "Potential NOT NULL violation",
    },
    Rule {
        kind: DiagnosticKind::ColumnCountMismatch,
        code: "E0005",
        name: "column-count-mismatch",
        category: RuleCategory::Correctness,
        summary: "INSERT column count doesn't match values",
    },
    Rule {
        kind: DiagnosticKind::AmbiguousColumn,
        code: "E0006",
        name: "ambiguous-column",
        category: RuleCategory::Correctness,
        summary: "Column reference is ambiguous across tables",
    },
    Rule {
        kind: DiagnosticKind::JoinTypeMismatch,
        code: "E0007",
        name: "join-type-mismatch",
        category: RuleCategory::Correctness,
        summary: "JOIN condition compares incompatible types",
    },
    Rule {
        kind: DiagnosticKind::MissingRequiredColumn,
        code: "E0008",
        name: "missing-required-column",
        category: RuleCategory::Correctness,
        summary: "INSERT omits a NOT NULL column without a default",
    },
    Rule {
        kind: DiagnosticKind::ParseError,
        code: "E1000",
        name: "parse-error",
        category: RuleCategory::Correctness,
        summary: "SQL could not be parsed",
    },
];

impl DiagnosticKind {
    /// The rule this kind of diagnostic belongs to
    pub fn rule(&self) -> &'static Rule {
        &RULES[*self as usize]
    }
}

/// Look up a rule by code (`E0002`) or name (`column-not-found`), ignoring case
pub fn find_rule(id: &str) -> Option<&'static Rule> {
    RULES
        .iter()
        .find(|r| r.code.eq_ignore_ascii_case(id) || r.name.eq_ignore_ascii_case(id))
}

/// Look up a category by name, ignoring case
pub fn find_category(name: &str) -> Option<RuleCategory> {
    RuleCategory::ALL
        .into_iter()
        .find(|c| c.name().eq_ignore_ascii_case(name))
}

/// Error for a rule setting that names no rule or category
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownRule(pub String);

impl fmt::Display for UnknownRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown rule or category '{}' (run `sqlsift rules` to list rules; categories: {})",
            self.0,
            RuleCategory::ALL.map(|c| c.name()).join(", ")
        )
    }
}

impl std::error::Error for UnknownRule {}

/// Configured levels of rules and categories
#[derive(Debug, Clone, Default)]
pub struct RuleConfig {
    categories: HashMap<RuleCategory, RuleLevel>,
    rules: HashMap<DiagnosticKind, RuleLevel>,
}

impl RuleConfig {
    /// Set the level of a rule (by code or name) or of a whole category
    pub fn configure(&mut self, id: &str, level: RuleLevel) -> Result<(), UnknownRule> {
        if let Some(rule) = find_rule(id) {
            self.rules.insert(rule.kind, level);
        } else if let Some(category) = find_category(id) {
            self.categories.insert(category, level);
        } else {
            return Err(UnknownRule(id.to_string()));
        }
        Ok(())
    }

    /// Effective level of a rule: its own setting, else its category's, else the default
    pub fn level(&self, kind: DiagnosticKind) -> RuleLevel {
        let rule = kind.rule();
        self.rules
            .get(&kind)
            .or_else(|| self.categories.get(&rule.category))
            .copied()
            .unwrap_or_else(|| rule.default_level())
    }

    /// Apply the configured levels: drop diagnostics of rules that are off and set the
    /// severity of the others
    pub fn apply(&self, diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
        diagnostics
            .into_iter()
            .filter_map(|mut d| {
                d.severity = match self.level(d.kind) {
                    RuleLevel::Off => return None,
                    RuleLevel::Warn => Severity::Warning,
                    RuleLevel::Error => Severity::Error,
                };
                Some(d)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_indexed_by_kind() {
        for (i, rule) in RULES.iter().enumerate() {
            assert_eq!(rule.kind as usize, i, "{} is out of order", rule.code);
            assert_eq!(rule.kind.rule().code, rule.code);
        }
    }

    #[test]
    fn codes_and_names_are_unique() {
        for (i, a) in RULES.iter().enumerate() {
            for b in &RULES[i + 1..] {
                assert_ne!(a.code, b.code);
                assert_ne!(a.name, b.name);
            }
            assert!(
                find_category(a.name).is_none(),
                "{} clashes with a category",
                a.name
            );
        }
    }

    #[test]
    fn rules_are_found_by_code_or_name() {
        assert_eq!(
            find_rule("E0006").map(|r| r.kind),
            Some(DiagnosticKind::AmbiguousColumn)
        );
        assert_eq!(
            find_rule("e0006").map(|r| r.kind),
            Some(DiagnosticKind::AmbiguousColumn)
        );
        assert_eq!(
            find_rule("Ambiguous-Column").map(|r| r.kind),
            Some(DiagnosticKind::AmbiguousColumn)
        );
        assert!(find_rule("E9999").is_none());
    }

    #[test]
    fn rule_setting_overrides_category_setting() {
        let mut config = RuleConfig::default();
        config.configure("correctness", RuleLevel::Warn).unwrap();
        config
            .configure("table-not-found", RuleLevel::Error)
            .unwrap();
        assert_eq!(
            config.level(DiagnosticKind::TableNotFound),
            RuleLevel::Error
        );
        assert_eq!(
            config.level(DiagnosticKind::ColumnNotFound),
            RuleLevel::Warn
        );
    }

    #[test]
    fn unknown_ids_are_rejected() {
        let mut config = RuleConfig::default();
        let err = config
            .configure("no-such-rule", RuleLevel::Off)
            .unwrap_err();
        assert!(err.to_string().contains("no-such-rule"));
    }

    #[test]
    fn levels_parse_from_config_strings() {
        assert_eq!("off".parse::<RuleLevel>(), Ok(RuleLevel::Off));
        assert_eq!("warning".parse::<RuleLevel>(), Ok(RuleLevel::Warn));
        assert_eq!("Error".parse::<RuleLevel>(), Ok(RuleLevel::Error));
        assert!("loud".parse::<RuleLevel>().is_err());
    }

    #[test]
    fn apply_drops_and_downgrades() {
        let mut config = RuleConfig::default();
        config.configure("E0001", RuleLevel::Off).unwrap();
        config.configure("E0002", RuleLevel::Warn).unwrap();
        let diagnostics = vec![
            Diagnostic::error(DiagnosticKind::TableNotFound, "t"),
            Diagnostic::error(DiagnosticKind::ColumnNotFound, "c"),
            Diagnostic::error(DiagnosticKind::TypeMismatch, "x"),
        ];
        let applied = config.apply(diagnostics);
        let summary: Vec<_> = applied.iter().map(|d| (d.kind, d.severity)).collect();
        assert_eq!(
            summary,
            [
                (DiagnosticKind::ColumnNotFound, Severity::Warning),
                (DiagnosticKind::TypeMismatch, Severity::Error),
            ]
        );
    }
}
