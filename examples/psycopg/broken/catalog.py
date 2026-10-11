"""Intentional mistakes: each fails only when its query runs"""


def search(cur, term):
    cur.execute("SELECT id, titel FROM books WHERE title ILIKE %s", (f"%{term}%",))


def restock(cur, book_id):
    cur.execute("UPDATE books SET in_stock = 'yes please' WHERE id = %s", (book_id,))


def add_author(cur):
    cur.execute("INSERT INTO authors (born) VALUES (%s)", ("1970-01-01",))
