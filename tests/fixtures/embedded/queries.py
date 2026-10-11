import sqlite3

from sqlalchemy import text


def get_author(cur, author_id):
    cur.execute("SELECT id, name FROM authors WHERE id = %s", (author_id,))
    return cur.fetchone()


def list_posts(cur, author_ids):
    cur.execute(
        """
        SELECT id, title, nme
        FROM posts
        WHERE author_id IN %s AND published = %(published)s
        """,
        {"ids": tuple(author_ids), "published": True},
    )


async def post_titles(conn, author_id):
    return await conn.fetch(
        "SELECT title FROM posts "
        "WHERE author_id = $1",
        author_id,
    )


def by_name(session, name):
    return session.execute(text("SELECT id FROM author WHERE name = :name"), {"name": name})


def dynamic(cur, column):
    # Built at run time: not checked
    cur.execute(f"SELECT {column} FROM nowhere")
    cur.execute("SELECT * FROM " + column)
    print("SELECT nothing FROM nowhere")
    # sqlsift:disable E0002
    cur.execute("SELECT nothing FROM authors")
