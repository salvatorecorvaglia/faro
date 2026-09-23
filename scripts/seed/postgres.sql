-- Fixture schema for Faro's Postgres tests.
--
-- Deliberately includes the cases that break naive SQL clients: exotic types,
-- a table with no primary key, a composite primary key, NULLs, unicode,
-- embedded quotes, a view, and a table large enough to exercise paging.

DROP SCHEMA IF EXISTS public CASCADE;
CREATE SCHEMA public;

CREATE TABLE authors (
    id          serial PRIMARY KEY,
    name        text NOT NULL,
    email       text UNIQUE,
    bio         text,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE books (
    id          serial PRIMARY KEY,
    author_id   integer NOT NULL REFERENCES authors(id) ON DELETE CASCADE,
    title       varchar(200) NOT NULL,
    isbn        char(13),
    price       numeric(10, 2),
    published   date,
    in_stock    boolean NOT NULL DEFAULT true,
    tags        text[],
    metadata    jsonb,
    cover       bytea
);

CREATE INDEX books_author_idx ON books(author_id);
CREATE INDEX books_title_idx ON books(title);

-- Composite primary key: the editable grid must handle multi-column identity.
CREATE TABLE book_stores (
    book_id     integer NOT NULL REFERENCES books(id),
    store_id    integer NOT NULL,
    quantity    integer NOT NULL DEFAULT 0,
    PRIMARY KEY (book_id, store_id)
);

-- No primary key: the grid must fall back to read-only rather than risk an
-- UPDATE that hits more than one row.
CREATE TABLE access_log (
    ts          timestamptz NOT NULL DEFAULT now(),
    path        text,
    status      smallint,
    duration_ms double precision
);

-- Wide type coverage, to check the decoder and the "unsupported" fallback.
CREATE TABLE type_gallery (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    a_smallint  smallint,
    a_int       integer,
    a_bigint    bigint,
    a_real      real,
    a_double    double precision,
    a_numeric   numeric(30, 10),
    a_bool      boolean,
    a_date      date,
    a_time      time,
    a_timestamp timestamp,
    a_tstz      timestamptz,
    a_interval  interval,
    a_json      json,
    a_jsonb     jsonb,
    a_bytea     bytea,
    a_inet      inet,
    a_int_arr   integer[],
    a_text_arr  text[]
);

CREATE VIEW books_with_authors AS
SELECT b.id, b.title, b.price, a.name AS author
FROM books b
JOIN authors a ON a.id = b.author_id;

INSERT INTO authors (name, email, bio) VALUES
    ('Ada Lovelace',        'ada@example.com',  'Wrote the first algorithm.'),
    ('Grace Hopper',        'grace@example.com', 'Coined the term "debugging".'),
    ('Ursula K. Le Guin',   'ursula@example.com', NULL),
    ('Ken O''Brien',        'ken@example.com',  'Name contains an apostrophe.'),
    ('Ólafur Þórðarson',    'olafur@example.com', 'Unicode: þæö ÞÆÖ 日本語 🎉');

INSERT INTO books (author_id, title, isbn, price, published, tags, metadata) VALUES
    (1, 'Notes on the Analytical Engine', '9780000000001', 42.50,  '1843-10-01',
     ARRAY['history','computing'], '{"rare": true, "pages": 120}'),
    (2, 'Compiling for Humans',           '9780000000002', 31.00,  '1952-06-15',
     ARRAY['compilers'],            '{"edition": 2}'),
    (3, 'The Dispossessed',               '9780000000003', 18.99,  '1974-01-01',
     ARRAY['fiction','scifi'],      '{"awards": ["Hugo","Nebula"]}'),
    (3, 'A Wizard of Earthsea',           '9780000000004', 15.25,  '1968-01-01',
     ARRAY['fiction','fantasy'],    NULL),
    (4, 'Quotes ''n'' Things',            NULL,            NULL,   NULL,
     NULL,                          NULL);

INSERT INTO book_stores (book_id, store_id, quantity) VALUES
    (1, 1, 3), (1, 2, 0), (2, 1, 7), (3, 1, 12), (3, 2, 5);

INSERT INTO type_gallery (
    a_smallint, a_int, a_bigint, a_real, a_double, a_numeric, a_bool,
    a_date, a_time, a_timestamp, a_tstz, a_interval,
    a_json, a_jsonb, a_bytea, a_inet, a_int_arr, a_text_arr
) VALUES (
    32767, 2147483647, 9223372036854775807, 3.14, 2.718281828459045,
    -- Beyond f64 precision on purpose: this must survive as an exact string.
    12345678901234567890.0987654321,
    true, '2026-08-05', '13:45:00', '2026-08-05 13:45:00', '2026-08-05 13:45:00+02',
    '3 days 4 hours',
    '{"k": "v"}', '{"k": "v"}', '\xdeadbeef', '192.168.1.1',
    ARRAY[1, 2, 3], ARRAY['a', 'b']
), (
    NULL, NULL, NULL, NULL, NULL, NULL, NULL,
    NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL
);

-- Enough rows to exercise pagination and the truncation indicator.
INSERT INTO access_log (path, status, duration_ms)
SELECT '/page/' || g, 200 + (g % 5), random() * 500
FROM generate_series(1, 5000) g;
