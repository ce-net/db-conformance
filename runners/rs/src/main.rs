//! The ce-db-rs runner of db-conformance (SCENARIOS.md). Drives only ce-db's public API.
//!
//! `conf-rs run --store ADDR --cap CAP --cap-ro CAP [--tag T]`: D1..D7, D9, D10, one `CONF <id> PASS|FAIL <why>` each.
//! `conf-rs d8-watch --store ADDR --cap CAP --key K`: prints `READY` once watching, then `CONF D8 PASS|FAIL`.
//! `conf-rs d8-write --store ADDR --cap CAP --key K`: writes golden_a(K) and exits.
use ce_db::prelude::*;
use ce_db::{Change, Collection, Db};
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
    let c = Ctx { db, store, cap, cap_ro: arg(&a, "--cap-ro").unwrap_or_else(|| "ro".into()), tag };
    let all: [(&str, fn(&Ctx) -> R); 9] = [("D1", d1), ("D2", d2), ("D3", d3), ("D4", d4), ("D5", d5), ("D6", d6), ("D7", d7), ("D9", d9), ("D10", d10)];
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
