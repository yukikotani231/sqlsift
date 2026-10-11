"""Reports with SQLAlchemy Core: text() with :name parameters"""

from sqlalchemy import text
from sqlalchemy.engine import Connection


def books_per_author(conn: Connection, since):
    return conn.execute(
        text(
            """
            SELECT a.name, count(b.id) AS books
            FROM authors AS a
            LEFT JOIN books AS b ON b.author_id = a.id AND b.published_on >= :since
            GROUP BY a.id, a.name
            ORDER BY books DESC
            """
        ),
        {"since": since},
    ).all()


def sort_column(conn: Connection, column: str):
    # Built at run time, so not checked
    return conn.execute(text(f"SELECT id FROM books ORDER BY {column}")).all()
