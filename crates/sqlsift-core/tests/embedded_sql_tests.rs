// SQL from application code: sqlc query names and TypeScript tagged templates
use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::error::{Diagnostic, DiagnosticKind};
use sqlsift_core::schema::{Catalog, SchemaBuilder};

const SCHEMA: &str = include_str!("../../../tests/fixtures/embedded/schema.sql");
const QUERIES_TS: &str = include_str!("../../../tests/fixtures/embedded/queries.ts");

fn catalog() -> Catalog {
    let mut builder = SchemaBuilder::new();
    builder.parse(SCHEMA).unwrap();
    builder.build().0
}

fn analyze(sql: &str) -> Vec<Diagnostic> {
    Analyzer::new(&catalog()).analyze(sql)
}

fn analyze_ts(source: &str, tags: &[&str]) -> Vec<Diagnostic> {
    Analyzer::new(&catalog()).analyze_embedded(source, tags)
}

/// (kind, line, column) of each diagnostic
fn locations(diagnostics: &[Diagnostic]) -> Vec<(DiagnosticKind, usize, usize)> {
    diagnostics
        .iter()
        .map(|d| {
            let span = d.span.expect("span");
            (d.kind, span.line, span.column)
        })
        .collect()
}

#[test]
fn tagged_templates_are_checked_at_their_location() {
    assert_eq!(
        locations(&analyze_ts(QUERIES_TS, &["sql"])),
        vec![
            (DiagnosticKind::ColumnNotFound, 16, 28),
            (DiagnosticKind::TableNotFound, 31, 10),
        ]
    );
}

#[test]
fn configured_tags_select_the_templates() {
    assert_eq!(
        locations(&analyze_ts(QUERIES_TS, &["sql", "$queryRaw"])),
        vec![
            (DiagnosticKind::ColumnNotFound, 16, 28),
            (DiagnosticKind::TableNotFound, 31, 10),
            (DiagnosticKind::ColumnNotFound, 35, 83),
        ]
    );
    assert!(analyze_ts(QUERIES_TS, &["query"]).is_empty());
}

#[test]
fn byte_offsets_point_into_the_original_source() {
    let source = "const s = 'Ünïcode';\nconst q = sql`SELECT nme FROM authors`;\n";
    let diagnostics = analyze_ts(source, &["sql"]);
    assert_eq!(diagnostics.len(), 1);
    let span = diagnostics[0].span.unwrap();
    assert_eq!((span.line, span.column), (2, 22));
    assert_eq!(&source[span.offset..span.offset + 3], "nme");
}

#[test]
fn interpolations_are_untyped_values() {
    let source = r"
        sql`SELECT id FROM posts WHERE id = ${id} AND title = ${t} LIMIT ${n}`;
        sql`INSERT INTO posts (author_id, title, body) VALUES (${a}, ${b}, ${c})`;
        sql`UPDATE posts SET published = ${p} WHERE id IN ${ids}`;
    ";
    let diagnostics = analyze_ts(source, &["sql"]);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn interpolated_table_names_are_not_reported() {
    let source = "sql`SELECT anything FROM ${table} WHERE x = 1`;\n\
                  sql`INSERT INTO ${t} (a) VALUES (1)`;\n\
                  sql`SELECT anything FROM ${schema}.posts`;\n\
                  sql`SELECT a.nme FROM authors a JOIN ${t} ON true`;";
    assert_eq!(
        locations(&analyze_ts(source, &["sql"])),
        vec![(DiagnosticKind::ColumnNotFound, 4, 14)]
    );
}

#[test]
fn templates_are_separate_statements() {
    // A `--` comment at the end of a template doesn't hide the next one
    let source = "sql`SELECT id FROM posts -- all posts`;\nsql`SELECT nme FROM authors`;";
    assert_eq!(
        locations(&analyze_ts(source, &["sql"])),
        vec![(DiagnosticKind::ColumnNotFound, 2, 12)]
    );
}

#[test]
fn parse_errors_point_into_the_template() {
    let source = "const a = 1;\nconst q = sql`SELEC id FROM posts`;";
    let diagnostics = analyze_ts(source, &["sql"]);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ParseError);
    assert_eq!(diagnostics[0].span.unwrap().line, 2);
}

#[test]
fn files_without_tagged_templates_have_no_diagnostics() {
    let source = "export const x = `SELECT nme FROM nowhere`;\n// sql`SELECT 1`\n";
    assert!(analyze_ts(source, &["sql"]).is_empty());
}

#[test]
fn mysql_placeholders() {
    let mut builder = SchemaBuilder::with_dialect(SqlDialect::MySQL);
    builder
        .parse("CREATE TABLE posts (id INT PRIMARY KEY, title TEXT);")
        .unwrap();
    let catalog = builder.build().0;
    let source = "sql`SELECT title FROM posts WHERE id = ${id} AND id IN ${ids}`;\n\
                  sql`SELECT titel FROM posts`;";
    let diagnostics =
        Analyzer::with_dialect(&catalog, SqlDialect::MySQL).analyze_embedded(source, &["sql"]);
    assert_eq!(
        locations(&diagnostics),
        vec![(DiagnosticKind::ColumnNotFound, 2, 12)]
    );
}

#[test]
fn inline_directives_work_inside_templates() {
    let source = "sql`\n  -- sqlsift:disable E0002\n  SELECT nme FROM authors\n`;";
    assert!(analyze_ts(source, &["sql"]).is_empty());
}

/// (kind, line, column, query name) of each diagnostic
fn summary(diagnostics: &[Diagnostic]) -> Vec<(DiagnosticKind, usize, usize, Option<&str>)> {
    diagnostics
        .iter()
        .map(|d| {
            let span = d.span.expect("span");
            (d.kind, span.line, span.column, d.query_name.as_deref())
        })
        .collect()
}

#[test]
fn sqlc_query_names_are_attached_to_diagnostics() {
    let diagnostics = analyze(include_str!("../../../tests/fixtures/embedded/queries.sql"));
    assert_eq!(
        summary(&diagnostics),
        vec![
            (DiagnosticKind::ColumnNotFound, 8, 12, Some("ListPosts")),
            (DiagnosticKind::TableNotFound, 18, 22, Some("GetAuthor")),
        ]
    );
}

#[test]
fn statements_before_the_first_name_have_no_query_name() {
    let diagnostics =
        analyze("SELECT nme FROM authors;\n-- name: A :one\nSELECT nme FROM authors;");
    assert_eq!(
        summary(&diagnostics),
        vec![
            (DiagnosticKind::ColumnNotFound, 1, 8, None),
            (DiagnosticKind::ColumnNotFound, 3, 8, Some("A")),
        ]
    );
}

#[test]
fn parse_errors_get_the_query_name() {
    let diagnostics = analyze("-- name: Broken :exec\nSELEC 1;\n\n-- name: Fine :one\nSELECT 1;");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ParseError);
    assert_eq!(diagnostics[0].query_name.as_deref(), Some("Broken"));
}

// postgres.js, kysely, Prisma and Slonik patterns (#121)

#[test]
fn postgres_js_insert_and_update_helpers() {
    let source = r"
        await sql`INSERT INTO posts ${sql(post, 'author_id', 'title')}`;
        await sql`INSERT INTO posts (author_id, title) VALUES ${sql(rows)}`;
        await sql`UPDATE posts SET ${sql(patch, 'title')} WHERE id = ${id}`;
        await sql`INSERT INTO posts ${sql(post)} ON CONFLICT (id) DO NOTHING RETURNING id`;
        await sql`INSERT INTO authors (name) VALUES ${sql(rows)} RETURNING nme`;
        await sql`UPDATE posts SET ${sql(patch)} WHERE idd = ${id}`;
        await sql`INSERT INTO ${table} ${sql(row)}`;
        await sql`INSERT INTO posts (author_id, title) VALUES ${sql(rows)} ON CONFLICT (id) DO UPDATE SET title = excluded.title`;
    ";
    assert_eq!(
        locations(&analyze_ts(source, &["sql"])),
        vec![
            (DiagnosticKind::ColumnNotFound, 6, 76),
            (DiagnosticKind::ColumnNotFound, 7, 56),
        ]
    );
}

#[test]
fn interpolated_fragments_between_clauses_are_dropped() {
    let source = r"
        sql`SELECT p.id FROM posts p WHERE p.title ILIKE ${t} ${cond ? sql`AND p.published` : sql``} ORDER BY p.id`;
        prisma.$queryRaw`SELECT id FROM authors ${where}`;
        sql`SELECT id FROM posts WHERE ${cond ? sql`published` : sql`true`} ORDER BY id ${dir}`;
        sql`SELECT titel FROM posts ${where}`;
    ";
    assert_eq!(
        locations(&analyze_ts(source, &["sql", "$queryRaw"])),
        vec![(DiagnosticKind::ColumnNotFound, 5, 20)]
    );
}

#[test]
fn templates_that_are_not_statements_are_fragments() {
    let source = r"
        const a = sql<boolean>`published = ${true}`;
        db.selectFrom('posts').where(sql`author_id = ${id}`);
        const w = Prisma.sql`WHERE id > ${minId}`;
        const f = sql`AND nme = ${x}`;
        const e = sql``;
        const q = sql`${a} UNION ${b}`;
        const s = sql`
          -- leading comments are skipped
          (SELECT nme FROM authors)
        `;
    ";
    assert_eq!(
        locations(&analyze_ts(source, &["sql"])),
        vec![(DiagnosticKind::ColumnNotFound, 10, 19)]
    );
}

#[test]
fn slonik_call_and_member_tags() {
    let source = r"
        sql.type(z.object({ id: z.number() }))`SELECT idd FROM posts`;
        sql.typeAlias('id')`SELECT idd FROM posts`;
        sql.unsafe`SELECT idd FROM posts`;
        sql.fragment`AND idd = 1`;
    ";
    assert_eq!(
        locations(&analyze_ts(source, &["sql"])),
        vec![
            (DiagnosticKind::ColumnNotFound, 2, 55),
            (DiagnosticKind::ColumnNotFound, 3, 36),
            (DiagnosticKind::ColumnNotFound, 4, 27),
        ]
    );
}

#[test]
fn directives_in_code_comments() {
    let file = "// sqlsift:disable-file\nconst q = sql`SELECT nme FROM authors`;";
    assert!(analyze_ts(file, &["sql"]).is_empty());
    let file = "/* sqlsift:disable-file E0002 */\nsql`SELECT nme FROM nowhere`;";
    assert_eq!(
        locations(&analyze_ts(file, &["sql"])),
        vec![(DiagnosticKind::TableNotFound, 2, 21)]
    );
    // A next-line directive applies to the template's first line of SQL
    let next_line =
        "// sqlsift:disable E0002\nconst rows = await sql`\n  SELECT nme FROM authors\n`;\n\
                     sql`SELECT nme FROM authors`;";
    assert_eq!(
        locations(&analyze_ts(next_line, &["sql"])),
        vec![(DiagnosticKind::ColumnNotFound, 5, 12)]
    );
    let inline = "sql`SELECT nme FROM authors`; // sqlsift:disable column-not-found";
    assert!(analyze_ts(inline, &["sql"]).is_empty());
}

#[test]
fn vue_and_svelte_script_blocks() {
    let source = "<template>\n  <p>{{ sql`SELECT nope` }}</p>\n</template>\n\
                  <script setup lang=\"ts\">\nconst q = await sql`SELECT nme FROM authors`;\n</script>\n";
    let diagnostics = Analyzer::new(&catalog()).analyze_embedded_component(source, &["sql"]);
    assert_eq!(
        locations(&diagnostics),
        vec![(DiagnosticKind::ColumnNotFound, 5, 28)]
    );
}
