//! The parser against sqlparser: statements the lowering handles must give exactly
//! sqlparser's AST, spans included.

use std::path::Path;

use sqlsift_parser::diff::{compare_source, compare_statement, first_difference, Outcome};

#[track_caller]
fn assert_same(sql: &str) {
    match compare_statement(sql) {
        Outcome::Same => {}
        Outcome::Different { ours, theirs } | Outcome::SpansDiffer { ours, theirs } => {
            panic!("{sql}\n{}", first_difference(&ours, &theirs));
        }
        other => panic!("{sql}: {other:?}"),
    }
}

#[test]
fn select_subset_matches_sqlparser() {
    for sql in [
        "SELECT 1",
        "SELECT a, b AS c, d e, \"Q\"\"x\" FROM t",
        "SELECT * FROM s.t AS x (a, b) WHERE x.a = 1",
        "SELECT t.*, u.* FROM t, u",
        "SELECT DISTINCT a FROM t",
        "SELECT DISTINCT ON (a, b) a, b FROM t ORDER BY a, b DESC NULLS LAST",
        "SELECT ALL a FROM t",
        "SELECT count(*), count(DISTINCT a), sum(b) FROM t GROUP BY c HAVING count(*) > 1",
        "SELECT a FROM t LIMIT 10 OFFSET 5",
        "SELECT a FROM t OFFSET 5 ROWS",
        "SELECT a FROM t1 JOIN t2 ON t1.id = t2.id LEFT JOIN t3 USING (id) \
         RIGHT OUTER JOIN t4 ON TRUE FULL JOIN t5 ON t5.x = t4.x CROSS JOIN t6 \
         NATURAL JOIN t7 INNER JOIN t8 ON t8.y = t1.y",
        "SELECT * FROM (t1 JOIN t2 ON t1.id = t2.id) AS j",
        "SELECT * FROM (SELECT a FROM t) AS s (x)",
        "SELECT a FROM t WHERE a + 1 * 2 - 3 / 4 % 5 > 6 AND NOT b OR c",
        "SELECT -a, +b, -1, 2 ^ 3, 'x' || 'y', j -> 'k', j ->> 'k', j @> '{}' FROM t",
        "SELECT a IS NULL, a IS NOT NULL, a NOTNULL, a IS TRUE, a IS NOT FALSE, \
         a IS UNKNOWN, a IS DISTINCT FROM b, a IS NOT DISTINCT FROM b FROM t",
        "SELECT a FROM t WHERE a IN (1, 2) AND b NOT IN (SELECT b FROM u) \
         AND c BETWEEN 1 AND 2 AND d NOT BETWEEN 3 AND 4",
        "SELECT a FROM t WHERE a LIKE 'x%' AND b NOT ILIKE $1 AND c SIMILAR TO 'y'",
        "SELECT (a), (a, b), (SELECT 1) FROM t WHERE EXISTS (SELECT 1) AND NOT EXISTS (SELECT 2)",
        "SELECT CAST(a AS integer), b::text, c::varchar(10), d::numeric(10, 2), \
         e::timestamp with time zone, f::double precision, g::int[] FROM t",
        "SELECT CASE WHEN a THEN 1 WHEN b THEN 2 ELSE 3 END, CASE a WHEN 1 THEN 'x' END FROM t",
        "SELECT $1, 'it''s', N'n', $$dollar$$, $tag$x$tag$, 1.5e3, TRUE, NULL",
        "SELECT a\n  FROM t\n -- comment\n WHERE b = 'x' /* block */ AND c < 2",
        "SELECT f(), s.f(a, b) FROM t",
        "WITH x (a) AS (SELECT 1), y AS MATERIALIZED (SELECT 2), \
         z AS NOT MATERIALIZED (SELECT 3) SELECT a FROM x",
        "WITH RECURSIVE r AS (SELECT 1 AS n UNION ALL SELECT n + 1 FROM r) SELECT * FROM r",
        "SELECT 1 UNION SELECT 2 INTERSECT SELECT 3 EXCEPT ALL SELECT 4 ORDER BY 1 LIMIT 1",
        "(SELECT a FROM t ORDER BY a LIMIT 1) UNION DISTINCT (SELECT b FROM u)",
        "SELECT * FROM (WITH c AS (SELECT 1) SELECT * FROM c) s \
         WHERE x IN (SELECT 1 UNION SELECT 2)",
        "VALUES (1, 'a'), (2, 'b')",
        "SELECT * FROM (VALUES (1, 2)) AS v (a, b)",
        "SELECT row_number() OVER (PARTITION BY a ORDER BY b DESC), \
         sum(x) OVER (ORDER BY y ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW), \
         avg(x) OVER (RANGE 2 PRECEDING), lag(x) OVER w, \
         count(*) FILTER (WHERE x > 1), \
         percentile_cont(0.5) WITHIN GROUP (ORDER BY x), \
         string_agg(DISTINCT x, ',' ORDER BY x) \
         FROM t WINDOW w AS (PARTITION BY a)",
        "SELECT * FROM generate_series(1, 3) WITH ORDINALITY AS g (n, i), \
         LATERAL f(g.n) AS y, LATERAL (SELECT 1) AS z",
        "SELECT a FROM t WHERE a = ANY($1) AND b <> ALL (SELECT 1) AND c = SOME($2)",
        "SELECT (x).y, (x).y.z, a[1], a[1][2], a[1:2], a[:2], t.a[1], ARRAY[1, 2], \
         ARRAY[[1], [2]], ARRAY(SELECT 1), DATE '2024-01-01', INTERVAL '1 day', \
         timestamp with time zone '2024-01-01 00:00', CURRENT_DATE, current_timestamp(3), \
         CURRENT_USER, EXTRACT(YEAR FROM d), x COLLATE \"C\", t AT TIME ZONE 'UTC', \
         E'a\\nb', B'101', X'1f', U&'d' FROM t LIMIT ALL",
        "SELECT a FROM t WHERE b LIKE 'x!%' ESCAPE '!' AND c @@ d",
    ] {
        assert_same(sql);
    }
}

/// Where sqlparser departs from PostgreSQL, this parser follows PostgreSQL and the
/// comparison reports a difference.
#[test]
fn postgres_wins_over_sqlparser() {
    // PostgreSQL: `a ISNULL` is `a IS NULL`. sqlparser reads ISNULL as a column alias.
    let sql = "SELECT a ISNULL FROM t";
    assert!(matches!(compare_statement(sql), Outcome::Different { .. }));
    // PostgreSQL rejects these; sqlparser accepts some of them.
    for sql in [
        "SELECT CURRENT_DATE()",
        "SELECT USER()",
        "SELECT f(x).y FROM t",
    ] {
        let outcome = compare_statement(sql);
        assert!(
            matches!(outcome, Outcome::Unsupported(_) | Outcome::Neither),
            "{sql}: {outcome:?}"
        );
    }
}

/// On the repository's own SQL files, no statement may lower to a different AST.
#[test]
fn no_differences_on_repository_sql() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = vec![];
    for dir in ["tests/fixtures", "examples"] {
        collect(&root.join(dir), &mut files);
    }
    assert!(!files.is_empty());
    let mut same = 0;
    for file in files {
        let source = std::fs::read_to_string(&file).unwrap();
        for stmt in compare_source(&source) {
            match stmt.outcome {
                Outcome::Same => same += 1,
                Outcome::Different { ours, theirs } | Outcome::SpansDiffer { ours, theirs } => {
                    panic!(
                        "{}: {}\n{}",
                        file.display(),
                        stmt.text,
                        first_difference(&ours, &theirs)
                    );
                }
                _ => {}
            }
        }
    }
    assert!(same > 0);
}

fn collect(path: &Path, out: &mut Vec<std::path::PathBuf>) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).unwrap() {
            collect(&entry.unwrap().path(), out);
        }
    } else if path.extension().is_some_and(|e| e == "sql") {
        out.push(path.to_path_buf());
    }
}
