package store

import (
	"context"
	"database/sql"
	"fmt"

	"github.com/jmoiron/sqlx"
)

func GetAuthor(ctx context.Context, db *sql.DB, id int64) *sql.Row {
	return db.QueryRowContext(ctx, "SELECT id, name FROM authors WHERE id = $1", id)
}

func ListPosts(ctx context.Context, db *sqlx.DB, authorID int64) ([]Post, error) {
	var posts []Post
	err := db.SelectContext(ctx, &posts, `
		SELECT id, title, nme
		FROM posts
		WHERE author_id = $1`, authorID)
	return posts, err
}

func PostTitles(ctx context.Context, conn *pgx.Conn, authorID int64) (pgx.Rows, error) {
	return conn.Query(ctx, "SELECT title FROM posts "+
		"WHERE author_id = @author_id", pgx.NamedArgs{"author_id": authorID})
}

func ByName(db *sqlx.DB, name string) (*sqlx.Rows, error) {
	return db.NamedQuery("SELECT id FROM author WHERE name = :name", map[string]any{"name": name})
}

func Dynamic(db *sql.DB, column string) {
	// Built at run time: not checked
	db.Query(fmt.Sprintf("SELECT %s FROM nowhere", column))
	db.Query("SELECT * FROM " + column)
	fmt.Println("SELECT nothing FROM nowhere")
	// sqlsift:disable E0002
	db.Exec("SELECT nothing FROM authors")
}
