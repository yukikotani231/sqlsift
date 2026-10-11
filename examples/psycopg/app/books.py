"""Book queries with psycopg (DB-API cursors)"""

import psycopg


def get_book(conn: psycopg.Connection, book_id: int):
    with conn.cursor() as cur:
        cur.execute(
            """
            SELECT b.id, b.title, b.isbn, a.name AS author
            FROM books AS b
            JOIN authors AS a ON a.id = b.author_id
            WHERE b.id = %s
            """,
            (book_id,),
        )
        return cur.fetchone()


def books_by_authors(conn: psycopg.Connection, author_ids: list[int]):
    with conn.cursor() as cur:
        cur.execute(
            "SELECT id, title FROM books "
            "WHERE author_id IN %(ids)s AND in_stock "
            "ORDER BY published_on DESC",
            {"ids": tuple(author_ids)},
        )
        return cur.fetchall()


def add_book(conn: psycopg.Connection, author_id: int, title: str, isbn: str | None):
    with conn.cursor() as cur:
        cur.execute(
            "INSERT INTO books (author_id, title, isbn) VALUES (%s, %s, %s) RETURNING id",
            (author_id, title, isbn),
        )
        return cur.fetchone()[0]
