-- Fixture schema for Faro's DuckDB tests.
--
-- Mirrors the SQLite fixture, plus the types DuckDB has that SQLite does not:
-- a real DECIMAL, HUGEINT and UBIGINT past i64, and a genuine BOOLEAN.

CREATE TABLE authors (
    id          INTEGER PRIMARY KEY,
    name        VARCHAR NOT NULL,
    email       VARCHAR UNIQUE,
    bio         VARCHAR,
    created_at  TIMESTAMP
);

CREATE TABLE books (
    id          INTEGER PRIMARY KEY,
    author_id   INTEGER NOT NULL,
    title       VARCHAR NOT NULL,
    isbn        VARCHAR,
    price       DECIMAL(10, 2),
    published   DATE,
    in_stock    BOOLEAN NOT NULL DEFAULT TRUE,
    metadata    VARCHAR,
    cover       BLOB
);

-- Composite primary key.
CREATE TABLE book_stores (
    book_id     INTEGER NOT NULL,
    store_id    INTEGER NOT NULL,
    quantity    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (book_id, store_id)
);

-- No primary key: the editable grid must fall back to read-only.
CREATE TABLE access_log (
    ts          TIMESTAMP,
    path        VARCHAR,
    status      SMALLINT,
    duration_ms DOUBLE
);

CREATE TABLE type_gallery (
    id          INTEGER PRIMARY KEY,
    a_tinyint   TINYINT,
    a_smallint  SMALLINT,
    a_int       INTEGER,
    a_bigint    BIGINT,
    -- Both exceed i64 on purpose: decoding must keep them exact rather than
    -- wrapping negative or rounding through a float.
    a_ubigint   UBIGINT,
    a_hugeint   HUGEINT,
    a_float     FLOAT,
    a_double    DOUBLE,
    a_decimal   DECIMAL(30, 10),
    a_bool      BOOLEAN,
    a_date      DATE,
    a_time      TIME,
    a_timestamp TIMESTAMP,
    a_varchar   VARCHAR,
    a_blob      BLOB
);

CREATE VIEW books_with_authors AS
SELECT b.id, b.title, b.price, a.name AS author
FROM books b JOIN authors a ON a.id = b.author_id;

INSERT INTO authors (id, name, email, bio, created_at) VALUES
    (1, 'Ada Lovelace',      'ada@example.com',    'Wrote the first algorithm.',   '2026-01-01 10:00:00'),
    (2, 'Grace Hopper',      'grace@example.com',  'Coined the term "debugging".', '2026-01-02 10:00:00'),
    (3, 'Ursula K. Le Guin', 'ursula@example.com', NULL,                           '2026-01-03 10:00:00'),
    (4, 'Ken O''Brien',      'ken@example.com',    'Name contains an apostrophe.', '2026-01-04 10:00:00'),
    (5, 'Ólafur Þórðarson',  'olafur@example.com', 'Unicode: þæö ÞÆÖ 日本語 🎉',    '2026-01-05 10:00:00');

INSERT INTO books (id, author_id, title, isbn, price, published, metadata) VALUES
    (1, 1, 'Notes on the Analytical Engine', '9780000000001', 42.50, '1843-10-01', '{"rare": true}'),
    (2, 2, 'Compiling for Humans',           '9780000000002', 31.00, '1952-06-15', '{"edition": 2}'),
    (3, 3, 'The Dispossessed',               '9780000000003', 18.99, '1974-01-01', NULL),
    (4, 3, 'A Wizard of Earthsea',           '9780000000004', 15.25, '1968-01-01', NULL),
    (5, 4, 'Quotes ''n'' Things',            NULL,            NULL,  NULL,         NULL);

INSERT INTO book_stores (book_id, store_id, quantity) VALUES
    (1, 1, 3), (1, 2, 0), (2, 1, 7), (3, 1, 12), (3, 2, 5);

INSERT INTO type_gallery (
    id, a_tinyint, a_smallint, a_int, a_bigint, a_ubigint, a_hugeint,
    a_float, a_double, a_decimal, a_bool,
    a_date, a_time, a_timestamp, a_varchar, a_blob
) VALUES (
    1, 127, 32767, 2147483647, 9223372036854775807,
    18446744073709551615,
    170141183460469231731687303715884105727,
    3.14, 2.718281828459045,
    12345678901234567890.0987654321,
    TRUE, '2026-08-05', '13:45:00', '2026-08-05 13:45:00',
    'hello 🎉', '\xDE\xAD\xBE\xEF'::BLOB
), (
    2, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
    NULL, NULL, NULL, NULL, NULL
);

-- Enough rows to page through and to exercise the truncation indicator.
INSERT INTO access_log (ts, path, status, duration_ms)
SELECT
    '2026-08-05 12:00:00'::TIMESTAMP,
    '/page/' || i,
    200 + (i % 5),
    random() * 500
FROM range(1, 5001) t(i);
