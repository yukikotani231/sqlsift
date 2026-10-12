package store

import (
	"context"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

// pgx: named arguments
func SetInStock(ctx context.Context, pool *pgxpool.Pool, id int64, inStock bool) error {
	_, err := pool.Exec(ctx, "UPDATE books SET in_stock = @in_stock WHERE id = @id",
		pgx.NamedArgs{"id": id, "in_stock": inStock})
	return err
}

func CountInStock(ctx context.Context, pool *pgxpool.Pool) (int64, error) {
	var n int64
	err := pool.QueryRow(ctx, "SELECT count(*) FROM books WHERE in_stock").Scan(&n)
	return n, err
}
