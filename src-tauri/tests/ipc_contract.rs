//! This fixture is also consumed by the frontend; either side changing the wire
//! contract must update and review the same JSON.
use faro_lib::error::FaroError;
use faro_lib::model::{ColumnInfo, PendingChange, QueryOutcome, ResultSet, Value};
use serde_json::json;

#[test]
fn serialization_matches_frontend_fixture() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/ipc/fixtures/contract.json")).unwrap();
    let result = QueryOutcome::Rows(ResultSet {
        columns: ["id", "payload", "blob", "items", "missing"]
            .into_iter()
            .map(|name| ColumnInfo {
                name: name.into(),
                type_name: "fixture".into(),
            })
            .collect(),
        rows: vec![vec![
            Value::Int(i64::MAX),
            Value::Bytes(vec![0, 255]),
            Value::Bytes(vec![42; 257]),
            Value::Array(vec![Value::Null, Value::Decimal("123.4500".into())]),
            Value::Null,
        ]],
        truncated: true,
        elapsed_ms: 7,
    });
    assert_eq!(serde_json::to_value(result).unwrap(), fixture["result"]);
    let errors = [
        FaroError::Conflict("Row changed".into()),
        FaroError::ResourceLimit("Result too large".into()),
        FaroError::Database {
            message: "Missing table".into(),
            code: Some("42P01".into()),
        },
    ];
    assert_eq!(json!(errors), fixture["errors"]);
    let changes: Vec<PendingChange> = serde_json::from_value(fixture["changes"].clone()).unwrap();
    assert_eq!(serde_json::to_value(changes).unwrap(), fixture["changes"]);
}
