"""Support desk queries, loaded by aiosql from ../queries."""

from pathlib import Path

import aiosql
import psycopg

queries = aiosql.from_path(Path(__file__).parent.parent / "queries", "psycopg")


def open_ticket(conn: psycopg.Connection, customer_id: int, subject: str, priority: int = 3):
    row = queries.tickets.open_ticket(
        conn, customer_id=customer_id, subject=subject, priority=priority
    )
    conn.commit()
    return row


def open_tickets(conn: psycopg.Connection, customer):
    # `customer` is any object with an `id` attribute (`:customer.id`)
    return list(queries.tickets.list_open_tickets(conn, customer=customer))
