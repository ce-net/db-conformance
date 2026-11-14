//! `db-conformance [--rs PATH] [--ts PATH] [--node node] [--store ADDR --cap CAP --cap-ro CAP --cap-items CAP --cap-other CAP]`
//!
//! Without `--store` every run gets a fresh dev store (ce-db-dev, in this process, loopback, nothing but what the
//! runners write). With `--store` the runners use that store, under the app `db-conformance` only, with a run tag in
//! every key, so the kit can be pointed at a granted dev namespace of a real replica.
//!
//! Each runner runs D1..D7, D9, D10, D12, D13 on its own store; then D8 runs both ways on one shared store: the
//! watcher starts, says READY, the other language writes, the watcher checks the event and the typed value. D11 always
//! runs on a dev store in a child process (`db-conformance dev-store`), never on the `--store`: the driver kills it
//! mid-watch, writes the changes the watcher must miss on a second instance over the same data file, and restarts
//! it on the first address. Prints the matrix and exits 1 when any cell is not PASS.
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const IDS: [&str; 13] = ["D1", "D2", "D3", "D4", "D5", "D6", "D7", "D8", "D9", "D10", "D11", "D12", "D13"];

fn arg(a: &[String], name: &str) -> Option<String> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

struct Store {
    addr: String,
    cap: String,
    cap_ro: String,
    cap_items: String,
    cap_other: String,
    _dev: Option<ce_db_dev::DevStore>,
}

/// The dev grants: the whole app, items read only, items read and write only, and every right of another app.
fn dev_grants() -> ce_db_dev::Grants {
    ce_db_dev::Grants::all_of("conf-rw", "db-conformance")
        .with("conf-ro", &["state.dbconformance.items.read"])
        .with("conf-items", &["state.dbconformance.items.read", "state.dbconformance.items.write"])
        .with("conf-other", &["state.partsdemo.*"])
}

fn store(a: &[String]) -> Store {
    if let Some(addr) = arg(a, "--store") {
        let c = |n: &str| arg(a, n).unwrap_or_default();
        return Store { addr, cap: c("--cap"), cap_ro: c("--cap-ro"), cap_items: c("--cap-items"), cap_other: c("--cap-other"), _dev: None };
    }
    let d = ce_db_dev::serve("127.0.0.1:0", dev_grants()).unwrap_or_else(|e| {
        eprintln!("db-conformance: dev store: {e}");
        std::process::exit(2)
    });
    Store { addr: d.addr.to_string(), cap: "conf-rw".into(), cap_ro: "conf-ro".into(), cap_items: "conf-items".into(), cap_other: "conf-other".into(), _dev: Some(d) }
}

/// A dev store in a child process (`db-conformance dev-store`), so D11 can kill it. Returns it and its address.
fn dev_child(listen: &str, data: &std::path::Path) -> Result<(Child, String), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut c = Command::new(me).args(["dev-store", "--listen", listen, "--data"]).arg(data).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().map_err(|e| e.to_string())?;
    let out = c.stdout.take().ok_or("no stdout")?;
    let mut l = String::new();
    BufReader::new(out).read_line(&mut l).map_err(|e| e.to_string())?;
    match l.trim().strip_prefix("ce-db-dev-store listening ") {
        Some(addr) => Ok((c, addr.to_string())),
        None => {
            let _ = c.kill();
            Err(format!("the dev store did not start on {listen}: {}", l.trim()))
        }
    }
}

/// D11 for one language: kill the store mid-watch, write elsewhere what the watcher misses, restart it, and expect
/// offline, resyncing, then exactly the missed changes (the runner's d11-watch decides).
fn d11(lang_cmd: &[String], tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("db-conformance-d11-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return format!("FAIL temp dir: {e}");
    }
    let data = dir.join("store.json");
    let r = (|| -> Result<String, String> {
        let (mut p1, addr) = dev_child("127.0.0.1:0", &data)?;
        let mut w = spawn(lang_cmd, &["d11-watch", "--store", &addr, "--cap", "conf-rw", "--tag", tag]).map_err(|e| format!("watcher: {e}"))?;
        let out = w.stdout.take().ok_or("no watcher stdout")?;
        let mut lines = BufReader::new(out).lines();
        loop {
            match lines.next() {
                Some(Ok(l)) if l == "READY" => break,
                Some(Ok(l)) if l.starts_with("CONF") => return Ok(l),
                Some(Ok(_)) => {}
                _ => return Err("the watcher ended before READY".into()),
            }
        }
        let _ = p1.kill();
        let _ = p1.wait();
        println!("    d11: killed the store at {addr} mid-watch");
        let (mut p2, other) = dev_child("127.0.0.1:0", &data)?;
        let wrote = Command::new(&lang_cmd[0]).args(&lang_cmd[1..]).args(["d11-write", "--store", &other, "--cap", "conf-rw", "--tag", tag]).output().map_err(|e| e.to_string())?;
        let _ = p2.kill();
        let _ = p2.wait();
        if !wrote.status.success() {
            return Err(format!("the writer failed: {}", String::from_utf8_lossy(&wrote.stdout).trim()));
        }
        println!("    d11: wrote the missed changes through a second instance at {other}, stopped it");
        let (mut p3, _) = dev_child(&addr, &data)?;
        println!("    d11: restarted the store at {addr} on the same data");
        let verdict = lines.map_while(Result::ok).find(|l| l.starts_with("CONF D11")).unwrap_or_else(|| "CONF D11 FAIL no verdict".into());
        let _ = w.wait();
        let _ = p3.kill();
        let _ = p3.wait();
        Ok(verdict)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    match r {
        Ok(v) => v,
        Err(e) => format!("CONF D11 FAIL {e}"),
    }
}

/// `db-conformance dev-store --listen ADDR --data FILE`: the D11 store, with the dev grants; serves until killed.
fn dev_store_main(a: &[String]) -> ! {
    let listen = arg(a, "--listen").unwrap_or_else(|| "127.0.0.1:0".into());
    let data = arg(a, "--data").map(std::path::PathBuf::from);
    match ce_db_dev::serve_with(&listen, dev_grants(), data) {
        Ok(s) => {
            println!("ce-db-dev-store listening {}", s.addr);
            loop {
                std::thread::park();
            }
        }
        Err(e) => {
            println!("ce-db-dev-store failed {e}");
            std::process::exit(2)
        }
    }
}

/// The command of a runner: `[program, args...]`.
fn runner(lang: &str, a: &[String]) -> Vec<String> {
    match lang {
        "rs" => vec![arg(a, "--rs").unwrap_or_else(|| {
            std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("conf-rs").to_string_lossy().into_owned())).unwrap_or_else(|| "conf-rs".into())
        })],
        _ => vec![arg(a, "--node").unwrap_or_else(|| "node".into()), arg(a, "--ts").unwrap_or_else(|| "dist/runners/ts/run.js".into())],
    }
}

fn spawn(cmd: &[String], args: &[&str]) -> std::io::Result<Child> {
    Command::new(&cmd[0]).args(&cmd[1..]).args(args).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn()
}

/// `CONF <id> PASS|FAIL ...` lines of a finished runner.
fn results(c: Child) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut c = c;
    if let Some(o) = c.stdout.take() {
        for l in BufReader::new(o).lines().map_while(Result::ok) {
            println!("    {l}");
            let w: Vec<&str> = l.splitn(3, ' ').collect();
            if w.len() >= 3 && w[0] == "CONF" {
                out.push((w[1].to_string(), l.clone()));
            }
        }
    }
    let _ = c.wait();
    out
}

fn d8(writer: &[String], watcher: &[String], s: &Store, key: &str) -> String {
    let mut w = match spawn(watcher, &["d8-watch", "--store", &s.addr, "--cap", &s.cap, "--key", key]) {
        Ok(w) => w,
        Err(e) => return format!("FAIL watcher did not start: {e}"),
    };
    let Some(out) = w.stdout.take() else { return "FAIL no watcher stdout".into() };
    let mut lines = BufReader::new(out).lines();
    let t = Instant::now();
    loop {
        match lines.next() {
            Some(Ok(l)) if l == "READY" => break,
            Some(Ok(l)) if l.starts_with("CONF") => return l,
            Some(Ok(_)) => {}
            _ => return "FAIL the watcher ended before READY".into(),
        }
        if t.elapsed() > Duration::from_secs(20) {
            let _ = w.kill();
            return "FAIL no READY within 20 s".into();
        }
    }
    let wrote = Command::new(&writer[0]).args(&writer[1..]).args(["d8-write", "--store", &s.addr, "--cap", &s.cap, "--key", key]).output();
    match &wrote {
        Ok(o) if o.status.success() => {}
        Ok(o) => return format!("FAIL the writer failed: {}", String::from_utf8_lossy(&o.stdout).trim()),
        Err(e) => return format!("FAIL the writer did not start: {e}"),
    }
    let verdict = lines.map_while(Result::ok).find(|l| l.starts_with("CONF D8")).unwrap_or_else(|| "CONF D8 FAIL no verdict".into());
    let _ = w.wait();
    verdict
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.get(1).map(String::as_str) == Some("dev-store") {
        dev_store_main(&a);
    }
    let tag = format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis()));
    let langs = ["rs", "ts"];
    let mut table: Vec<(String, String, String)> = Vec::new(); // (lang, id, verdict)
    for lang in langs {
        let s = store(&a);
        println!("== {lang} runner, store {}", s.addr);
        let cmd = runner(lang, &a);
        let got = match spawn(&cmd, &["run", "--store", &s.addr, "--cap", &s.cap, "--cap-ro", &s.cap_ro, "--cap-items", &s.cap_items, "--cap-other", &s.cap_other, "--tag", &format!("{lang}{tag}")]) {
            Ok(c) => results(c),
            Err(e) => {
                println!("    {} did not start: {e}", cmd.join(" "));
                vec![]
            }
        };
        for id in IDS.iter().filter(|i| **i != "D8" && **i != "D11") {
            let v = got.iter().find(|(i, _)| i == id).map(|(_, l)| if l.contains(" PASS") { "PASS" } else { "FAIL" }).unwrap_or("MISSING");
            table.push((lang.into(), id.to_string(), v.into()));
        }
    }
    for lang in langs {
        println!("== D11 {lang}: the store killed mid-watch and restarted (a dev store in a child process, never --store)");
        let line = d11(&runner(lang, &a), &format!("{lang}{tag}"));
        println!("    {line}");
        table.push((lang.into(), "D11".into(), if line.contains(" PASS") { "PASS".into() } else { "FAIL".into() }));
    }
    let s = store(&a);
    println!("== D8 across languages, store {}", s.addr);
    for (wr, wa) in [("rs", "ts"), ("ts", "rs")] {
        let line = d8(&runner(wr, &a), &runner(wa, &a), &s, &format!("d8-{wr}-{wa}-{tag}"));
        println!("    {wr} writes, {wa} watches: {line}");
        table.push((format!("{wr}->{wa}"), "D8".into(), if line.contains(" PASS") { "PASS".into() } else { "FAIL".into() }));
    }
    println!("\n== matrix");
    println!("{:<6} {}", "", IDS.iter().map(|i| format!("{i:<5}")).collect::<String>());
    for lang in langs {
        let row: String = IDS
            .iter()
            .map(|id| {
                if *id == "D8" {
                    let w = table.iter().find(|(l, i, _)| l.starts_with(&format!("{lang}->")) && i == "D8").map(|t| t.2.as_str()).unwrap_or("-");
                    let r = table.iter().find(|(l, i, _)| l.ends_with(&format!("->{lang}")) && i == "D8").map(|t| t.2.as_str()).unwrap_or("-");
                    format!("{:<5}", if w == "PASS" && r == "PASS" { "PASS" } else { "FAIL" })
                } else {
                    format!("{:<5}", table.iter().find(|(l, i, _)| l == lang && i == id).map(|t| t.2.as_str()).unwrap_or("-"))
                }
            })
            .collect();
        println!("{lang:<6} {row}");
    }
    let bad = table.iter().filter(|t| t.2 != "PASS").count();
    println!("\n{} of {} PASS", table.len() - bad, table.len());
    std::process::exit(if bad == 0 { 0 } else { 1 });
}
