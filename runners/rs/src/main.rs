//! The ce-db-rs runner of db-conformance (SCENARIOS.md). Drives only ce-db's public API.
//!
//! `conf-rs run --store ADDR --cap CAP --cap-ro CAP --cap-items CAP --cap-other CAP [--tag T]`: D1..D7, D9, D10, D12,
//!   D13, one `CONF <id> PASS|FAIL <why>` each.
//! `conf-rs d8-watch --store ADDR --cap CAP --key K`: prints `READY` once watching, then `CONF D8 PASS|FAIL`.
//! `conf-rs d8-write --store ADDR --cap CAP --key K`: writes golden_a(K) and exits.
//! `conf-rs d11-watch --store ADDR --cap CAP --tag T`: writes b and c, watches the prefix, prints `READY`, then
//!   `CONF D11 PASS|FAIL` once the store went away, came back and the missed changes arrived.
//! `conf-rs d11-write --store ADDR --cap CAP --tag T`: the changes the watcher misses (insert a, update b, remove c).
use ce_db::prelude::*;
use ce_db::{Change, Collection, Db, Link};
use conf_schema::*;
use std::collections::HashSet;
use std::time::{Duration, Instant};

const GOLDEN: &str = include_str!("../../../golden/items.json");

type R = Result<(), String>;

fn ensure(c: bool, why: impl Into<String>) -> R {
    if c {
        Ok(())
    } else {
        Err(why.into())
    }
}

fn e2s(e: DbError) -> String {
    format!("{e:?}")
}

struct Ctx {
    db: Db,
    store: String,
    cap: String,
    cap_ro: String,
    /// read and write on `items` only (D12)
    cap_items: String,
    /// every right of another app (D13)
    cap_other: String,
    tag: String,
}

impl Ctx {
    fn items(&self) -> Result<Collection<Item>, String> {
        self.db.collection::<Item>().map_err(e2s)
    }
    fn k(&self, s: &str) -> String {
        format!("{s}-{}", self.tag)
    }
}

fn d1(_: &Ctx) -> R {
    let g: serde_json::Value = serde_json::from_str(GOLDEN).map_err(|e| e.to_string())?;
    for (name, rec) in [("a", golden_a("g-a")), ("b", golden_b("g-b"))] {
        let want = g[name].as_str().ok_or("golden file")?;
        let got = ce_db::encode(&rec).map_err(e2s)?;
        ensure(got == want, format!("{name}: encoded {got} want {want}"))?;
        let back = ce_db::decode::<Item>(want).map_err(e2s)?;
        ensure(back.value == rec && back.extra.is_empty() && back.writer_version == 2, format!("{name}: decoded {:?}", back.value))?;
    }
    Ok(())
}

fn d2(c: &Ctx) -> R {
    let items = c.items()?;
    let id = c.k("d2");
    let a = item(&id, "d2", 3);
    ensure(items.insert(&a).map_err(e2s)? == id, "insert returns the key")?;
    ensure(items.get(&id).map_err(e2s)? == Some(a.clone()), "get after insert")?;
    ensure(items.insert(&a) == Err(DbError::Conflict { held_version: 1 }), "a second insert is Conflict { held_version: 1 }")?;
    let u = items.update(&id, |x| x.qty = 4).map_err(e2s)?;
    ensure(u.qty == 4 && items.get(&id).map_err(e2s)?.map(|x| x.qty) == Some(4), "update")?;
    ensure(items.update(c.k("d2-none"), |x| x.qty = 0) == Err(DbError::NotFound { key: c.k("d2-none") }), "update of a missing key is NotFound")?;
    let mut s = items.get_stored(&id).map_err(e2s)?.ok_or("stored")?;
    ensure(s.version == 2, format!("version 2 after insert+update, got {}", s.version))?;
    s.version = 1;
    ensure(items.replace_stored(&s) == Err(DbError::Conflict { held_version: 2 }), "a stale compare-and-set is Conflict { held_version: 2 }")?;
    items.remove(&id).map_err(e2s)?;
    ensure(items.get(&id).map_err(e2s)?.is_none(), "get after remove is None")
}

fn d3(c: &Ctx) -> R {
    let next = c.db.collection::<ItemNext>().map_err(e2s)?;
    let id = c.k("d3");
    let b = item(&id, "d3", 1);
    let n = ItemNext { id: id.clone(), name: b.name.clone(), bin: b.bin.clone(), qty: 1, big: 1, delta: -1, ratio: 0.25, flag: false, tags: vec![], raw: Hex(vec![]), at: Millis(1), note: None, colour: Some("red".into()) };
    next.insert(&n).map_err(e2s)?;
    // an older program (Item, version 2) opens the collection registered at version 3: allowed, and it updates
    let items = c.items()?;
    let s = items.get_stored(&id).map_err(e2s)?.ok_or("row")?;
    ensure(s.extra.get("colour").and_then(|v| v.as_str()) == Some("red"), "the unknown field is in extra")?;
    items.update(&id, |x| x.qty = 5).map_err(e2s)?;
    let after = next.get(&id).map_err(e2s)?.ok_or("row")?;
    ensure(after.qty == 5 && after.colour.as_deref() == Some("red"), format!("after the old reader's update: qty {} colour {:?}", after.qty, after.colour))
}

fn d4(c: &Ctx) -> R {
    let items = c.items()?;
    let bin = c.k("d4");
    let mut want: Vec<String> = Vec::new();
    for i in 0..12 {
        let id = c.k(&format!("d4/{i:02}"));
        let b = if i % 3 == 0 { bin.clone() } else { c.k("d4-other") };
        if b == bin {
            want.push(id.clone());
        }
        items.insert(&item(&id, &b, i)).map_err(e2s)?;
    }
    let got: Vec<String> = items.by_bin(bin.clone()).fetch().map_err(e2s)?.into_iter().map(|x| x.id).collect();
    ensure(got == want, format!("by_bin: {got:?} want {want:?}"))?;
    ensure(items.by_bin(bin).count().map_err(e2s)? == 4, "count")?;
    ensure(items.by_bin(c.k("d4-none")).fetch().map_err(e2s)?.is_empty(), "no match is empty")
}

fn d5(c: &Ctx) -> R {
    let items = c.items()?;
    let mut ids = Vec::new();
    for i in 0..30u32 {
        let id = c.k(&format!("d5/{i:02}"));
        items.insert(&item(&id, "d5", 1000 + i % 3)).map_err(e2s)?;
        ids.push((1000 + i % 3, id));
    }
    ids.sort();
    let walk = |desc: bool| -> Result<Vec<(u32, String)>, String> {
        let mut out = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let mut q = items.by_qty(1000..=1002).limit(7);
            if desc {
                q = q.desc();
            }
            if let Some(a) = &after {
                q = q.after(a.clone());
            }
            let (rows, next) = q.fetch_page().map_err(e2s)?;
            out.extend(rows.into_iter().map(|x| (x.qty, x.id)));
            match next {
                Some(n) => after = Some(n),
                None => return Ok(out),
            }
        }
    };
    let up = walk(false)?;
    ensure(up == ids, format!("ascending pages of 7: {} rows, first {:?}", up.len(), up.first()))?;
    let mut rev = ids.clone();
    rev.reverse();
    ensure(walk(true)? == rev, "descending pages of 7")?;
    ensure(items.by_qty(1001..1002).count().map_err(e2s)? == 10, "half-open range 1001..1002 has 10")?;
    ensure(items.by_qty(1003..).count().map_err(e2s)? == 0, "above the data is empty")
}

fn d6(c: &Ctx) -> R {
    let items = c.items()?;
    let p = c.k("d6");
    for i in 0..1201 {
        items.insert(&item(&format!("{p}/{i:04}"), "d6", 0)).map_err(e2s)?;
    }
    let mut seen = HashSet::new();
    let mut pages = 0;
    let mut after: Option<String> = None;
    loop {
        let mut q = items.prefix(format!("{p}/")).limit(500);
        if let Some(a) = &after {
            q = q.after(a.clone());
        }
        let (rows, next) = q.fetch_page().map_err(e2s)?;
        pages += 1;
        for r in rows {
            ensure(seen.insert(r.id.clone()), format!("duplicate {}", r.id))?;
        }
        match next {
            Some(n) => after = Some(n),
            None => break,
        }
    }
    ensure(seen.len() == 1201 && pages == 3, format!("{} rows in {pages} pages, want 1201 in 3", seen.len()))
}

fn d7(c: &Ctx) -> R {
    let items = c.items()?;
    let p = c.k("d7");
    let mut w = items.prefix(format!("{p}/")).watch().map_err(e2s)?;
    let next = |w: &mut ce_db::Watch<Item>| w.next_timeout(Duration::from_secs(3)).ok_or("no event within 3 s".to_string())?.map_err(e2s);
    ensure(next(&mut w)? == Change::Synced, "Synced first (nothing matched yet)")?;
    let other = ce_db::open_at("db-conformance", &c.store, &c.cap).map_err(e2s)?.collection::<Item>().map_err(e2s)?;
    let id = format!("{p}/1");
    let a = item(&id, "d7", 1);
    other.insert(&a).map_err(e2s)?;
    ensure(next(&mut w)? == Change::Added(a.clone()), "Added")?;
    other.update(&id, |x| x.qty = 2).map_err(e2s)?;
    match next(&mut w)? {
        Change::Changed { old, new } => ensure(old.qty == 1 && new.qty == 2, "Changed old/new")?,
        o => return Err(format!("want Changed, got {o:?}")),
    }
    other.remove(&id).map_err(e2s)?;
    ensure(next(&mut w)? == Change::Removed(id.clone()), "Removed")?;
    ensure(w.next_timeout(Duration::from_millis(300)).is_none(), "nothing more")
}

fn d9(c: &Ctx) -> R {
    let items = c.items()?;
    let id = c.k("d9");
    let mut big = item(&id, "d9", 1);
    big.tags = (0..300).map(|i| format!("t{i:05}")).collect();
    let n = serde_json::to_string(&big.tags).map_err(|e| e.to_string())?.len();
    match items.insert(&big) {
        Err(DbError::TooLarge { bytes, limit: 2000 }) if bytes > 2000 => {}
        o => return Err(format!("want TooLarge over 2000 (tags alone {n} bytes), got {o:?}")),
    }
    ensure(items.get(&id).map_err(e2s)?.is_none(), "nothing written")
}

fn d10(c: &Ctx) -> R {
    let ro = ce_db::open_at("db-conformance", &c.store, &c.cap_ro).map_err(e2s)?.collection::<Item>().map_err(e2s)?;
    match ro.insert(&item(&c.k("d10"), "d10", 1)) {
        Err(DbError::Refused { need }) => ensure(need == "state.dbconformance.items.write", format!("names {need}")),
        o => Err(format!("want Refused, got {o:?}")),
    }?;
    ensure(c.items()?.get(c.k("d10")).map_err(e2s)?.is_none(), "nothing written")
}

/// D12: a grant on collection `items` is refused on collection `notes` of the same app, and still works on `items`.
fn d12(c: &Ctx) -> R {
    c.db.collection::<Note>().map_err(e2s)?; // the full grant registers `notes`
    let db = ce_db::open_at("db-conformance", &c.store, &c.cap_items).map_err(e2s)?;
    let items = db.collection::<Item>().map_err(e2s)?;
    items.insert(&item(&c.k("d12"), "d12", 1)).map_err(|e| format!("the items grant on items: {e:?}"))?;
    let notes = db.collection::<Note>().map_err(e2s)?;
    match notes.insert(&Note { id: c.k("d12"), text: "x".into() }) {
        Err(DbError::Refused { need }) => ensure(need == "state.dbconformance.notes.write", format!("insert names {need}"))?,
        o => return Err(format!("insert on notes: want Refused, got {o:?}")),
    }
    match notes.get(c.k("d12")) {
        Err(DbError::Refused { need }) => ensure(need == "state.dbconformance.notes.read", format!("get names {need}"))?,
        o => return Err(format!("get on notes: want Refused, got {o:?}")),
    }
    ensure(c.db.collection::<Note>().map_err(e2s)?.get(c.k("d12")).map_err(e2s)?.is_none(), "nothing written to notes")
}

/// D13: a grant of another app is refused on this app, naming a right of this app, and nothing is written.
fn d13(c: &Ctx) -> R {
    let refused = |e: DbError| match e {
        DbError::Refused { need } if need.starts_with("state.dbconformance.") => Ok(need),
        o => Err(format!("want Refused naming a state.dbconformance right, got {o:?}")),
    };
    let need = match ce_db::open_at("db-conformance", &c.store, &c.cap_other) {
        Err(e) => refused(e)?,
        Ok(db) => match db.collection::<Item>() {
            Err(e) => refused(e)?,
            Ok(items) => match items.insert(&item(&c.k("d13"), "d13", 1)) {
                Err(e) => refused(e)?,
                Ok(_) => return Err("another app's grant wrote to db-conformance".into()),
            },
        },
    };
    ensure(c.items()?.get(c.k("d13")).map_err(e2s)?.is_none(), format!("refused ({need}) and nothing written"))
}

/// D11, the watcher: b and c exist and are seen, then READY; the driver kills the store, writes elsewhere, and brings
/// it back. The watch must report Offline, then Resyncing, then deliver exactly the missed changes, then be Live.
fn d11_watch(db: &Db, tag: &str) -> R {
    let items = db.collection::<Item>().map_err(e2s)?;
    let p = format!("d11-{tag}/");
    let (a, b, c) = (format!("{p}a"), format!("{p}b"), format!("{p}c"));
    items.insert(&item(&b, "d11", 1)).map_err(e2s)?;
    items.insert(&item(&c, "d11", 1)).map_err(e2s)?;
    let mut w = items.prefix(p.clone()).watch().map_err(e2s)?;
    let links = w.link_changes();
    let mut first = Vec::new();
    loop {
        match w.next_timeout(Duration::from_secs(5)).ok_or("no Synced within 5 s")?.map_err(e2s)? {
            Change::Synced => break,
            ch => first.push(ch),
        }
    }
    ensure(first.len() == 2, format!("before Synced: {first:?}"))?;
    println!("READY");
    let t = Instant::now();
    let mut seen: Vec<String> = Vec::new(); // the link states and changes, in order
    let mut got: Vec<Change<Item>> = Vec::new();
    let mut live_again = false;
    // link transitions are sent before the changes they cause, so drain them before recording each change
    let drain = |seen: &mut Vec<String>, live_again: &mut bool| {
        for l in links.try_iter() {
            let name = match &l {
                Link::Offline(_) => "offline",
                Link::Resyncing => "resyncing",
                Link::Live => "live",
                Link::Connecting => "connecting",
            };
            if seen.last().map(String::as_str) != Some(name) {
                seen.push(name.into());
            }
            *live_again |= l == Link::Live && seen.iter().any(|s| s == "resyncing");
        }
    };
    while t.elapsed() < Duration::from_secs(40) {
        drain(&mut seen, &mut live_again);
        if let Some(ch) = w.next_timeout(Duration::from_millis(50)) {
            let ch = ch.map_err(e2s)?;
            drain(&mut seen, &mut live_again);
            seen.push(format!("change {}", match &ch {
                Change::Added(x) => format!("added {}", x.id),
                Change::Changed { new, .. } => format!("changed {}", new.id),
                Change::Removed(k) => format!("removed {k}"),
                Change::Synced => "synced".into(),
            }));
            got.push(ch);
        }
        if got.len() >= 3 && live_again {
            break;
        }
    }
    eprintln!("d11: {}", seen.join(" -> "));
    let off = seen.iter().position(|s| s == "offline").ok_or(format!("never Offline: {seen:?}"))?;
    let re = seen.iter().skip(off).position(|s| s == "resyncing").map(|i| i + off).ok_or(format!("never Resyncing after Offline: {seen:?}"))?;
    let first_change = seen.iter().position(|s| s.starts_with("change")).ok_or(format!("no change delivered: {seen:?}"))?;
    ensure(first_change > re, format!("a change arrived before Resyncing: {seen:?}"))?;
    ensure(live_again, format!("not Live again: {seen:?}"))?;
    let want_b = Item { qty: 2, ..item(&b, "d11", 1) };
    let mut ok = [false; 3];
    for ch in &got {
        match ch {
            Change::Added(x) if *x == item(&a, "d11", 1) => ok[0] = true,
            Change::Changed { old, new } if old.qty == 1 && *new == want_b => ok[1] = true,
            Change::Removed(k) if *k == c => ok[2] = true,
            o => return Err(format!("an unexpected change {o:?}")),
        }
    }
    ensure(ok == [true; 3] && got.len() == 3, format!("missed changes: added a {}, changed b {}, removed c {}; {} changes", ok[0], ok[1], ok[2], got.len()))?;
    ensure(w.next_timeout(Duration::from_millis(300)).is_none(), "no duplicate after")
}

fn d11_write(db: &Db, tag: &str) -> R {
    let items = db.collection::<Item>().map_err(e2s)?;
    let p = format!("d11-{tag}/");
    items.insert(&item(&format!("{p}a"), "d11", 1)).map_err(e2s)?;
    items.update(format!("{p}b"), |x| x.qty = 2).map_err(e2s)?;
    items.remove(format!("{p}c")).map_err(e2s)
}

fn arg(a: &[String], name: &str) -> Option<String> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let store = arg(&a, "--store").unwrap_or_else(|| "127.0.0.1:7811".into());
    let cap = arg(&a, "--cap").unwrap_or_else(|| "rw".into());
    let db = match ce_db::open_at("db-conformance", &store, &cap) {
        Ok(d) => d,
        Err(e) => {
            println!("CONF OPEN FAIL {e}");
            std::process::exit(1);
        }
    };
    let mode = a.get(1).cloned().unwrap_or_default();
    if mode == "d11-watch" || mode == "d11-write" {
        let tag = arg(&a, "--tag").unwrap_or_else(|| "d11".into());
        let r = if mode == "d11-watch" { d11_watch(&db, &tag) } else { d11_write(&db, &tag) };
        match (mode.as_str(), r) {
            ("d11-watch", Ok(())) => println!("CONF D11 PASS"),
            ("d11-watch", Err(e)) => println!("CONF D11 FAIL {e}"),
            (_, Ok(())) => println!("WROTE d11"),
            (_, Err(e)) => {
                println!("CONF D11 FAIL the writer: {e}");
                std::process::exit(1)
            }
        }
        return;
    }
    if mode == "d8-write" || mode == "d8-watch" {
        let key = arg(&a, "--key").unwrap_or_else(|| "d8".into());
        let items = match db.collection::<Item>() {
            Ok(i) => i,
            Err(e) => {
                println!("CONF D8 FAIL {e}");
                std::process::exit(1);
            }
        };
        if mode == "d8-write" {
            let r = items.insert(&golden_a(&key));
            println!("WROTE {key} {r:?}");
            std::process::exit(if r.is_ok() { 0 } else { 1 });
        }
        let r = (|| -> R {
            let mut w = items.watch_key(&key).map_err(e2s)?;
            ensure(w.next_timeout(Duration::from_secs(3)) == Some(Ok(Change::Synced)), "Synced")?;
            println!("READY");
            let t = Instant::now();
            let ch = w.next_timeout(Duration::from_secs(10)).ok_or("no event within 10 s")?.map_err(e2s)?;
            let ms = t.elapsed().as_millis();
            ensure(ch == Change::Added(golden_a(&key)), format!("watch event {ch:?}"))?;
            ensure(items.get(&key).map_err(e2s)? == Some(golden_a(&key)), "get equals the golden record")?;
            eprintln!("d8: seen {ms} ms after READY");
            Ok(())
        })();
        match r {
            Ok(()) => println!("CONF D8 PASS"),
            Err(e) => println!("CONF D8 FAIL {e}"),
        }
        return;
    }
    let tag = arg(&a, "--tag").unwrap_or_else(|| format!("{}", std::process::id()));
    let c = Ctx {
        db,
        store,
        cap,
        cap_ro: arg(&a, "--cap-ro").unwrap_or_else(|| "ro".into()),
        cap_items: arg(&a, "--cap-items").unwrap_or_else(|| "items".into()),
        cap_other: arg(&a, "--cap-other").unwrap_or_else(|| "other".into()),
        tag,
    };
    let all: [(&str, fn(&Ctx) -> R); 11] = [("D1", d1), ("D2", d2), ("D3", d3), ("D4", d4), ("D5", d5), ("D6", d6), ("D7", d7), ("D9", d9), ("D10", d10), ("D12", d12), ("D13", d13)];
    let mut fail = false;
    for (id, f) in all {
        let t = Instant::now();
        match f(&c) {
            Ok(()) => println!("CONF {id} PASS ({} ms)", t.elapsed().as_millis()),
            Err(e) => {
                fail = true;
                println!("CONF {id} FAIL {e}");
            }
        }
    }
    std::process::exit(if fail { 1 } else { 0 });
}
