-- Fixture schema for Faro's SQL Server tests.
--
-- Mirrors the Postgres fixture with T-SQL spellings, plus the cases that catch
-- drivers out: NVARCHAR unicode, a table with no primary key, a composite key,
-- MONEY and DECIMAL that must not pass through a float, and VARBINARY.

IF DB_ID('faro_test') IS NULL
    CREATE DATABASE faro_test;
GO

USE faro_test;
GO

-- Dropped child-first so the foreign keys do not block it.
DROP VIEW IF EXISTS books_with_authors;
DROP TABLE IF EXISTS book_stores;
DROP TABLE IF EXISTS books;
DROP TABLE IF EXISTS authors;
DROP TABLE IF EXISTS access_log;
DROP TABLE IF EXISTS type_gallery;
GO

CREATE TABLE authors (
    id          INT IDENTITY(1,1) PRIMARY KEY,
    name        NVARCHAR(200) NOT NULL,
    email       NVARCHAR(200) UNIQUE,
    bio         NVARCHAR(MAX),
    created_at  DATETIME2 NOT NULL DEFAULT SYSUTCDATETIME()
);

CREATE TABLE books (
    id          INT IDENTITY(1,1) PRIMARY KEY,
    author_id   INT NOT NULL,
    title       NVARCHAR(200) NOT NULL,
    isbn        CHAR(13),
    price       DECIMAL(10, 2),
    published   DATE,
    in_stock    BIT NOT NULL DEFAULT 1,
    metadata    NVARCHAR(MAX),
    cover       VARBINARY(MAX),
    CONSTRAINT books_author_fk FOREIGN KEY (author_id)
        REFERENCES authors(id) ON DELETE CASCADE
);

CREATE INDEX books_title_idx ON books(title);

-- Composite primary key.
CREATE TABLE book_stores (
    book_id     INT NOT NULL,
    store_id    INT NOT NULL,
    quantity    INT NOT NULL DEFAULT 0,
    CONSTRAINT book_stores_pk PRIMARY KEY (book_id, store_id),
    CONSTRAINT book_stores_book_fk FOREIGN KEY (book_id) REFERENCES books(id)
);

-- No primary key: the editable grid must fall back to read-only.
CREATE TABLE access_log (
    ts          DATETIME2,
    path        NVARCHAR(255),
    status      SMALLINT,
    duration_ms FLOAT
);

CREATE TABLE type_gallery (
    id            INT IDENTITY(1,1) PRIMARY KEY,
    a_tinyint     TINYINT,
    a_smallint    SMALLINT,
    a_int         INT,
    a_bigint      BIGINT,
    a_float       REAL,
    a_double      FLOAT,
    -- Neither of these may be routed through f64.
    a_decimal     DECIMAL(30, 10),
    a_money       MONEY,
    a_bit         BIT,
    a_date        DATE,
    a_time        TIME,
    a_datetime2   DATETIME2,
    a_datetimeoff DATETIMEOFFSET,
    a_char        CHAR(10),
    a_nvarchar    NVARCHAR(100),
    a_uuid        UNIQUEIDENTIFIER,
    a_binary      VARBINARY(MAX),
    a_xml         XML
);
GO

CREATE VIEW books_with_authors AS
SELECT b.id, b.title, b.price, a.name AS author
FROM books b JOIN authors a ON a.id = b.author_id;
GO

-- N'' literals throughout: without the prefix SQL Server narrows the text to
-- the database collation's code page and the unicode row is silently mangled.
SET IDENTITY_INSERT authors ON;
INSERT INTO authors (id, name, email, bio) VALUES
    (1, N'Ada Lovelace',      N'ada@example.com',    N'Wrote the first algorithm.'),
    (2, N'Grace Hopper',      N'grace@example.com',  N'Coined the term "debugging".'),
    (3, N'Ursula K. Le Guin', N'ursula@example.com', NULL),
    (4, N'Ken O''Brien',      N'ken@example.com',    N'Name contains an apostrophe.'),
    (5, N'Ólafur Þórðarson',  N'olafur@example.com', N'Unicode: þæö ÞÆÖ 日本語 🎉');
SET IDENTITY_INSERT authors OFF;
GO

SET IDENTITY_INSERT books ON;
INSERT INTO books (id, author_id, title, isbn, price, published, metadata) VALUES
    (1, 1, N'Notes on the Analytical Engine', '9780000000001', 42.50, '1843-10-01', N'{"rare": true}'),
    (2, 2, N'Compiling for Humans',           '9780000000002', 31.00, '1952-06-15', N'{"edition": 2}'),
    (3, 3, N'The Dispossessed',               '9780000000003', 18.99, '1974-01-01', NULL),
    (4, 3, N'A Wizard of Earthsea',           '9780000000004', 15.25, '1968-01-01', NULL),
    (5, 4, N'Quotes ''n'' Things',            NULL,            NULL,  NULL,         NULL);
SET IDENTITY_INSERT books OFF;
GO

INSERT INTO book_stores (book_id, store_id, quantity) VALUES
    (1, 1, 3), (1, 2, 0), (2, 1, 7), (3, 1, 12), (3, 2, 5);
GO

SET IDENTITY_INSERT type_gallery ON;
INSERT INTO type_gallery (
    id, a_tinyint, a_smallint, a_int, a_bigint, a_float, a_double,
    a_decimal, a_money, a_bit, a_date, a_time, a_datetime2, a_datetimeoff,
    a_char, a_nvarchar, a_uuid, a_binary, a_xml
) VALUES (
    1, 255, 32767, 2147483647, 9223372036854775807, 3.14, 2.718281828459045,
    -- Past f64's exact range on purpose.
    12345678901234567890.0987654321,
    922337203685477.5807,
    1, '2026-08-05', '13:45:00', '2026-08-05 13:45:00', '2026-08-05 13:45:00 +02:00',
    'fixed', N'variable 🎉', NEWID(), 0xDEADBEEF, N'<root><a>1</a></root>'
), (
    2, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
    NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL
);
SET IDENTITY_INSERT type_gallery OFF;
GO

-- Enough rows to page through and to exercise the truncation indicator.
-- Built from sys.all_objects, which every instance has plenty of.
INSERT INTO access_log (ts, path, status, duration_ms)
SELECT TOP 5000
    SYSUTCDATETIME(),
    N'/page/' + CAST(ROW_NUMBER() OVER (ORDER BY a.object_id, b.object_id) AS NVARCHAR(20)),
    200 + (ROW_NUMBER() OVER (ORDER BY a.object_id, b.object_id) % 5),
    RAND(CHECKSUM(NEWID())) * 500
FROM sys.all_objects a CROSS JOIN sys.all_objects b;
GO
