//! sqlc query names
//!
//! [sqlc](https://sqlc.dev) query files name each query with a comment before it:
//!
//! ```sql
//! -- name: GetPost :one
//! SELECT * FROM posts WHERE id = $1;
//! ```
//!
//! [`QueryNames`] finds these comments so that diagnostics can say which query they
//! are in.

/// The sqlc query names of a file, by the line of their `-- name:` comment
pub(crate) struct QueryNames<'a> {
    /// (1-indexed line, name), in line order
    names: Vec<(usize, &'a str)>,
}

impl<'a> QueryNames<'a> {
    /// Find the `-- name: <Name> :<command>` comments in `sql`
    pub fn parse(sql: &'a str) -> Self {
        if !sql.contains("name:") {
            return Self { names: Vec::new() };
        }
        let names = sql
            .lines()
            .enumerate()
            .filter_map(|(i, line)| Some((i + 1, query_name(line)?)))
            .collect();
        Self { names }
    }

    /// Name of the query that `line` belongs to: the last `-- name:` comment at or
    /// before it
    pub fn at(&self, line: usize) -> Option<&'a str> {
        let index = self.names.partition_point(|&(l, _)| l <= line);
        index.checked_sub(1).map(|i| self.names[i].1)
    }
}

/// The query name of a `-- name: <Name> :<command>` line
fn query_name(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix("--")?.trim_start();
    let rest = rest.strip_prefix("name:")?;
    let mut words = rest.split_whitespace();
    let name = words.next()?;
    let command = words.next()?.strip_prefix(':')?;
    let is_word = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    };
    (is_word(name) && is_word(command)).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_comments() {
        assert_eq!(query_name("-- name: GetPost :one"), Some("GetPost"));
        assert_eq!(
            query_name("  --name: list_posts :many  "),
            Some("list_posts")
        );
        assert_eq!(
            query_name("-- name: CreatePosts :copyfrom extra"),
            Some("CreatePosts")
        );
        assert_eq!(query_name("-- name: GetPost"), None);
        assert_eq!(query_name("-- name GetPost :one"), None);
        assert_eq!(query_name("SELECT 1 -- name: GetPost :one"), None);
    }

    #[test]
    fn finds_the_query_of_a_line() {
        let names = QueryNames::parse(
            "SELECT 1;\n-- name: A :one\nSELECT 2;\n\n-- name: B :many\nSELECT 3;",
        );
        assert_eq!(names.at(1), None);
        assert_eq!(names.at(2), Some("A"));
        assert_eq!(names.at(4), Some("A"));
        assert_eq!(names.at(6), Some("B"));
    }
}
