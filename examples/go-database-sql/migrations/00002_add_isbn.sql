-- +goose Up
ALTER TABLE books ADD COLUMN isbn TEXT UNIQUE;

-- +goose Down
ALTER TABLE books DROP COLUMN isbn;
