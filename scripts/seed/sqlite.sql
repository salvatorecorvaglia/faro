-- Fixture schema for Faro's SQLite tests.
--
-- Mirrors the Postgres fixture where SQLite allows it, and adds SQLite's own
-- quirks: dynamic typing, a rowid table without an explicit primary key, and
-- values stored in a column whose declared type does not match.

DROP TABLE IF EXISTS book_stores;
DROP TABLE IF EXISTS books;
DROP TABLE IF EXISTS authors;
DROP TABLE IF EXISTS access_log;
DROP TABLE IF EXISTS type_gallery;
DROP VIEW IF EXISTS books_with_authors;

CREATE TABLE authors (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    email       TEXT UNIQUE,
    bio         TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE books (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    author_id   INTEGER NOT NULL REFERENCES authors(id) ON DELETE CASCADE,
    title       TEXT NOT NULL,
    isbn        TEXT,
    price       REAL,
    published   TEXT,
    in_stock    INTEGER NOT NULL DEFAULT 1,
    metadata    TEXT,
    cover       BLOB
);

CREATE INDEX books_author_idx ON books(author_id);

CREATE TABLE book_stores (
    book_id     INTEGER NOT NULL REFERENCES books(id),
    store_id    INTEGER NOT NULL,
    quantity    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (book_id, store_id)
);

-- No primary key: exercises the read-only grid path.
CREATE TABLE access_log (
    ts          TEXT,
    path        TEXT,
    status      INTEGER,
    duration_ms REAL
);

-- SQLite has no exact decimal type. A value written into a NUMERIC column is
-- coerced to REAL by type affinity and loses precision *on insert* — before
-- Faro ever sees it. The only way to keep exact digits is a TEXT column, so
-- the fixture carries both to document the difference.
CREATE TABLE type_gallery (
    id            INTEGER PRIMARY KEY,
    a_int         INTEGER,
    a_real        REAL,
    a_text        TEXT,
    a_blob        BLOB,
    a_numeric     NUMERIC,   -- will be coerced to REAL
    a_decimal_txt TEXT,      -- keeps full precision
    a_bool        BOOLEAN,
    a_null        TEXT
);

CREATE VIEW books_with_authors AS
SELECT b.id, b.title, b.price, a.name AS author
FROM books b JOIN authors a ON a.id = b.author_id;

INSERT INTO authors (name, email, bio) VALUES
    ('Ada Lovelace',      'ada@example.com',    'Wrote the first algorithm.'),
    ('Grace Hopper',      'grace@example.com',  'Coined the term "debugging".'),
    ('Ursula K. Le Guin', 'ursula@example.com', NULL),
    ('Ken O''Brien',      'ken@example.com',    'Name contains an apostrophe.'),
    ('Ólafur Þórðarson',  'olafur@example.com', 'Unicode: þæö ÞÆÖ 日本語 🎉');

INSERT INTO books (author_id, title, isbn, price, published, metadata) VALUES
    (1, 'Notes on the Analytical Engine', '9780000000001', 42.50, '1843-10-01', '{"rare": true}'),
    (2, 'Compiling for Humans',           '9780000000002', 31.00, '1952-06-15', '{"edition": 2}'),
    (3, 'The Dispossessed',               '9780000000003', 18.99, '1974-01-01', NULL),
    (3, 'A Wizard of Earthsea',           '9780000000004', 15.25, '1968-01-01', NULL),
    (4, 'Quotes ''n'' Things',            NULL,            NULL,  NULL,         NULL);

INSERT INTO book_stores (book_id, store_id, quantity) VALUES
    (1, 1, 3), (1, 2, 0), (2, 1, 7), (3, 1, 12), (3, 2, 5);

INSERT INTO type_gallery
    (a_int, a_real, a_text, a_blob, a_numeric, a_decimal_txt, a_bool, a_null)
VALUES
    (9223372036854775807, 2.718281828459045, 'hello 🎉', X'deadbeef',
     '12345678901234567890.0987654321',
     '12345678901234567890.0987654321', 1, NULL),
    (NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);

-- Enough rows to page through.
WITH RECURSIVE seq(n) AS (
    SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 5000
)
INSERT INTO access_log (ts, path, status, duration_ms)
SELECT datetime('now'), '/page/' || n, 200 + (n % 5), abs(random() % 500)
FROM seq;
