//! How a table page is selected: sort key, column filters, and paging.

/// Sort/filter descriptor for table browsing.
///
/// These are pushed down into SQL rather than applied to the fetched page,
/// because sorting 1000 fetched rows of a 10-million-row table would show the
/// wrong rows entirely.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseOptions {
    #[serde(default)]
    pub sort_column: Option<String>,
    #[serde(default)]
    pub sort_desc: bool,
    /// Column filters, ANDed together.
    ///
    /// `column` is checked against the table's real column list and dropped if
    /// it does not match; `value` is rendered as a literal by the dialect,
    /// which is responsible for escaping it for that engine. Neither is
    /// concatenated raw.
    #[serde(default)]
    pub filters: Vec<ColumnFilter>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub offset: u64,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnFilter {
    pub column: String,
    pub op: FilterOp,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum FilterOp {
    Equals,
    NotEquals,
    Contains,
    StartsWith,
    GreaterThan,
    LessThan,
    IsNull,
    IsNotNull,
}
