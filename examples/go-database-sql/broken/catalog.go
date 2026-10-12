package broken

import (
	"context"
	"database/sql"
)

// Intentional mistakes: each fails only when its query runs

func Search(ctx context.Context, db *sql.DB, term string) (*sql.Rows, error) {
	return db.QueryContext(ctx, "SELECT id, titel FROM books WHERE title ILIKE $1", "%"+term+"%")
}

func Restock(ctx context.Context, db *sql.DB, id int64) error {
	_, err := db.ExecContext(ctx, "UPDATE books SET in_stock = 'yes please' WHERE id = $1", id)
	return err
}

func AddAuthor(ctx context.Context, db *sql.DB) error {
	_, err := db.ExecContext(ctx, "INSERT INTO authors (born) VALUES ($1)", "1970-01-01")
	return err
}
