package store

import (
	"context"
	"database/sql"
	"fmt"

	"github.com/jmoiron/sqlx"
)

type Book struct {
	ID     int64   `db:"id"`
	Title  string  `db:"title"`
	Author string  `db:"author"`
	Price  float64 `db:"price"`
}

// database/sql: the query is the argument after ctx
func BookByID(ctx context.Context, db *sql.DB, id int64) (Book, error) {
	var b Book
	err := db.QueryRowContext(ctx, `
		SELECT b.id, b.title, a.name AS author, b.price
		FROM books b
		JOIN authors a ON a.id = b.author_id
		WHERE b.id = $1`, id).Scan(&b.ID, &b.Title, &b.Author, &b.Price)
	return b, err
}

// sqlx: after ctx and the destination; `IN (?)` expanded by sqlx.In and rebound
func BooksByIDs(ctx context.Context, db *sqlx.DB, ids []int64) ([]Book, error) {
	query, args, err := sqlx.In("SELECT b.id, b.title, a.name AS author, b.price "+
		"FROM books b JOIN authors a ON a.id = b.author_id WHERE b.id IN (?)", ids)
	if err != nil {
		return nil, err
	}
	var books []Book
	err = db.SelectContext(ctx, &books, db.Rebind(query), args...)
	return books, err
}

// sqlx named query
func AddBook(db *sqlx.DB, b Book, authorID int64) error {
	_, err := db.NamedExec(`INSERT INTO books (author_id, title, price, isbn)
		VALUES (:author_id, :title, :price, :isbn)`, map[string]any{
		"author_id": authorID, "title": b.Title, "price": b.Price, "isbn": nil,
	})
	return err
}

// Built at run time: not checked
func SortedBy(ctx context.Context, db *sql.DB, column string) (*sql.Rows, error) {
	return db.QueryContext(ctx, fmt.Sprintf("SELECT id, title FROM books ORDER BY %s", column))
}
