-- Fixture schema for Faro's ClickHouse tests.
--
-- ClickHouse is not a row store, so this mirrors the others' *shape* rather than
-- their DDL: MergeTree tables with sorting keys, no foreign keys, and no unique
-- constraints. That difference is the point — Faro must present a ClickHouse
-- table as read-only, because a MergeTree primary key is a sparse index rather
-- than a unique constraint and cannot identify a single row.

DROP TABLE IF EXISTS book_stores;
DROP TABLE IF EXISTS books;
DROP TABLE IF EXISTS authors;
DROP TABLE IF EXISTS access_log;
DROP TABLE IF EXISTS type_gallery;
DROP VIEW IF EXISTS books_with_authors;

CREATE TABLE authors (
    id          Int32,
    name        String,
    email       Nullable(String),
    bio         Nullable(String),
    created_at  DateTime
) ENGINE = MergeTree ORDER BY id;

CREATE TABLE books (
    id          Int32,
    author_id   Int32,
    title       String,
    isbn        Nullable(String),
    price       Nullable(Decimal(10, 2)),
    published   Nullable(Date),
    in_stock    Bool,
    metadata    Nullable(String)
) ENGINE = MergeTree ORDER BY id;

-- A compound sorting key, which Faro surfaces as an index and never as a
-- primary key.
CREATE TABLE book_stores (
    book_id     Int32,
    store_id    Int32,
    quantity    Int32
) ENGINE = MergeTree ORDER BY (book_id, store_id);

CREATE TABLE access_log (
    ts          DateTime,
    path        String,
    status      Int16,
    duration_ms Float64
) ENGINE = MergeTree ORDER BY ts;

CREATE TABLE type_gallery (
    id          Int32,
    a_int8      Int8,
    a_int16     Int16,
    a_int32     Int32,
    a_int64     Int64,
    -- Both exceed i64: decoding must keep them exact rather than wrapping.
    a_uint64    UInt64,
    a_int128    Int128,
    a_float32   Float32,
    a_float64   Float64,
    a_decimal   Decimal(38, 10),
    a_bool      Bool,
    a_date      Date,
    a_datetime  DateTime,
    a_uuid      UUID,
    a_string    String,
    a_nullable  Nullable(String),
    a_array     Array(Int32),
    a_enum      Enum8('small' = 1, 'medium' = 2, 'large' = 3)
) ENGINE = MergeTree ORDER BY id;

CREATE VIEW books_with_authors AS
SELECT b.id, b.title, b.price, a.name AS author
FROM books b JOIN authors a ON a.id = b.author_id;

INSERT INTO authors (id, name, email, bio, created_at) VALUES
    (1, 'Ada Lovelace',      'ada@example.com',    'Wrote the first algorithm.',   '2026-01-01 10:00:00'),
    (2, 'Grace Hopper',      'grace@example.com',  'Coined the term "debugging".', '2026-01-02 10:00:00'),
    (3, 'Ursula K. Le Guin', 'ursula@example.com', NULL,                           '2026-01-03 10:00:00'),
    (4, 'Ken O\'Brien',      'ken@example.com',    'Name contains an apostrophe.', '2026-01-04 10:00:00'),
    (5, 'Ólafur Þórðarson',  'olafur@example.com', 'Unicode: þæö ÞÆÖ 日本語 🎉',    '2026-01-05 10:00:00');

INSERT INTO books (id, author_id, title, isbn, price, published, in_stock, metadata) VALUES
    (1, 1, 'Notes on the Analytical Engine', '9780000000001', 42.50, '1843-10-01', true, '{"rare": true}'),
    (2, 2, 'Compiling for Humans',           '9780000000002', 31.00, '1952-06-15', true, '{"edition": 2}'),
    (3, 3, 'The Dispossessed',               '9780000000003', 18.99, '1974-01-01', true, NULL),
    (4, 3, 'A Wizard of Earthsea',           '9780000000004', 15.25, '1968-01-01', true, NULL),
    (5, 4, 'Quotes \'n\' Things',            NULL,            NULL,  NULL,         false, NULL);

INSERT INTO book_stores (book_id, store_id, quantity) VALUES
    (1, 1, 3), (1, 2, 0), (2, 1, 7), (3, 1, 12), (3, 2, 5);

INSERT INTO type_gallery (
    id, a_int8, a_int16, a_int32, a_int64, a_uint64, a_int128,
    a_float32, a_float64, a_decimal, a_bool,
    a_date, a_datetime, a_uuid, a_string, a_nullable, a_array, a_enum
) VALUES (
    1, 127, 32767, 2147483647, 9223372036854775807,
    18446744073709551615,
    170141183460469231731687303715884105727,
    3.14, 2.718281828459045,
    12345678901234567890.0987654321,
    true, '2026-08-05', '2026-08-05 13:45:00',
    '0192e8f0-1234-5678-9abc-def012345678',
    'hello 🎉', NULL, [1, 2, 3], 'medium'
);

-- Enough rows to page through and to exercise the truncation indicator.
INSERT INTO access_log (ts, path, status, duration_ms)
SELECT
    toDateTime('2026-08-05 12:00:00'),
    concat('/page/', toString(number + 1)),
    toInt16(200 + (number % 5)),
    rand() / 1000000
FROM numbers(5000);
