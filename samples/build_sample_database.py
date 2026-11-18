"""Rebuild `sample.sqlite`, the database used by the built-in samples.

Run from anywhere::

    python3 samples/build_sample_database.py

The file is committed so the app can embed it without a Python build step.
Regenerate it whenever the schema or seed data changes, then run the samples
test (`cargo test -p codingpad-kernel-core --test samples`).
"""

from pathlib import Path
import sqlite3

OUTPUT = Path(__file__).with_name("sample.sqlite")

SCHEMA = """
PRAGMA foreign_keys = ON;

CREATE TABLE categories (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL UNIQUE,
    description TEXT    NOT NULL
);

CREATE TABLE products (
    id           INTEGER PRIMARY KEY,
    name         TEXT    NOT NULL,
    category_id  INTEGER NOT NULL REFERENCES categories(id),
    price        NUMERIC NOT NULL,
    stock        INTEGER NOT NULL,
    discontinued INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE customers (
    id        INTEGER PRIMARY KEY,
    name      TEXT NOT NULL,
    email     TEXT NOT NULL UNIQUE,
    city      TEXT NOT NULL,
    country   TEXT NOT NULL,
    joined_on TEXT NOT NULL
);

CREATE TABLE orders (
    id          INTEGER PRIMARY KEY,
    customer_id INTEGER NOT NULL REFERENCES customers(id),
    ordered_at  TEXT    NOT NULL,
    status      TEXT    NOT NULL CHECK (status IN ('pending', 'shipped', 'delivered', 'cancelled'))
);

CREATE TABLE order_items (
    id         INTEGER PRIMARY KEY,
    order_id   INTEGER NOT NULL REFERENCES orders(id),
    product_id INTEGER NOT NULL REFERENCES products(id),
    quantity   INTEGER NOT NULL,
    unit_price NUMERIC NOT NULL
);

INSERT INTO categories (id, name, description) VALUES
    (1, 'Coffee', 'Single-origin and blended coffee beans'),
    (2, 'Tea', 'Loose-leaf tea and tisanes'),
    (3, 'Equipment', 'Brewing gear for the kitchen'),
    (4, 'Accessories', 'Filters, mugs and small extras'),
    (5, 'Gifts', 'Ready-to-give boxes and bundles');

INSERT INTO products (id, name, category_id, price, stock, discontinued) VALUES
    ( 1, 'Ethiopia Yirgacheffe', 1, 18.50,  24, 0),
    ( 2, 'Colombia Huila',       1, 16.00,  40, 0),
    ( 3, 'Sumatra Mandheling',   1, 17.25,   0, 1),
    ( 4, 'Sencha Green',         2, 14.00,  18, 0),
    ( 5, 'Earl Grey',            2, 12.50,  30, 0),
    ( 6, 'Pour-over Kettle',     3, 54.00,   8, 0),
    ( 7, 'Ceramic Dripper',      3, 22.00,  15, 0),
    ( 8, 'Reusable Filter',      4,  9.50,  60, 0),
    ( 9, 'Gift Box',             5, 35.00,  12, 0),
    (10, 'Travel Mug',           4, 19.00,  22, 0);

INSERT INTO customers (id, name, email, city, country, joined_on) VALUES
    (1, 'Ada Lovelace',      'ada@example.com',       'London',     'UK',          '2025-01-12'),
    (2, 'Grace Hopper',      'grace@example.com',     'New York',   'USA',         '2025-02-03'),
    (3, 'Alan Turing',       'alan@example.com',      'Manchester', 'UK',          '2025-02-21'),
    (4, 'Katherine Johnson', 'katherine@example.com', 'Hampton',    'USA',         '2025-03-08'),
    (5, 'Edsger Dijkstra',   'edsger@example.com',    'Rotterdam',  'Netherlands', '2025-03-19'),
    (6, 'Margaret Hamilton', 'margaret@example.com',  'Boston',     'USA',         '2025-04-02'),
    (7, 'Tim Berners-Lee',   'tim@example.com',       'Geneva',     'Switzerland', '2025-04-27'),
    (8, 'Barbara Liskov',    'barbara@example.com',   'Cambridge',  'USA',         '2025-05-15');

INSERT INTO orders (id, customer_id, ordered_at, status) VALUES
    ( 1, 1, '2025-06-03 09:15:00', 'delivered'),
    ( 2, 2, '2025-06-08 14:40:00', 'delivered'),
    ( 3, 1, '2025-06-21 11:05:00', 'shipped'),
    ( 4, 4, '2025-07-02 16:30:00', 'delivered'),
    ( 5, 3, '2025-07-11 08:20:00', 'cancelled'),
    ( 6, 6, '2025-07-19 13:00:00', 'delivered'),
    ( 7, 2, '2025-08-04 10:45:00', 'shipped'),
    ( 8, 8, '2025-08-18 15:10:00', 'delivered'),
    ( 9, 5, '2025-09-01 12:25:00', 'pending'),
    (10, 7, '2025-09-14 09:50:00', 'delivered');

INSERT INTO order_items (id, order_id, product_id, quantity, unit_price) VALUES
    ( 1,  1,  1, 2, 18.50),
    ( 2,  1,  8, 1,  9.50),
    ( 3,  2,  2, 3, 16.00),
    ( 4,  2,  5, 1, 12.50),
    ( 5,  3,  4, 1, 14.00),
    ( 6,  3,  7, 1, 22.00),
    ( 7,  4,  6, 1, 54.00),
    ( 8,  4,  9, 1, 35.00),
    ( 9,  4, 10, 2, 19.00),
    (10,  5,  1, 1, 18.50),
    (11,  6,  2, 2, 16.00),
    (12,  6,  4, 2, 14.00),
    (13,  6,  8, 3,  9.50),
    (14,  7,  5, 2, 12.50),
    (15,  7, 10, 1, 19.00),
    (16,  8,  7, 1, 22.00),
    (17,  8,  9, 1, 35.00),
    (18,  9,  1, 4, 18.50),
    (19, 10,  6, 1, 54.00),
    (20, 10,  8, 2,  9.50);
"""


def main() -> None:
    OUTPUT.unlink(missing_ok=True)
    with sqlite3.connect(OUTPUT) as database:
        database.executescript(SCHEMA)
        database.commit()
    with sqlite3.connect(OUTPUT) as database:
        database.execute("VACUUM")
    print(f"Wrote {OUTPUT}")


if __name__ == "__main__":
    main()
