//! The records every db-conformance runner writes and reads (SCENARIOS.md). The TypeScript runner's types are
//! generated from this file: `ce-wire-gen --records schema/src/lib.rs --lang ts -o sdk/ts`.
use ce_db_core::prelude::*;

/// One item: every field type slice 1 has.
#[derive(Record, Clone, Debug, PartialEq)]
#[db(app = "db-conformance", collection = "items", version = 2)]
pub struct Item {
    #[db(key)]
    pub id: String,
    #[db(max = 200)]
    pub name: String,
    #[db(index)]
    pub bin: String,
    #[db(index, ordered)]
    pub qty: u32,
    pub big: u64,
    pub delta: i64,
    pub ratio: f64,
    pub flag: bool,
    pub tags: Vec<String>,
    pub raw: Hex,
    pub at: Millis,
    #[db(since = 2)]
    pub note: Option<String>,
}

/// The same collection one version later, as a newer program writes it (D3): `colour` is unknown to `Item`.
#[derive(Record, Clone, Debug, PartialEq)]
#[db(app = "db-conformance", collection = "items", version = 3)]
pub struct ItemNext {
    #[db(key)]
    pub id: String,
    #[db(max = 200)]
    pub name: String,
    #[db(index)]
    pub bin: String,
    #[db(index, ordered)]
    pub qty: u32,
    pub big: u64,
    pub delta: i64,
    pub ratio: f64,
    pub flag: bool,
    pub tags: Vec<String>,
    pub raw: Hex,
    pub at: Millis,
    #[db(since = 2)]
    pub note: Option<String>,
    #[db(since = 3)]
    pub colour: Option<String>,
}

/// D1's first golden record (golden/items.json "a"): extremes of every type, `note` absent.
pub fn golden_a(id: &str) -> Item {
    Item {
        id: id.into(),
        name: "Grüße \"q\"\n\u{1}".into(),
        bin: "A4".into(),
        qty: 7,
        big: u64::MAX,
        delta: i64::MIN,
        ratio: 1.5,
        flag: true,
        tags: vec!["x".into(), "y".into()],
        raw: Hex(vec![0, 1, 254, 255]),
        at: Millis(1731576000000),
        note: None,
    }
}

/// D1's second golden record ("b"): an integral float, empty list and bytes, `note` present.
pub fn golden_b(id: &str) -> Item {
    Item { id: id.into(), name: String::new(), bin: "B".into(), qty: 0, big: 0, delta: 0, ratio: 100.0, flag: false, tags: vec![], raw: Hex(vec![]), at: Millis(0), note: Some("n".into()) }
}

/// A plain item for the other scenarios.
pub fn item(id: &str, bin: &str, qty: u32) -> Item {
    Item { id: id.into(), name: format!("item {id}"), bin: bin.into(), qty, big: 1, delta: -1, ratio: 0.25, flag: false, tags: vec![], raw: Hex(vec![]), at: Millis(1), note: None }
}
