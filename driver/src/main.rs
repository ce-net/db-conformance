//! `db-conformance [--rs PATH] [--ts PATH] [--node node] [--store ADDR --cap CAP --cap-ro CAP]`
//!
//! Without `--store` every run gets a fresh dev store (ce-db-dev, in this process, loopback, nothing but what the
//! runners write). With `--store` the runners use that store, under the app `db-conformance` only, with a run tag in
//! every key, so the kit can be pointed at a granted dev namespace of a real replica.
//!
//! Each runner runs D1..D7, D9, D10 on its own store; then D8 runs both ways on one shared store: the watcher starts,
//! says READY, the other language writes, the watcher checks the event and the typed value. Prints the matrix and
//! exits 1 when any cell is not PASS.
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const IDS: [&str; 10] = ["D1", "D2", "D3", "D4", "D5", "D6", "D7", "D8", "D9", "D10"];

fn arg(a: &[String], name: &str) -> Option<String> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

struct Store {
    addr: String,
    cap: String,
    cap_ro: String,
    _dev: Option<ce_db_dev::DevStore>,
}

fn store(a: &[String]) -> Store {
    if let Some(addr) = arg(a, "--store") {
        return Store { addr, cap: arg(a, "--cap").unwrap_or_default(), cap_ro: arg(a, "--cap-ro").unwrap_or_default(), _dev: None };
    }
    let g = ce_db_dev::Grants::all_of("conf-rw", "db-conformance").with("conf-ro", &["state.dbconformance.items.read"]);
    let d = ce_db_dev::serve("127.0.0.1:0", g).unwrap_or_else(|e| {
        eprintln!("db-conformance: dev store: {e}");
        std::process::exit(2)
    });
    Store { addr: d.addr.to_string(), cap: "conf-rw".into(), cap_ro: "conf-ro".into(), _dev: Some(d) }
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
    let tag = format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis()));
    let langs = ["rs", "ts"];
    let mut table: Vec<(String, String, String)> = Vec::new(); // (lang, id, verdict)
    for lang in langs {
        let s = store(&a);
        println!("== {lang} runner, store {}", s.addr);
        let cmd = runner(lang, &a);
        let got = match spawn(&cmd, &["run", "--store", &s.addr, "--cap", &s.cap, "--cap-ro", &s.cap_ro, "--tag", &format!("{lang}{tag}")]) {
            Ok(c) => results(c),
            Err(e) => {
                println!("    {} did not start: {e}", cmd.join(" "));
                vec![]
            }
        };
        for id in IDS.iter().filter(|i| **i != "D8") {
            let v = got.iter().find(|(i, _)| i == id).map(|(_, l)| if l.contains(" PASS") { "PASS" } else { "FAIL" }).unwrap_or("MISSING");
            table.push((lang.into(), id.to_string(), v.into()));
        }
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
