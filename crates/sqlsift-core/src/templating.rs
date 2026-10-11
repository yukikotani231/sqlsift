//! Template support for query files (dbt / Jinja)
//!
//! Query files of a dbt project are Jinja templates. With [`Templating::Jinja`],
//! `mask_jinja` rewrites the template syntax into SQL before parsing:
//!
//! - `{# comment #}` and `{% statement %}` become whitespace. The SQL inside a
//!   `{% for %}` block is checked once, as its first and last iteration (so
//!   `{% if not loop.last %},{% endif %}` and `{{ ',' if not loop.last }}` are
//!   dropped). Of an `{% if %}` / `{% elif %}` / `{% else %}` block only one
//!   branch is kept; the others are blanked. It is the first branch whose
//!   condition may be true: conditions sqlsift can evaluate are `is_incremental()`
//!   (false, the first build), `var('x', true|false)` (the default), comparisons
//!   of `target.type` with the dialect's dbt adapter, `{% set x = ... %}`
//!   variables with such a value, and `not` / `and` / `or` of these.
//! - The bodies of `{% set x %}...{% endset %}`, `{% call %}`, `{% macro %}`,
//!   `{% test %}`, `{% materialization %}` and `{% docs %}` blocks are blanked;
//!   the body of `{% raw %}...{% endraw %}` is SQL text.
//! - `{{ source('schema', 'table') }}` where a table name is expected becomes
//!   `schema.table` when the catalog has that table, and `{{ ref('model') }}`
//!   becomes `ref.model` when the catalog has the model from a dbt `catalog.json`
//!   (see [`crate::dbt`]). Names other than lowercase identifiers are quoted.
//! - Any other `{{ expression }}` where a table name is expected (after FROM, JOIN,
//!   INTO, UPDATE, TABLE or USING, e.g. `FROM {{ ref('orders') }}`) becomes an
//!   identifier made from its text (`___ref__orders____`). It names a table sqlsift
//!   doesn't know, so its columns are unknown: name diagnostics reported on it are
//!   dropped with `Preprocessed::is_substituted`, and columns that may come from it
//!   are not reported. The same goes for an alias (`AS {{ name }}`), a type
//!   (`x::{{ dbt.type_bigint() }}`) and an expression that is part of a name
//!   (`{{ prefix }}_total`, `t.{{ column }}`).
//!   `{{ var('x') }}` there becomes `var.x` when the catalog has the relation
//!   variable (see [`crate::dbt::RelationVars`]), and `{{ var('x', ref('m')) }}`
//!   its default relation. An alias built from tags (`AS {{ f }}_o` in a loop)
//!   makes qualifiers of that shape (`a_o.v`) unknown, and an output column with
//!   such an alias has an unknown name.
//! - `{{ expression }}` at the start of a statement, after comments
//!   (`{{ config(...) }}`), becomes whitespace. When the query follows it, the
//!   macro may write its WITH clause: its table names may be CTEs, and aren't
//!   reported.
//! - `{{ expression }}` right after a complete expression expands to more list
//!   items or a whole clause. In a SELECT list (`select a {{ source_relation() }}`)
//!   it becomes `, <identifier>.*`, a wildcard over a relation sqlsift doesn't
//!   know, so the columns of that query block are unknown; elsewhere
//!   (`{{ dbt_utils.group_by(2) }}`, `partition by a {{ m() }}`, a WHERE clause)
//!   it becomes whitespace.
//! - `{{ expression }}` that is all there is in the parentheses of a CTE or a
//!   derived table (`as ({{ dbt_utils.date_spine(...) }})`) becomes
//!   `select <identifier>.*`: a query whose columns are unknown.
//! - A string literal with an `{{ expression }}` in it (`'{{ var("x") }}'`) and any
//!   other `{{ expression }}` become the placeholder `$1`, an untyped value like a
//!   bind parameter.
//! - `sqlsift:disable` directives in `{# ... #}` comments work like `--` comments.
//!
//! Every character is replaced by one character and line breaks are kept, so
//! parser and diagnostic line/column locations still point at the original text.

use std::borrow::Cow;
use std::collections::HashMap;
use std::str::FromStr;

use crate::dbt;
use crate::dialect::SqlDialect;
use crate::psql::Preprocessed;

/// How query files are templated
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Templating {
    /// Plain SQL
    #[default]
    None,
    /// Jinja templates (dbt models): template tags are masked before analysis
    Jinja,
}

impl FromStr for Templating {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" | "off" => Ok(Templating::None),
            "jinja" | "dbt" => Ok(Templating::Jinja),
            _ => Err(format!(
                "Unknown templating: '{s}'. Supported values: jinja, none."
            )),
        }
    }
}

impl std::fmt::Display for Templating {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Templating::None => write!(f, "none"),
            Templating::Jinja => write!(f, "jinja"),
        }
    }
}

/// The dbt adapter (`target.type`) that runs models in `dialect`
pub(crate) fn dbt_adapter(dialect: SqlDialect) -> Option<&'static str> {
    match dialect {
        SqlDialect::PostgreSQL => Some("postgres"),
        SqlDialect::Snowflake => Some("snowflake"),
        SqlDialect::BigQuery => Some("bigquery"),
        SqlDialect::Redshift => Some("redshift"),
        SqlDialect::Databricks => Some("databricks"),
        SqlDialect::MySQL | SqlDialect::SQLite => None,
    }
}

/// Whether `sql` looks like a Jinja template (has `{{` or `{%`)
pub(crate) fn looks_like_jinja(sql: &str) -> bool {
    sql.contains("{{") || sql.contains("{%")
}

/// Keywords after which `{{ ... }}` names a table
const TABLE_KEYWORDS: &[&str] = &["from", "join", "into", "update", "table", "using"];

/// Blocks whose body is not SQL and is blanked (`{% macro %}...{% endmacro %}`). A
/// `{% set %}` block (without `=`) also is.
const NON_SQL_BLOCKS: &[&str] = &["macro", "call", "test", "materialization", "docs", "set"];

/// Keywords before a string literal whose text is part of a value
/// (`interval '{{ n }} days'`): such a string is kept a string
const TYPED_STRING_KEYWORDS: &[&str] = &["interval", "date", "time", "timestamp"];

/// The kind of a Jinja tag, from its opening delimiter
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tag {
    /// `{# ... #}`
    Comment,
    /// `{% ... %}`
    Statement,
    /// `{{ ... }}`
    Expression,
}

/// Byte index just past the tag that starts at `start` (the end of the input if
/// it isn't closed). Quoted strings inside statements and expressions may contain
/// the closing delimiter.
fn tag_end(sql: &str, start: usize, tag: Tag) -> usize {
    let bytes = sql.as_bytes();
    let close: &[u8] = match tag {
        Tag::Comment => b"#}",
        Tag::Statement => b"%}",
        Tag::Expression => b"}}",
    };
    let mut i = start + 2;
    while i < bytes.len() {
        let b = bytes[i];
        if bytes[i..].starts_with(close) {
            return i + 2;
        }
        if tag != Tag::Comment && matches!(b, b'\'' | b'"') {
            i += 1;
            while i < bytes.len() && bytes[i] != b {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
        }
        i += 1;
    }
    bytes.len()
}

/// The text of a tag between its delimiters, without whitespace control (`-`, `+`)
fn tag_content(text: &str) -> &str {
    let inner = text.get(2..).unwrap_or("");
    let closed = text.len() >= 4 && ["}}", "%}", "#}"].iter().any(|c| text.ends_with(c));
    let inner = if closed {
        &inner[..inner.len() - 2]
    } else {
        inner
    };
    inner
        .trim()
        .trim_start_matches(['-', '+'])
        .trim_end_matches(['-', '+'])
        .trim()
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Where an expression tag is, judged from the (already masked) text before it and
/// the text after it
enum Position {
    /// At the start of a statement: the tag expands to something other than a value
    Statement,
    /// Where a table name is expected (`FROM {{ ref('a') }}`)
    Table,
    /// Where another name is expected: an alias (`AS {{ name }}`), a type
    /// (`x::{{ dbt.type_bigint() }}`), or part of a name (`{{ prefix }}_total`,
    /// `t.{{ col }}`, `{{ target.schema }}.users`)
    Name,
    /// Right after a complete expression or clause (`select a {{ m() }} from t`,
    /// `from t {{ dbt_utils.group_by(2) }}`): the tag expands to more list items or
    /// to a whole clause
    Continuation,
    /// The whole body of a CTE or derived table (`as ({{ dbt_utils.date_spine() }})`)
    Query,
    /// Anywhere else: a value
    Value,
}

fn is_name_char(c: char) -> bool {
    is_ident_char(c) || matches!(c, '.' | '"')
}

/// The identifier word at the end of `text`
fn last_word(text: &str) -> &str {
    let start = text.rfind(|c: char| !is_ident_char(c)).map_or(0, |i| i + 1);
    &text[start..]
}

/// `statement_start`: only whitespace and comments since the start of the input or
/// the last `;`
fn position(before: &str, after: &str, statement_start: bool) -> Position {
    if statement_start {
        return Position::Statement;
    }
    let glued = before.ends_with(is_name_char) || after.starts_with(is_name_char);
    let before = without_trailing_comments(before);
    let word = last_word(before);
    if glued {
        Position::Name
    } else if TABLE_KEYWORDS.iter().any(|k| word.eq_ignore_ascii_case(k)) {
        Position::Table
    } else if word.eq_ignore_ascii_case("as") || before.ends_with("::") {
        Position::Name
    } else if ends_expression(before) && !starts_with_word(after, "as") {
        Position::Continuation
    } else if is_query_body(before, after) {
        Position::Query
    } else {
        Position::Value
    }
}

/// `text` without trailing whitespace and `--` / `/* */` comments
fn without_trailing_comments(text: &str) -> &str {
    let mut text = text.trim_end();
    loop {
        if text.ends_with("*/") {
            if let Some(open) = text.rfind("/*") {
                text = text[..open].trim_end();
                continue;
            }
        }
        let line_start = text.rfind('\n').map_or(0, |i| i + 1);
        let line = &text[line_start..];
        let code = without_line_comment(line);
        if code.len() == line.len() {
            return text;
        }
        text = text[..line_start + code.len()].trim_end();
    }
}

/// A line without its `--` comment (a `--` outside string literals and quoted
/// identifiers)
fn without_line_comment(line: &str) -> &str {
    let mut quote = None;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (None, '\'' | '"') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '-') if line[i + 1..].starts_with('-') => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Whether `text` starts (after whitespace) with the keyword `word`
fn starts_with_word(text: &str, word: &str) -> bool {
    let text = text.trim_start();
    text.get(..word.len())
        .is_some_and(|w| w.eq_ignore_ascii_case(word))
        && !text[word.len()..].starts_with(is_ident_char)
}

/// Whether a tag between `before` and `after` is all there is in the parentheses of
/// a CTE (`AS (...)`) or a derived table (`FROM (...)`)
fn is_query_body(before: &str, after: &str) -> bool {
    let Some(open) = before.strip_suffix('(') else {
        return false;
    };
    let word = last_word(open.trim_end());
    after.trim_start().starts_with(')')
        && ["as", "from", "join"]
            .iter()
            .any(|k| word.eq_ignore_ascii_case(k))
}

/// Keywords after which an expression or a name follows (a tag after one of them
/// is a value, not the continuation of a complete expression)
const OPERAND_KEYWORDS: &[&str] = &[
    "select",
    "distinct",
    "all",
    "where",
    "and",
    "or",
    "not",
    "on",
    "by",
    "when",
    "then",
    "else",
    "case",
    "in",
    "is",
    "as",
    "from",
    "join",
    "like",
    "ilike",
    "between",
    "having",
    "limit",
    "offset",
    "set",
    "values",
    "return",
    "returns",
    "with",
    "union",
    "except",
    "intersect",
    "minus",
    "interval",
    "over",
    "partition",
    "order",
    "group",
    "qualify",
    "using",
    "into",
    "cast",
    "exists",
    "any",
    "some",
    "top",
    "lateral",
    "table",
    "update",
    "delete",
    "insert",
    "escape",
    "similar",
    "to",
    "filter",
    "within",
    "at",
    "zone",
    "inner",
    "left",
    "right",
    "full",
    "outer",
    "cross",
    "natural",
    "window",
    "returning",
    "recursive",
    "materialized",
    "rows",
    "range",
    "groups",
    "unbounded",
    "current",
    "row",
    "following",
    "preceding",
    "date",
    "time",
    "timestamp",
    "array",
    "struct",
    "if",
];

/// Whether the (masked, trimmed) text ends with a complete expression: a name, a
/// literal, a closing parenthesis or quote, or the `*` of `SELECT *`
fn ends_expression(before: &str) -> bool {
    let Some(last) = before.chars().next_back() else {
        return false;
    };
    match last {
        ')' | '\'' | '"' | '`' | ']' => true,
        '*' => {
            let rest = before[..before.len() - 1].trim_end();
            rest.ends_with(',')
                || rest.ends_with('.')
                || ["select", "distinct"]
                    .iter()
                    .any(|k| last_word(rest).eq_ignore_ascii_case(k))
        }
        c if is_ident_char(c) => {
            let word = last_word(before);
            !OPERAND_KEYWORDS
                .iter()
                .any(|k| word.eq_ignore_ascii_case(k))
        }
        _ => false,
    }
}

/// Whether the end of the (masked) text is in a SELECT list: the nearest clause
/// keyword before it, outside parentheses, is SELECT
fn in_select_list(before: &str) -> bool {
    const CLAUSES: &[&str] = &[
        "from",
        "where",
        "by",
        "having",
        "qualify",
        "limit",
        "on",
        "using",
        "window",
        "union",
        "except",
        "intersect",
        "values",
        "set",
        "returning",
        "join",
    ];
    let is_clause = |word: &str| {
        if word.eq_ignore_ascii_case("select") {
            Some(true)
        } else {
            CLAUSES
                .iter()
                .any(|k| word.eq_ignore_ascii_case(k))
                .then_some(false)
        }
    };
    let mut depth = 0usize;
    for line in before.rsplit('\n') {
        let line = without_line_comment(line);
        let mut word_end = None;
        for (i, c) in line.char_indices().rev() {
            if is_ident_char(c) {
                word_end.get_or_insert(i + c.len_utf8());
                continue;
            }
            if let Some(end) = word_end.take() {
                if depth == 0 {
                    if let Some(select) = is_clause(&line[i + c.len_utf8()..end]) {
                        return select;
                    }
                }
            }
            match c {
                ')' => depth += 1,
                '(' if depth == 0 => return false,
                '(' => depth -= 1,
                _ => {}
            }
        }
        if let Some(end) = word_end {
            if depth == 0 {
                if let Some(select) = is_clause(&line[..end]) {
                    return select;
                }
            }
        }
    }
    false
}

/// A quoted name argument (`'customers'`), without its quotes
fn unquote_name(arg: &str) -> Option<&str> {
    let arg = arg.trim();
    let name = arg
        .strip_prefix('\'')
        .and_then(|a| a.strip_suffix('\''))
        .or_else(|| arg.strip_prefix('"').and_then(|a| a.strip_suffix('"')))?;
    let valid = name.chars().all(is_ident_char)
        && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_');
    valid.then_some(name)
}

/// `source('schema', 'table')`: the schema and table names
fn source_call(content: &str) -> Option<(&str, &str)> {
    let args = content
        .strip_prefix("source")?
        .trim_start()
        .strip_prefix('(')?
        .strip_suffix(')')?;
    let (schema, table) = args.split_once(',')?;
    Some((unquote_name(schema)?, unquote_name(table)?))
}

/// `ref('model')`: the model name (`None` for a package-qualified or versioned
/// ref, `ref('package', 'model')`, `ref('model', v=2)`)
fn ref_call(content: &str) -> Option<&str> {
    let arg = content
        .strip_prefix("ref")?
        .trim_start()
        .strip_prefix('(')?
        .strip_suffix(')')?;
    unquote_name(arg)
}

/// The relation of `var('x')`: the relation variable `var.x` (see
/// [`dbt::RelationVars`]), or a default that is a relation call
/// (`var('x', ref('y'))`), as (schema, table)
fn var_relation(content: &str) -> Option<(&str, &str)> {
    let args = content
        .strip_prefix("var")?
        .trim_start()
        .strip_prefix('(')?
        .strip_suffix(')')?;
    let args = split_top_level(args, ",");
    let name = unquote_name(args.first()?)?;
    match args.get(1).map(|a| a.trim()) {
        Some(default) => relation_call(default),
        None => Some((dbt::VAR_SCHEMA, name)),
    }
}

/// The (schema, table) of a `source('s', 't')` / `ref('m')` call
pub(crate) fn relation_call(content: &str) -> Option<(&str, &str)> {
    source_call(content).or_else(|| ref_call(content).map(|model| (dbt::REF_SCHEMA, model)))
}

/// A name part as SQL: as is when it is a lowercase identifier (which every
/// dialect reads as itself), else double-quoted
fn sql_name_part(name: &str) -> Cow<'_, str> {
    if name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        Cow::Borrowed(name)
    } else {
        Cow::Owned(format!("\"{name}\""))
    }
}

/// A loop separator like `{{ ',' if not loop.last }}`
fn is_loop_separator(content: &str) -> bool {
    let Some(quote) = content.chars().next().filter(|c| matches!(c, '\'' | '"')) else {
        return false;
    };
    let Some((literal, rest)) = content[1..].split_once(quote) else {
        return false;
    };
    literal.chars().all(|c| c == ',' || c.is_whitespace())
        && rest.trim_start().starts_with("if ")
        && rest.contains("loop.")
}

/// The value of a branch condition when it is known: the body of a `{% for %}` loop
/// is checked once, as its first and last iteration
/// is checked once, as its first and last iteration. Also known: `is_incremental()`
/// (false: the model's first, full build), `var('x', <true|false>)` (its default)
/// and comparisons of `target.type` with the dbt adapter of the dialect.
fn branch_condition(condition: &str, known: &Known) -> Option<bool> {
    let condition = condition.trim();
    // `a or b`, `a and b` (outside parentheses and strings)
    for (op, short_circuit) in [(" or ", true), (" and ", false)] {
        let parts = split_top_level(condition, op);
        if parts.len() > 1 {
            let values: Vec<Option<bool>> =
                parts.iter().map(|p| branch_condition(p, known)).collect();
            if values.contains(&Some(short_circuit)) {
                return Some(short_circuit);
            }
            return values.iter().all(Option::is_some).then_some(!short_circuit);
        }
    }
    if let Some(inner) = condition
        .strip_prefix('(')
        .and_then(|c| c.strip_suffix(')'))
        .filter(|c| {
            // `(a) == (b)` isn't one parenthesized condition
            let mut depth = 0i32;
            c.chars().all(|ch| {
                depth += match ch {
                    '(' => 1,
                    ')' => -1,
                    _ => 0,
                };
                depth >= 0
            })
        })
    {
        return branch_condition(inner, known);
    }
    if let Some(rest) = condition.strip_prefix("not ") {
        if !rest.trim_start().starts_with("in ") {
            return branch_condition(rest, known).map(|v| !v);
        }
    }
    let compact: String = condition.split_whitespace().collect();
    match compact.as_str() {
        "loop.last" | "loop.first" => return Some(true),
        "is_incremental()" => return Some(false),
        _ => {}
    }
    if let Some(args) = compact
        .strip_prefix("var(")
        .and_then(|c| c.strip_suffix(')'))
    {
        let (_, default) = args.rsplit_once(',')?;
        return match default {
            "true" | "True" => Some(true),
            "false" | "False" => Some(false),
            _ => None,
        };
    }
    if compact.chars().all(is_ident_char) {
        return known.sets.get(&compact).copied();
    }
    target_condition(condition, known.target?)
}

/// What is known when evaluating a branch condition
struct Known<'k> {
    /// The dbt adapter (`target.type`)
    target: Option<&'k str>,
    /// Variables set to a known value (`{% set enabled = var('x', false) %}`)
    sets: &'k HashMap<String, bool>,
}

/// `target.type == 'x'`, `!=`, `in [...]`, `not in [...]`, with `target` the dbt
/// adapter of the dialect
fn target_condition(condition: &str, target: &str) -> Option<bool> {
    const ADAPTERS: &[&str] = &[
        "postgres",
        "redshift",
        "snowflake",
        "bigquery",
        "databricks",
        "spark",
        "mysql",
        "sqlite",
        "duckdb",
        "sqlserver",
        "synapse",
        "fabric",
        "trino",
        "athena",
        "oracle",
    ];
    let rest = condition.strip_prefix("target.type")?.trim_start();
    let (negate, rest) = if let Some(r) = rest.strip_prefix("==") {
        (false, r)
    } else if let Some(r) = rest.strip_prefix("!=") {
        (true, r)
    } else if let Some(r) = rest.strip_prefix("not in") {
        (true, r)
    } else {
        (false, rest.strip_prefix("in")?)
    };
    let names: Vec<&str> = rest
        .trim()
        .trim_start_matches(['[', '('])
        .trim_end_matches([']', ')'])
        .split(',')
        .map(|n| n.trim().trim_matches(['\'', '"']))
        .collect();
    // Only names of adapters (anything else may be a variable)
    if names.iter().any(|n| !ADAPTERS.contains(n)) {
        return None;
    }
    // Databricks models may also run on the spark adapter
    if target == "databricks" && names.contains(&"spark") {
        return None;
    }
    Some(names.contains(&target) != negate)
}

/// `text` split at `separator` where it is outside parentheses, brackets and strings
fn split_top_level<'t>(text: &'t str, separator: &str) -> Vec<&'t str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut quote = None;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')' | ']' | '}') => depth -= 1,
            (None, _) if depth == 0 && i >= start && text[i..].starts_with(separator) => {
                parts.push(&text[start..i]);
                start = i + separator.len();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// An open `{% ... %}` block
enum Frame {
    /// `{% if %}`: whether the current branch is kept, and whether any branch was
    If { active: bool, taken: bool },
    /// `{% for %}`: whether in its `{% else %}` branch (blanked)
    For { in_else: bool },
    /// A block whose body is blanked, ended by `{% end<keyword> %}`
    Blank(String),
}

/// Where the text being copied is, lexically
#[derive(Clone, Copy)]
enum Lex {
    Code,
    /// In a `'...'` string starting at this byte of the output, which contains a
    /// template expression when `templated`
    String {
        start: usize,
        templated: bool,
    },
    QuotedIdentifier,
    LineComment,
    BlockComment,
}

/// A template tag that became a value or a name, for parse error messages
struct TagSpan {
    line: usize,
    start: usize,
    end: usize,
    text: String,
}

/// A query file with its Jinja template syntax masked (see the module docs)
pub(crate) struct Template<'a> {
    /// The masked text and its substituted identifiers
    pub masked: Preprocessed<'a>,
    /// `sqlsift:` directives in `{# ... #}` comments: (line, directive)
    directives: Vec<(usize, String)>,
    /// Tags that became values or names
    tags: Vec<TagSpan>,
    /// Aliases built from tags (`AS {{ f }}_o`): the text before and after the tags
    alias_patterns: Vec<(String, String)>,
    /// Statements whose tables may be CTEs a macro writes: (line, column) ranges
    open_statements: Vec<((usize, usize), (usize, usize))>,
}

impl<'a> Template<'a> {
    /// A file without templating
    pub fn unchanged(sql: &'a str) -> Self {
        Template {
            masked: Preprocessed::unchanged(sql),
            directives: Vec::new(),
            tags: Vec::new(),
            alias_patterns: Vec::new(),
            open_statements: Vec::new(),
        }
    }

    /// Whether `line`/`column` is in a query that follows a macro which may write its
    /// WITH clause (`{{ recursive_dag() }}\nselect * from all_relationships`): a
    /// table name there may be one of its CTEs
    pub fn may_be_templated_cte(&self, line: usize, column: usize) -> bool {
        self.open_statements
            .iter()
            .any(|&(start, end)| start <= (line, column) && (line, column) <= end)
    }

    /// Whether `name` may be an alias built from template tags (`AS {{ f }}_o`
    /// in a `{% for %}` loop defines `a_o`, `b_o`, ...)
    pub fn may_be_templated_alias(&self, name: &str) -> bool {
        let name = name.to_ascii_lowercase();
        self.alias_patterns.iter().any(|(prefix, suffix)| {
            name.len() > prefix.len() + suffix.len()
                && name.starts_with(prefix.as_str())
                && name.ends_with(suffix.as_str())
        })
    }

    /// The text to read `sqlsift:disable` directives from: the masked text (so lines
    /// holding only template tags are blank), with the directives of `{# ... #}`
    /// comments as `--` comments
    pub fn directive_text(&self) -> Cow<'_, str> {
        if self.directives.is_empty() {
            return Cow::Borrowed(&self.masked.text);
        }
        let lines: Vec<String> = self
            .masked
            .text
            .split('\n')
            .enumerate()
            .map(|(i, line)| {
                let mut line = line.trim_end_matches('\r').to_string();
                for (_, directive) in self.directives.iter().filter(|(l, _)| *l == i + 1) {
                    line.push_str(" -- ");
                    line.push_str(directive);
                }
                line
            })
            .collect();
        Cow::Owned(lines.join("\n"))
    }

    /// The original text (first line) of the template tag at `line`/`column`, if one
    /// became a value or a name there
    pub fn tag_at(&self, line: usize, column: usize) -> Option<&str> {
        self.tags
            .iter()
            .find(|t| t.line == line && (t.start..t.end).contains(&column))
            .map(|t| t.text.as_str())
    }
}

/// State of [`mask_jinja`]
struct Masker<'s, 'f> {
    sql: &'s str,
    table_exists: &'f dyn Fn(&str, &str) -> bool,
    /// The dbt adapter (`target.type`) of the dialect, if known
    target: Option<&'static str>,
    /// `{% set %}` variables whose value is known
    sets: HashMap<String, bool>,
    out: String,
    identifiers: Vec<(usize, usize, usize)>,
    directives: Vec<(usize, String)>,
    tags: Vec<TagSpan>,
    alias_patterns: Vec<(String, String)>,
    /// Output byte offsets of macros that may write the WITH clause of the query
    /// that follows
    open_statements: Vec<usize>,
    frames: Vec<Frame>,
    lex: Lex,
    /// The previous character was the first quote of a doubled quote (`''`) in a
    /// string
    quote_pending: bool,
    /// Only whitespace, comments and blanked tags since the start or the last `;`
    statement_start: bool,
    /// Line and character column of the next character
    line: usize,
    column: usize,
}

impl Masker<'_, '_> {
    /// Whether the text at this point is part of the SQL: outside blanked blocks and
    /// branches
    fn active(&self) -> bool {
        self.frames.iter().all(|frame| match frame {
            Frame::If { active, .. } => *active,
            Frame::For { in_else } => !in_else,
            Frame::Blank(_) => false,
        })
    }

    /// Move the line/column past `text`
    fn advance(&mut self, text: &str) {
        for c in text.chars() {
            if c == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
        }
    }

    /// Copy the SQL text `sql[range]` (blanked when not [`Self::active`])
    fn copy(&mut self, range: std::ops::Range<usize>) {
        let sql = self.sql;
        if !self.active() {
            blank(&mut self.out, &sql[range.clone()]);
            self.advance(&sql[range]);
            return;
        }
        let mut prev = '\0';
        for (i, c) in sql[range.clone()].char_indices() {
            let at = range.start + i;
            let next = sql[at + c.len_utf8()..].chars().next();
            let mut closed_string = None;
            // Code other than whitespace and the start of a comment
            let starts_comment = matches!((c, next), ('-', Some('-')) | ('/', Some('*')));
            if matches!(self.lex, Lex::Code) && !c.is_whitespace() && !starts_comment {
                self.statement_start = c == ';';
            }
            self.lex = match self.lex {
                Lex::Code => match c {
                    '\'' => Lex::String {
                        start: self.out.len(),
                        templated: false,
                    },
                    '"' => Lex::QuotedIdentifier,
                    '-' if next == Some('-') => Lex::LineComment,
                    '/' if next == Some('*') => Lex::BlockComment,
                    _ => Lex::Code,
                },
                Lex::String { start, templated } if c == '\'' => {
                    if self.quote_pending {
                        // The second quote of `''`
                        self.quote_pending = false;
                        Lex::String { start, templated }
                    } else if next == Some('\'') {
                        self.quote_pending = true;
                        Lex::String { start, templated }
                    } else {
                        closed_string = templated.then_some(start);
                        Lex::Code
                    }
                }
                Lex::QuotedIdentifier if c == '"' => Lex::Code,
                Lex::LineComment if c == '\n' => Lex::Code,
                Lex::BlockComment if c == '/' && prev == '*' => Lex::Code,
                lex => lex,
            };
            self.out.push(c);
            self.advance(&sql[at..at + c.len_utf8()]);
            if let Some(start) = closed_string {
                self.untype_string(start);
            }
            prev = c;
        }
    }

    /// A string literal (from byte `start` of the output to its end) with a template
    /// expression in it renders to any literal (`'{{ var("x") }}'`): make it the
    /// untyped placeholder `$1`, unless it is part of a typed literal
    /// (`interval '{{ n }} days'`)
    fn untype_string(&mut self, start: usize) {
        let keyword = last_word(self.out[..start].trim_end());
        if TYPED_STRING_KEYWORDS
            .iter()
            .any(|k| keyword.eq_ignore_ascii_case(k))
        {
            return;
        }
        let literal = self.out.split_off(start);
        self.out.push_str("$1");
        blank(&mut self.out, &literal[2..]);
    }

    /// Handle a `{% ... %}` tag (blanked), updating the open blocks. Returns the end
    /// of the text it covers (past `{% endraw %}` for `{% raw %}`).
    /// What is known when evaluating a branch condition here
    fn known(&self) -> Known<'_> {
        Known {
            target: self.target,
            sets: &self.sets,
        }
    }

    fn statement(&mut self, start: usize, end: usize) -> usize {
        let text = &self.sql[start..end];
        let content = tag_content(text);
        let keyword_end = content
            .find(|c: char| !is_ident_char(c))
            .unwrap_or(content.len());
        let (keyword, args) = content.split_at(keyword_end);
        let keyword = keyword.to_ascii_lowercase();
        blank(&mut self.out, text);
        self.advance(text);
        match keyword.as_str() {
            "if" => {
                let active = branch_condition(args, &self.known()) != Some(false);
                self.frames.push(Frame::If {
                    active,
                    taken: active,
                });
            }
            "elif" => {
                let possible = branch_condition(args, &self.known()) != Some(false);
                if let Some(Frame::If { active, taken }) = self.frames.last_mut() {
                    *active = !*taken && possible;
                    *taken |= *active;
                }
            }
            "else" => match self.frames.last_mut() {
                Some(Frame::If { active, taken }) => {
                    *active = !*taken;
                    *taken = true;
                }
                Some(Frame::For { in_else }) => *in_else = true,
                _ => {}
            },
            "for" => self.frames.push(Frame::For { in_else: false }),
            "endif" | "endfor" => {
                let top_matches = match self.frames.last() {
                    Some(Frame::If { .. }) => keyword == "endif",
                    Some(Frame::For { .. }) => keyword == "endfor",
                    _ => false,
                };
                if top_matches {
                    self.frames.pop();
                }
            }
            "raw" => {
                // The body is SQL text, with no tags in it
                let (body_end, raw_end) = find_endraw(self.sql, end);
                self.copy(end..body_end);
                let end_tag = &self.sql[body_end..raw_end];
                blank(&mut self.out, end_tag);
                self.advance(end_tag);
                return raw_end;
            }
            "set" if args.contains('=') => {
                // `{% set name = value %}`: remember a value a branch may test
                if let Some((name, value)) = args.split_once('=') {
                    let name = name.trim();
                    if !value.starts_with('=') && name.chars().all(is_ident_char) {
                        let value = branch_condition(value, &self.known());
                        match value {
                            Some(v) if self.active() => {
                                self.sets.insert(name.to_string(), v);
                            }
                            _ => {
                                self.sets.remove(name);
                            }
                        }
                    }
                }
            }
            kw if NON_SQL_BLOCKS.contains(&kw) => {
                self.frames.push(Frame::Blank(format!("end{kw}")));
            }
            kw => {
                if matches!(self.frames.last(), Some(Frame::Blank(end)) if end == kw) {
                    self.frames.pop();
                }
            }
        }
        end
    }

    /// Write the tag `text` as `<prefix><identifier>.*`, a wildcard over a relation
    /// sqlsift doesn't know (so the columns of its query block are unknown), on the
    /// first line of the tag long enough to hold it; the rest is blanked. Writes
    /// nothing and returns `false` when no line is long enough.
    fn open_wildcard(&mut self, text: &str, prefix: &str) -> bool {
        let needed = prefix.len() + 5;
        let Some(fit) = text.split('\n').position(|l| l.chars().count() >= needed) else {
            return false;
        };
        for (i, segment) in text.split('\n').enumerate() {
            if i > 0 {
                self.out.push('\n');
            }
            if i != fit {
                blank(&mut self.out, segment);
                continue;
            }
            let width = segment.chars().count();
            let line = self.line + i;
            let column = if i == 0 { self.column } else { 1 };
            self.out.push_str(prefix);
            self.out.push('_');
            self.out.extend(
                segment
                    .chars()
                    .skip(prefix.len() + 1)
                    .take(width - prefix.len() - 3)
                    .map(|c| if is_ident_char(c) { c } else { '_' }),
            );
            self.out.push_str(".*");
            self.identifiers
                .push((line, column + prefix.len(), column + width - 2));
        }
        true
    }

    /// Handle a `{{ ... }}` tag
    fn expression(&mut self, start: usize, end: usize) {
        let text = &self.sql[start..end];
        let content = tag_content(text);
        let position = if !self.active() || is_loop_separator(content) {
            Position::Statement
        } else {
            match &mut self.lex {
                Lex::String { templated, .. } => {
                    // The whole string literal becomes a value
                    *templated = true;
                    Position::Statement
                }
                Lex::Code => position(&self.out, &self.sql[end..], self.statement_start),
                _ => Position::Statement,
            }
        };
        let first_line = text.find('\n').map_or(text, |n| &text[..n]);
        let single_line = first_line.len() == text.len();
        let column = self.column;
        let width = first_line.chars().count();
        // The tag's first line as identifier characters (`{{ ref('a') }}` →
        // `___ref__a____`)
        let ident: String = first_line
            .chars()
            .map(|c| if is_ident_char(c) { c } else { '_' })
            .collect();
        let position = match position {
            // More SELECT list items: `, <tag>.*`, a wildcard over a relation sqlsift
            // doesn't know, so the query block's output columns are unknown. Any
            // other clause (`group by 1, 2`, `where ...`, `, column` in PARTITION
            // BY) is blanked.
            Position::Continuation
                if in_select_list(&self.out) && self.open_wildcard(text, ",") =>
            {
                Position::Continuation
            }
            Position::Continuation => {
                blank(&mut self.out, text);
                Position::Continuation
            }
            // A query whose output columns are unknown: `select <tag>.*`
            Position::Query if self.open_wildcard(text, "select ") => Position::Query,
            Position::Query => Position::Value,
            position => position,
        };
        if !matches!(position, Position::Statement) {
            self.statement_start = false;
        } else if self.statement_start
            && self.active()
            && !content.starts_with("config")
            && starts_with_word(&self.sql[end..], "select")
        {
            // A macro that writes the WITH clause of the query that follows
            // (`{{ recursive_dag() }}\nselect * from all_relationships`): the tables
            // of that query may be its CTEs
            self.open_statements.push(self.out.len());
        }
        // A relation the catalog has: `{{ source('raw', 'customers') }}` →
        // `raw.customers`, `{{ ref('orders') }}` → `ref.orders` (dbt catalog)
        let relation = if single_line && matches!(position, Position::Table) {
            relation_call(content)
                .or_else(|| var_relation(content))
                .filter(|&(schema, table)| (self.table_exists)(schema, table))
                .map(|(schema, table)| {
                    format!("{}.{}", sql_name_part(schema), sql_name_part(table))
                })
                .filter(|name| name.len() <= text.chars().count())
        } else {
            None
        };
        match (position, relation) {
            (Position::Table, Some(name)) => {
                let padding = text.chars().count() - name.len();
                self.out.push_str(&name);
                self.out.push_str(&" ".repeat(padding));
            }
            (Position::Table | Position::Name, _) => {
                // An identifier made from the tag's first line (`{{ ref('a') }}` →
                // `___ref__a____`), including the name it is part of
                // (`{{ target.schema }}.users`, `total_{{ c }}`)
                let glued_before = self
                    .out
                    .chars()
                    .rev()
                    .take_while(|&c| is_name_char(c))
                    .count();
                let start_column = column - glued_before;
                let glued_after: String = if single_line {
                    self.sql[end..]
                        .chars()
                        .take_while(|&c| is_name_char(c))
                        .collect()
                } else {
                    String::new()
                };
                // An alias built from the tag (`AS {{ f }}_o`): names of that shape
                // may be aliases of this file
                let glued_start = self
                    .out
                    .char_indices()
                    .rev()
                    .take(glued_before)
                    .last()
                    .map_or(self.out.len(), |(i, _)| i);
                let (out_before, glued_text) = self.out.split_at(glued_start);
                if last_word(out_before.trim_end()).eq_ignore_ascii_case("as") {
                    self.alias_patterns.push((
                        glued_text.to_ascii_lowercase(),
                        glued_after.to_ascii_lowercase(),
                    ));
                }
                self.out.push_str(&ident);
                let end_column = column + width + glued_after.chars().count();
                self.identifiers.push((self.line, start_column, end_column));
                self.tags.push(TagSpan {
                    line: self.line,
                    start: start_column,
                    end: end_column,
                    text: first_line.to_string(),
                });
                blank(&mut self.out, &text[first_line.len()..]);
            }
            (Position::Value, _) => {
                // `$1` padded with spaces (`{{` is two characters)
                self.out.push_str("$1");
                blank(&mut self.out, &text[2..]);
                self.tags.push(TagSpan {
                    line: self.line,
                    start: column,
                    end: column + first_line.chars().count(),
                    text: first_line.to_string(),
                });
            }
            (Position::Continuation | Position::Query, _) => {
                // The tag is written above
                self.tags.push(TagSpan {
                    line: self.line,
                    start: column,
                    end: column + width,
                    text: first_line.to_string(),
                });
            }
            (Position::Statement, _) => blank(&mut self.out, text),
        }
        self.advance(text);
    }

    /// Handle a `{# ... #}` tag (blanked); a `sqlsift:` directive in it is recorded
    fn comment(&mut self, start: usize, end: usize) {
        let text = &self.sql[start..end];
        let content = tag_content(text);
        if content.starts_with("sqlsift:") && !content.contains('\n') {
            self.directives.push((self.line, content.to_string()));
        }
        blank(&mut self.out, text);
        self.advance(text);
    }
}

/// Start and end of the `{% endraw %}` tag after byte `from` (the end of the input
/// if there is none)
fn find_endraw(sql: &str, from: usize) -> (usize, usize) {
    let mut i = from;
    while let Some(offset) = sql[i..].find("{%") {
        let start = i + offset;
        let end = tag_end(sql, start, Tag::Statement);
        if tag_content(&sql[start..end]).eq_ignore_ascii_case("endraw") {
            return (start, end);
        }
        i = start + 2;
    }
    (sql.len(), sql.len())
}

/// Mask Jinja template syntax (see the module docs). `table_exists(schema, table)`
/// tells whether the catalog has a table for `{{ source(schema, table) }}` (and,
/// with schema [`dbt::REF_SCHEMA`], for `{{ ref(table) }}`).
pub(crate) fn mask_jinja<'a>(
    sql: &'a str,
    table_exists: &dyn Fn(&str, &str) -> bool,
    target: Option<&'static str>,
) -> Template<'a> {
    if !sql.contains("{{") && !sql.contains("{%") && !sql.contains("{#") {
        return Template::unchanged(sql);
    }

    let mut masker = Masker {
        sql,
        table_exists,
        target,
        sets: HashMap::new(),
        out: String::with_capacity(sql.len()),
        identifiers: Vec::new(),
        directives: Vec::new(),
        tags: Vec::new(),
        alias_patterns: Vec::new(),
        open_statements: Vec::new(),
        frames: Vec::new(),
        lex: Lex::Code,
        quote_pending: false,
        statement_start: true,
        line: 1,
        column: 1,
    };
    let mut rest = 0;

    while let Some(offset) = sql[rest..].find('{') {
        let start = rest + offset;
        let tag = match sql.as_bytes().get(start + 1) {
            Some(b'#') => Some(Tag::Comment),
            Some(b'%') => Some(Tag::Statement),
            Some(b'{') => Some(Tag::Expression),
            _ => None,
        };
        // Text before the tag (or up to and including a lone `{`) is copied as is
        let copy_end = if tag.is_some() { start } else { start + 1 };
        masker.copy(rest..copy_end);
        rest = copy_end;
        let Some(tag) = tag else {
            continue;
        };

        let end = tag_end(sql, start, tag);
        rest = match tag {
            Tag::Comment => {
                masker.comment(start, end);
                end
            }
            Tag::Statement => masker.statement(start, end),
            Tag::Expression => {
                masker.expression(start, end);
                end
            }
        };
    }
    masker.copy(rest..sql.len());

    // Each open statement ends at the next `;`
    let line_column = |offset: usize| {
        let before = &masker.out[..offset];
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        (
            before.matches('\n').count() + 1,
            before[line_start..].chars().count() + 1,
        )
    };
    let open_statements = masker
        .open_statements
        .iter()
        .map(|&start| {
            let end = masker.out[start..]
                .find(';')
                .map_or(masker.out.len(), |i| start + i);
            (line_column(start), line_column(end))
        })
        .collect();

    Template {
        masked: Preprocessed::new(Cow::Owned(masker.out), masker.identifiers),
        directives: masker.directives,
        tags: masker.tags,
        alias_patterns: masker.alias_patterns,
        open_statements,
    }
}

/// Append `text` with every character but line breaks replaced by a space
fn blank(out: &mut String, text: &str) {
    out.extend(
        text.chars()
            .map(|c| if matches!(c, '\n' | '\r') { c } else { ' ' }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(sql: &str) -> Template<'_> {
        mask_jinja(
            sql,
            &|schema, table| {
                matches!(
                    (schema, table),
                    ("raw", "customers") | ("ref", "orders" | "Orders" | "a_very_long_model_name")
                )
            },
            Some("postgres"),
        )
    }

    fn mask(sql: &str) -> String {
        let result = template(sql).masked.text.into_owned();
        assert_eq!(
            result.chars().count(),
            sql.chars().count(),
            "{sql:?} -> {result:?}"
        );
        assert_eq!(
            result.lines().count(),
            sql.lines().count(),
            "{sql:?} -> {result:?}"
        );
        result
    }

    #[test]
    fn parses_templating_values() {
        assert_eq!("jinja".parse(), Ok(Templating::Jinja));
        assert_eq!("dbt".parse(), Ok(Templating::Jinja));
        assert_eq!("None".parse(), Ok(Templating::None));
        assert!("mustache".parse::<Templating>().is_err());
        assert_eq!(Templating::Jinja.to_string(), "jinja");
    }

    #[test]
    fn blanks_comments_and_statements() {
        assert_eq!(mask("{# c #}SELECT 1"), "       SELECT 1");
        assert_eq!(
            mask("SELECT 1 {% if x %}\nWHERE a{%- endif -%}"),
            "SELECT 1           \nWHERE a             "
        );
        assert_eq!(mask("{#\nmulti\n#}\nSELECT 1"), "  \n     \n  \nSELECT 1");
    }

    #[test]
    fn expressions_become_placeholders() {
        assert_eq!(mask("SELECT {{ x }} AS a"), "SELECT $1      AS a");
        assert_eq!(
            mask("WHERE d > '{{ var(\"start\") }}'"),
            "WHERE d > $1                  "
        );
    }

    #[test]
    fn strings_with_expressions_are_values() {
        assert_eq!(mask("WHERE a = 'x{{ y }}z'"), "WHERE a = $1         ");
        // Doubled quotes, and strings without expressions, are kept
        assert_eq!(
            mask("WHERE a = 'it''s {{ y }}' OR b = 'c'"),
            "WHERE a = $1              OR b = 'c'"
        );
        // Typed literals stay strings
        assert_eq!(
            mask("WHERE d > now() - interval '{{ n }} days'"),
            "WHERE d > now() - interval '        days'"
        );
        // Quotes in comments and quoted identifiers don't start strings
        assert_eq!(
            mask("SELECT \"it's\", {{ x }} -- don't\nFROM t"),
            "SELECT \"it's\", $1      -- don't\nFROM t"
        );
    }

    #[test]
    fn expressions_in_table_position_become_identifiers() {
        assert_eq!(
            mask("SELECT * FROM {{ ref('a') }} JOIN {{ source('s','t') }} t"),
            "SELECT * FROM ___ref__a_____ JOIN ___source__s___t_____ t"
        );
    }

    #[test]
    fn known_sources_become_schema_tables() {
        assert_eq!(
            mask("SELECT * FROM {{ source('raw', 'customers') }} c"),
            "SELECT * FROM raw.customers                    c"
        );
        assert!(!template("SELECT * FROM {{ source('raw', 'customers') }}")
            .masked
            .is_substituted(1, 15));
    }

    #[test]
    fn known_refs_become_ref_tables() {
        assert_eq!(
            mask("SELECT * FROM {{ ref('orders') }} o"),
            "SELECT * FROM ref.orders          o"
        );
        assert_eq!(
            mask("SELECT * FROM {{ref('Orders')}}"),
            "SELECT * FROM ref.\"Orders\"     "
        );
        assert!(!template("SELECT * FROM {{ ref('orders') }}")
            .masked
            .is_substituted(1, 15));
        // Unknown, package-qualified and versioned refs stay unknown tables
        assert_eq!(mask("FROM {{ ref('x') }}"), "FROM ___ref__x_____");
        assert_eq!(
            mask("FROM {{ ref('p', 'orders') }}"),
            "FROM ___ref__p____orders_____"
        );
        assert_eq!(
            mask("FROM {{ ref('orders', v=2) }}"),
            "FROM ___ref__orders___v_2____"
        );
        // A ref() that isn't a table name
        assert_eq!(
            mask("SELECT '{{ ref(\"orders\") }}'"),
            "SELECT $1                   "
        );
    }

    #[test]
    fn expressions_in_names_become_identifiers() {
        assert_eq!(
            mask("SELECT 1 AS {{ c }}, total_{{ c }}, {{ c }}_x, t.{{ c }} FROM t"),
            "SELECT 1 AS ___c___, total____c___, ___c____x, t.___c___ FROM t"
        );
        assert_eq!(
            mask("SELECT id::{{ dbt.type_bigint() }} FROM t"),
            "SELECT id::___dbt_type_bigint_____ FROM t"
        );
        let p = template("SELECT total_{{ c }}_x FROM t");
        assert!(p.masked.is_substituted(1, 8));
        assert!(p.masked.is_substituted(1, 22));
        assert!(!p.masked.is_substituted(1, 24));
    }

    #[test]
    fn expression_at_statement_start_is_blanked() {
        assert_eq!(
            mask("{{ config(materialized='table') }}\nSELECT 1"),
            "                                  \nSELECT 1"
        );
        assert_eq!(mask("SELECT 1;\n{{ x }}"), "SELECT 1;\n       ");
    }

    #[test]
    fn only_the_first_branch_is_kept() {
        assert_eq!(
            mask("SELECT {% if a %}x{% elif b %}y{% else %}z{% endif %} FROM t"),
            "SELECT           x                                    FROM t"
        );
        // Nested blocks in a blanked branch stay blanked
        assert_eq!(
            mask("{% if a %}x{% else %}{% if b %}y{% endif %}{% endif %}"),
            "          x                                           "
        );
    }

    #[test]
    fn loops_are_checked_as_their_last_iteration() {
        assert_eq!(
            mask("SELECT {% for c in cs %}{{ c }}{% if not loop.last %},{% endif %}{% endfor %}"),
            "SELECT                  $1                                                   "
        );
        assert_eq!(
            mask("{% for c in cs %}x{% if loop.last %};{% else %},{% endif %}{% endfor %}"),
            "                 x                  ;                                  "
        );
        assert_eq!(
            mask("{% for c in cs %}x{{ ',' if not loop.last }}{% endfor %}"),
            "                 x                                      "
        );
        assert_eq!(
            mask("{% for c in cs %}x{% else %}y{% endfor %}"),
            "                 x                       "
        );
    }

    #[test]
    fn non_sql_blocks_are_blanked() {
        assert_eq!(
            mask("{% set s %}'a', 'b'{% endset %}SELECT 1"),
            "                               SELECT 1"
        );
        assert_eq!(
            mask("{% set s = 'a' %}SELECT 1"),
            "                 SELECT 1"
        );
        assert_eq!(
            mask("{% call statement('x') %}select 1{% endcall %}SELECT 2"),
            "                                              SELECT 2"
        );
        assert_eq!(
            mask("{% macro m(a) %}\n{{ a }} +\n{% endmacro %}"),
            "                \n         \n              "
        );
    }

    #[test]
    fn raw_bodies_are_sql_text() {
        assert_eq!(
            mask("SELECT {% raw %}'{{ x }}'{% endraw %} AS a"),
            "SELECT          '{{ x }}'             AS a"
        );
    }

    #[test]
    fn jinja_comment_directives() {
        let t = template("{# sqlsift:disable-file #}\nSELECT 1 {#- sqlsift:disable E0002 -#}");
        assert_eq!(
            t.directive_text(),
            "                           -- sqlsift:disable-file\nSELECT 1                               -- sqlsift:disable E0002"
        );
    }

    #[test]
    fn records_tag_text() {
        let t = template("SELECT {{ x }}\nFROM {{ ref('a') }}");
        assert_eq!(t.tag_at(1, 8), Some("{{ x }}"));
        assert_eq!(t.tag_at(2, 6), Some("{{ ref('a') }}"));
        assert_eq!(t.tag_at(2, 5), None);
    }

    #[test]
    fn closing_delimiters_in_strings_and_unclosed_tags() {
        assert_eq!(mask("SELECT {{ '}}' }} a"), "SELECT $1         a");
        assert_eq!(mask("SELECT 1 {% if"), "SELECT 1      ");
        assert_eq!(mask("SELECT '{' , '{x}'"), "SELECT '{' , '{x}'");
    }

    #[test]
    fn non_ascii_keeps_character_columns() {
        assert_eq!(mask("{# 日本語 #}SELECT 1"), "         SELECT 1");
    }

    #[test]
    fn records_identifier_positions() {
        let p = template("SELECT 1\nFROM {{ ref('a') }}.users x");
        assert!(p.masked.is_substituted(2, 6));
        assert!(p.masked.is_substituted(2, 25));
        assert!(!p.masked.is_substituted(2, 5));
        assert!(!p.masked.is_substituted(2, 27));
    }

    #[test]
    fn statement_start_skips_comments() {
        assert_eq!(
            mask("-- c\n{{ config(a=1) }}\nselect 1"),
            "-- c\n                 \nselect 1"
        );
        assert_eq!(mask("/* c */ {{ x }} select 1"), "/* c */         select 1");
    }

    #[test]
    fn continuations_of_select_lists_open_the_columns() {
        assert_eq!(
            mask("select a {{ m() }} from t"),
            "select a ,__m___.* from t"
        );
        let t = template("select a {{ m() }} from t");
        assert!(t.masked.is_substituted(1, 11));
        // After a comment line, a tag is still a continuation of `a`
        assert_eq!(
            mask("select a\n  -- note\n  {{ m() }}\nfrom t"),
            "select a\n  -- note\n  ,__m___.*\nfrom t"
        );
        // A tag followed by an alias is a value
        assert_eq!(
            mask("select a,\n-- x\n{{ f() }} as b from t"),
            "select a,\n-- x\n$1        as b from t"
        );
        // Multi-line: written on the first line long enough
        assert_eq!(
            mask("select a {{\n  m(x, y)\n}} from t"),
            "select a   \n,_m_x__.*\n   from t"
        );
    }

    #[test]
    fn clause_macros_are_blanked() {
        assert_eq!(
            mask("select a from t {{ group_by(1) }}"),
            "select a from t                  "
        );
        assert_eq!(
            mask("over (partition by a {{ p() }} order by b)"),
            "over (partition by a           order by b)"
        );
        // Not after a keyword that expects an operand
        assert_eq!(mask("where {{ c }}"), "where $1     ");
    }

    #[test]
    fn query_bodies_have_unknown_columns() {
        assert_eq!(
            mask("with s as ({{ date_spine() }}) select 1"),
            "with s as (select _spine___.*) select 1"
        );
        // Too short to hold a query: a value (a parse error)
        assert_eq!(mask("with s as ({{ x }})"), "with s as ($1     )");
    }

    #[test]
    fn aliases_built_from_tags() {
        let t = template("select 1 from o as {{ f }}_o join p as q_{{ g }}");
        assert!(t.may_be_templated_alias("a_o"));
        assert!(t.may_be_templated_alias("q_x"));
        assert!(!t.may_be_templated_alias("_o"));
        assert!(!t.may_be_templated_alias("x"));
    }

    #[test]
    fn known_branch_conditions() {
        let sets = HashMap::from([("enabled".to_string(), false)]);
        let known = Known {
            target: Some("postgres"),
            sets: &sets,
        };
        let eval = |c: &str| branch_condition(c, &known);
        assert_eq!(eval("not loop.last"), Some(false));
        assert_eq!(eval("is_incremental()"), Some(false));
        assert_eq!(eval("not is_incremental()"), Some(true));
        assert_eq!(eval("var('x', false)"), Some(false));
        assert_eq!(eval("var(\"x\", True)"), Some(true));
        assert_eq!(eval("var('x')"), None);
        assert_eq!(eval("var('x', [])"), None);
        assert_eq!(eval("enabled"), Some(false));
        assert_eq!(eval("other"), None);
        assert_eq!(eval("target.type == 'redshift'"), Some(false));
        assert_eq!(eval("target.type != 'postgres'"), Some(false));
        assert_eq!(eval("target.type in ['postgres', 'redshift']"), Some(true));
        assert_eq!(
            eval("target.type not in ('snowflake', 'bigquery')"),
            Some(true)
        );
        assert_eq!(eval("target.type == my_var"), None);
        assert_eq!(eval("is_incremental() and x"), Some(false));
        assert_eq!(eval("x or not is_incremental()"), Some(true));
        assert_eq!(eval("x and y"), None);
        assert_eq!(eval("(is_incremental())"), Some(false));
        // Unknown target
        let unknown = Known {
            target: None,
            sets: &sets,
        };
        assert_eq!(
            branch_condition("target.type == 'postgres'", &unknown),
            None
        );
    }

    #[test]
    fn known_branches_and_set_variables_choose_the_branch() {
        assert_eq!(
            mask("{% if is_incremental() %}a{% else %}b{% endif %}"),
            "                         a          b           ".replace('a', " ")
        );
        assert_eq!(
            mask("{% set on = var('x', false) %}{% if on %}a{% endif %}b"),
            "                                                     b"
        );
    }
}
