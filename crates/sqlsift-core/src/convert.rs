//! sqlparser's AST converted to the analyzer's ([`sqlsift_ast::from_sqlparser`])

pub(crate) use sqlsift_ast::from_sqlparser::statement;

#[cfg(test)]
mod tests {
    use sqlparser::dialect::PostgreSqlDialect;
    use sqlparser::parser::Parser;

    /// Every fixture query converts into a statement that prints the same SQL.
    #[test]
    fn round_trips_fixture_queries() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");
        let mut failures = Vec::new();
        let mut checked = 0;
        for pattern in ["tests/fixtures/**/*.sql", "examples/**/*.sql"] {
            for path in glob::glob(&format!("{root}{pattern}")).unwrap().flatten() {
                let Ok(sql) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let Ok(statements) = Parser::parse_sql(&PostgreSqlDialect {}, &sql) else {
                    continue;
                };
                for statement in &statements {
                    if !matches!(
                        statement,
                        sqlparser::ast::Statement::Query(_)
                            | sqlparser::ast::Statement::Insert(_)
                            | sqlparser::ast::Statement::Update(_)
                            | sqlparser::ast::Statement::Delete(_)
                    ) {
                        continue;
                    }
                    checked += 1;
                    let expected = statement.to_string();
                    let actual = sqlsift_ast::from_sqlparser::statement(statement).to_string();
                    if expected != actual {
                        failures.push(format!(
                            "{}\n  sp: {expected}\n  a:  {actual}",
                            path.display()
                        ));
                    }
                }
            }
        }
        assert!(checked > 30, "only {checked} statements checked");
        assert!(
            failures.is_empty(),
            "{} of {checked} differ:\n{}",
            failures.len(),
            failures
                .iter()
                .take(20)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
