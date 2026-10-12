use std::fmt::Write;

use sqlsift_parser::{parse, SyntaxKind};

fn dump(sql: &str) -> String {
    let parse = parse(sql);
    let mut out = parse.root.debug_dump(sql);
    for e in &parse.errors {
        let _ = writeln!(
            out,
            "error {}..{}: {}",
            e.range.start, e.range.end, e.message
        );
    }
    out
}

/// Every input gives back its text, errors or not.
#[test]
fn tree_is_lossless() {
    for sql in [
        "SELECT a, b FROM t WHERE x = 1; -- trailing\n",
        "  /* lead */ select * from t join u using (id)",
        "SELECT FROM WHERE ((( ;; garbage $$ unterminated",
        "SELECT 'unterminated",
        "CREATE TABLE t (a int); SELECT (a, b) IN (SELECT 1, 2) FROM t",
        "SELECT a::int[] , b[1:2], CASE WHEN x THEN 1 ELSE 2 END FROM t AS x (a, b)",
        "",
        ";",
    ] {
        let parse = parse(sql);
        let text: String = parse
            .root
            .descendant_tokens()
            .iter()
            .map(|t| t.text(sql))
            .collect();
        assert_eq!(text, sql);
        assert_eq!(parse.root.range.end as usize, sql.len());
    }
}

#[test]
fn select_tree() {
    insta_like(
        "SELECT t.a AS x, count(*) FROM s.t t LEFT JOIN u ON t.id = u.id WHERE a + 1 * 2 > 3",
        "\
SourceFile
  SelectStmt
    SelectClause
      Ident \"SELECT\"
      TargetList
        TargetItem
          ColumnRef
            Name
              Ident \"t\"
              Dot \".\"
              Ident \"a\"
          Alias
            Ident \"AS\"
            Ident \"x\"
        Comma \",\"
        TargetItem
          FuncCall
            Name
              Ident \"count\"
            ArgList
              LParen \"(\"
              Star
                Operator \"*\"
              RParen \")\"
    FromClause
      Ident \"FROM\"
      JoinExpr
        TableRef
          Name
            Ident \"s\"
            Dot \".\"
            Ident \"t\"
          Alias
            Ident \"t\"
        Ident \"LEFT\"
        Ident \"JOIN\"
        TableRef
          Name
            Ident \"u\"
        JoinCondition
          Ident \"ON\"
          BinaryExpr
            ColumnRef
              Name
                Ident \"t\"
                Dot \".\"
                Ident \"id\"
            Operator \"=\"
            ColumnRef
              Name
                Ident \"u\"
                Dot \".\"
                Ident \"id\"
    WhereClause
      Ident \"WHERE\"
      BinaryExpr
        BinaryExpr
          ColumnRef
            Name
              Ident \"a\"
          Operator \"+\"
          BinaryExpr
            Literal
              Number \"1\"
            Operator \"*\"
            Literal
              Number \"2\"
        Operator \">\"
        Literal
          Number \"3\"
",
    );
}

fn insta_like(sql: &str, expected: &str) {
    let actual = dump(sql);
    assert_eq!(actual, expected, "\n{actual}");
}

/// A broken expression is an Error node or a recorded error, and the next statement is
/// parsed normally.
#[test]
fn recovers_at_the_next_statement() {
    let sql = "SELECT a FROM t WHERE a = = 1; SELECT b FROM u";
    let parse = parse(sql);
    assert!(!parse.errors.is_empty());
    let stmts: Vec<_> = parse.statements().collect();
    assert_eq!(stmts.len(), 2);
    assert!(!stmts[1].has_errors());
    assert_eq!(stmts[1].text(sql), "SELECT b FROM u");
}

#[test]
fn unknown_statements_are_opaque() {
    let sql = "CREATE FUNCTION f() RETURNS int AS $$ SELECT 1; $$ LANGUAGE sql; SELECT 1";
    let parse = parse(sql);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let kinds: Vec<_> = parse.statements().map(|s| s.kind).collect();
    assert_eq!(kinds, [SyntaxKind::OpaqueStmt, SyntaxKind::SelectStmt]);
}

/// Missing pieces are reported once each, and the clause after them is still parsed.
#[test]
fn recovers_inside_a_statement() {
    let sql = "SELECT a, FROM t WHERE b";
    let parse = parse(sql);
    assert_eq!(parse.errors.len(), 1, "{:?}", parse.errors);
    let stmt = parse.statements().next().unwrap();
    assert!(stmt.child(SyntaxKind::FromClause).is_some());
    assert!(stmt.child(SyntaxKind::WhereClause).is_some());
}
