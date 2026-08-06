// Fixture data for Faro's MongoDB tests.
//
// Mirrors the other engines' shape as collections, plus the things that only a
// document store can do and that Faro must therefore handle: fields absent from
// some documents, a field holding more than one BSON type, nested documents,
// arrays, and a Decimal128 past f64's exact range.

const db = db.getSiblingDB('faro_test');

db.authors.drop();
db.books.drop();
db.book_stores.drop();
db.access_log.drop();
db.type_gallery.drop();

db.authors.insertMany([
  { _id: 1, name: 'Ada Lovelace', email: 'ada@example.com', bio: 'Wrote the first algorithm.' },
  { _id: 2, name: 'Grace Hopper', email: 'grace@example.com', bio: 'Coined the term "debugging".' },
  // bio explicitly null, distinct from the field being absent below.
  { _id: 3, name: 'Ursula K. Le Guin', email: 'ursula@example.com', bio: null },
  { _id: 4, name: "Ken O'Brien", email: 'ken@example.com', bio: 'Name contains an apostrophe.' },
  {
    _id: 5,
    name: 'Ólafur Þórðarson',
    email: 'olafur@example.com',
    bio: 'Unicode: þæö ÞÆÖ 日本語 🎉',
    // Present on this document only: field inference must still surface it.
    nickname: 'Óli',
  },
]);

db.books.insertMany([
  {
    _id: 1,
    author_id: 1,
    title: 'Notes on the Analytical Engine',
    isbn: '9780000000001',
    price: NumberDecimal('42.50'),
    in_stock: true,
    // Nested document and array, which the grid shows collapsed and the JSON
    // view shows in full.
    metadata: { rare: true, tags: ['history', 'computing'] },
  },
  {
    _id: 2,
    author_id: 2,
    title: 'Compiling for Humans',
    isbn: '9780000000002',
    price: NumberDecimal('31.00'),
    in_stock: true,
    metadata: { edition: 2 },
  },
  { _id: 3, author_id: 3, title: 'The Dispossessed', price: NumberDecimal('18.99'), in_stock: true },
  { _id: 4, author_id: 3, title: 'A Wizard of Earthsea', price: NumberDecimal('15.25'), in_stock: true },
  { _id: 5, author_id: 4, title: "Quotes 'n' Things", in_stock: false },
]);

db.book_stores.insertMany([
  { book_id: 1, store_id: 1, quantity: 3 },
  { book_id: 1, store_id: 2, quantity: 0 },
  { book_id: 2, store_id: 1, quantity: 7 },
  { book_id: 3, store_id: 1, quantity: 12 },
  { book_id: 3, store_id: 2, quantity: 5 },
]);

db.type_gallery.insertMany([
  {
    _id: 1,
    a_int: NumberInt(2147483647),
    a_long: NumberLong('9223372036854775807'),
    a_double: 2.718281828459045,
    // Past f64's exact range: must survive as text, not be rounded.
    a_decimal: NumberDecimal('12345678901234567890.0987654321'),
    a_bool: true,
    a_date: new Date('2026-08-05T13:45:00Z'),
    a_string: 'hello 🎉',
    a_null: null,
    a_array: [1, 2, 3],
    a_object: { nested: { deep: 'value' } },
    a_binary: new BinData(0, 'deadbeef'),
    a_objectid: new ObjectId('0192e8f00000000000000000'),
  },
  // A second document where a_string holds a number instead: field inference
  // must report both types rather than picking one.
  { _id: 2, a_string: 42 },
]);

// Enough documents to page through and to exercise the truncation indicator.
const logs = [];
for (let i = 1; i <= 5000; i++) {
  logs.push({
    ts: new Date('2026-08-05T12:00:00Z'),
    path: '/page/' + i,
    status: 200 + (i % 5),
    duration_ms: Math.random() * 500,
  });
}
db.access_log.insertMany(logs);

// An index Faro should surface as a real one, alongside the implicit _id index.
db.books.createIndex({ title: 1 }, { name: 'books_title_idx' });

print('seeded faro_test');
