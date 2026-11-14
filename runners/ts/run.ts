// The ce-db-ts runner of db-conformance (SCENARIOS.md). Drives only @ce-net/db's public API and the generated
// sdk/ts/db-conformance.ts (ce-wire-gen --records from schema/src/lib.rs).
//   node dist/runners/ts/run.js run --store ADDR --cap CAP --cap-ro CAP [--tag T]
//   node dist/runners/ts/run.js d8-watch|d8-write --store ADDR --cap CAP --key K
//   node dist/runners/ts/run.js d11-watch|d11-write --store ADDR --cap CAP --tag T
import * as fs from 'node:fs';
import { isDeepStrictEqual } from 'node:util';
import { open, encode, decode, DbError, type Db, type Change, type Link } from '@ce-net/db';
import { items, itemNext, notes, ItemMeta, type Item } from '../../sdk/ts/db-conformance.js';

const GOLDEN = JSON.parse(fs.readFileSync(new URL('../../../golden/items.json', import.meta.url), 'utf8')) as Record<string, string>;

function goldenA(id: string): Item {
  return { id, name: 'Grüße "q"\n\u0001', bin: 'A4', qty: 7, big: (1n << 64n) - 1n, delta: -(1n << 63n), ratio: 1.5, flag: true, tags: ['x', 'y'], raw: new Uint8Array([0, 1, 254, 255]), at: 1731576000000 };
}
function goldenB(id: string): Item {
  return { id, name: '', bin: 'B', qty: 0, big: 0n, delta: 0n, ratio: 100, flag: false, tags: [], raw: new Uint8Array(), at: 0, note: 'n' };
}
function item(id: string, bin: string, qty: number): Item {
  return { id, name: `item ${id}`, bin, qty, big: 1n, delta: -1n, ratio: 0.25, flag: false, tags: [], raw: new Uint8Array(), at: 1 };
}

function ensure(c: boolean, why: string): void {
  if (!c) throw new Error(why);
}
function kind(e: unknown): string {
  return e instanceof DbError ? JSON.stringify(e.info, (_, v) => (typeof v === 'bigint' ? `${v}n` : v)) : String(e);
}
async function fails(p: Promise<unknown>, want: (e: DbError) => boolean, what: string): Promise<void> {
  try {
    await p;
  } catch (e) {
    if (e instanceof DbError && want(e)) return;
    throw new Error(`${what}: got ${kind(e)}`);
  }
  throw new Error(`${what}: it succeeded`);
}

function arg(name: string, d = ''): string {
  const i = process.argv.indexOf(name);
  return i >= 0 && process.argv[i + 1] !== undefined ? process.argv[i + 1] : d;
}

interface Ctx {
  db: Db;
  store: string;
  cap: string;
  capRo: string;
  /** read and write on `items` only (D12) */
  capItems: string;
  /** every right of another app (D13) */
  capOther: string;
  tag: string;
}
const k = (c: Ctx, s: string) => `${s}-${c.tag}`;

async function d1(): Promise<void> {
  for (const [n, rec] of [['a', goldenA('g-a')], ['b', goldenB('g-b')]] as const) {
    const got = encode(ItemMeta, rec);
    ensure(got === GOLDEN[n], `${n}: encoded ${got} want ${GOLDEN[n]}`);
    const back = decode<Item>(ItemMeta, GOLDEN[n]);
    ensure(isDeepStrictEqual(back.value, rec) && Object.keys(back.extra).length === 0 && back.writerVersion === 2, `${n}: decoded ${kind(back.value)}`);
  }
}

async function d2(c: Ctx): Promise<void> {
  const it = items(c.db);
  const id = k(c, 'd2');
  const a = item(id, 'd2', 3);
  ensure((await it.insert(a)) === id, 'insert returns the key');
  ensure(isDeepStrictEqual(await it.get(id), a), 'get after insert');
  await fails(it.insert(a), (e) => e.info.kind === 'Conflict' && e.info.heldVersion === 1n, 'a second insert is Conflict { heldVersion: 1 }');
  const u = await it.update(id, (x) => {
    x.qty = 4;
  });
  ensure(u.qty === 4 && (await it.get(id))?.qty === 4, 'update');
  await fails(it.update(k(c, 'd2-none'), (x) => x), (e) => e.info.kind === 'NotFound', 'update of a missing key is NotFound');
  const s = (await it.getStored(id))!;
  ensure(s.version === 2n, `version 2 after insert+update, got ${s.version}`);
  await fails(it.replaceStored({ ...s, version: 1n }), (e) => e.info.kind === 'Conflict' && e.info.heldVersion === 2n, 'a stale compare-and-set is Conflict { heldVersion: 2 }');
  await it.remove(id);
  ensure((await it.get(id)) === undefined, 'get after remove is undefined');
}

async function d3(c: Ctx): Promise<void> {
  const next = itemNext(c.db);
  const id = k(c, 'd3');
  await next.insert({ ...item(id, 'd3', 1), colour: 'red' });
  const it = items(c.db);
  const s = (await it.getStored(id))!;
  ensure(s.extra.colour === 'red', 'the unknown field is in extra');
  await it.update(id, (x) => {
    x.qty = 5;
  });
  const after = (await next.get(id))!;
  ensure(after.qty === 5 && after.colour === 'red', `after the old reader's update: qty ${after.qty} colour ${after.colour}`);
}

async function d4(c: Ctx): Promise<void> {
  const it = items(c.db);
  const bin = k(c, 'd4');
  const want: string[] = [];
  for (let i = 0; i < 12; i++) {
    const id = k(c, `d4/${String(i).padStart(2, '0')}`);
    const b = i % 3 === 0 ? bin : k(c, 'd4-other');
    if (b === bin) want.push(id);
    await it.insert(item(id, b, i));
  }
  const got = (await it.byBin(bin).fetch()).map((x) => x.id);
  ensure(isDeepStrictEqual(got, want), `byBin: ${got} want ${want}`);
  ensure((await it.byBin(bin).count()) === 4, 'count');
  ensure((await it.byBin(k(c, 'd4-none')).fetch()).length === 0, 'no match is empty');
}

async function d5(c: Ctx): Promise<void> {
  const it = items(c.db);
  const ids: [number, string][] = [];
  for (let i = 0; i < 30; i++) {
    const id = k(c, `d5/${String(i).padStart(2, '0')}`);
    await it.insert(item(id, 'd5', 1000 + (i % 3)));
    ids.push([1000 + (i % 3), id]);
  }
  ids.sort((a, b) => a[0] - b[0] || (a[1] < b[1] ? -1 : 1));
  const walk = async (desc: boolean) => {
    const out: [number, string][] = [];
    let after: string | null = null;
    for (;;) {
      let q = it.byQty({ gte: 1000, lte: 1002 }).limit(7);
      if (desc) q = q.desc();
      if (after !== null) q = q.after(after);
      const p = await q.fetchPage();
      out.push(...p.rows.map((x) => [x.qty, x.id] as [number, string]));
      if (p.next === null) return out;
      after = p.next;
    }
  };
  ensure(isDeepStrictEqual(await walk(false), ids), 'ascending pages of 7');
  ensure(isDeepStrictEqual(await walk(true), [...ids].reverse()), 'descending pages of 7');
  ensure((await it.byQty({ gte: 1001, lt: 1002 }).count()) === 10, 'half-open range has 10');
  ensure((await it.byQty({ gte: 1003 }).count()) === 0, 'above the data is empty');
}

async function d6(c: Ctx): Promise<void> {
  const it = items(c.db);
  const p = k(c, 'd6');
  for (let i = 0; i < 1201; i++) await it.insert(item(`${p}/${String(i).padStart(4, '0')}`, 'd6', 0));
  const seen = new Set<string>();
  let pages = 0;
  let after: string | null = null;
  for (;;) {
    let q = it.prefix(`${p}/`).limit(500);
    if (after !== null) q = q.after(after);
    const pg = await q.fetchPage();
    pages++;
    for (const r of pg.rows) {
      ensure(!seen.has(r.id), `duplicate ${r.id}`);
      seen.add(r.id);
    }
    if (pg.next === null) break;
    after = pg.next;
  }
  ensure(seen.size === 1201 && pages === 3, `${seen.size} rows in ${pages} pages, want 1201 in 3`);
}

async function d7(c: Ctx): Promise<void> {
  const it = items(c.db);
  const p = k(c, 'd7');
  const w = await it.prefix(`${p}/`).watch();
  const next = async (): Promise<Change<Item>> => {
    const ch = await w.next(3000);
    if (!ch) throw new Error('no event within 3 s');
    return ch;
  };
  ensure((await next()).kind === 'synced', 'synced first (nothing matched yet)');
  const other = items(await open('db-conformance', { store: c.store, cap: c.cap }));
  const id = `${p}/1`;
  const a = item(id, 'd7', 1);
  await other.insert(a);
  const added = await next();
  ensure(added.kind === 'added' && isDeepStrictEqual(added.doc, a), `added: ${kind(added)}`);
  await other.update(id, (x) => {
    x.qty = 2;
  });
  const ch = await next();
  ensure(ch.kind === 'changed' && ch.old.qty === 1 && ch.doc.qty === 2, 'changed old/new');
  await other.remove(id);
  const rm = await next();
  ensure(rm.kind === 'removed' && rm.key === id, 'removed');
  ensure((await w.next(300)) === undefined, 'nothing more');
  w.close();
}

async function d9(c: Ctx): Promise<void> {
  const it = items(c.db);
  const id = k(c, 'd9');
  const big = item(id, 'd9', 1);
  big.tags = Array.from({ length: 300 }, (_, i) => `t${String(i).padStart(5, '0')}`);
  await fails(it.insert(big), (e) => e.info.kind === 'TooLarge' && e.info.bytes > 2000 && e.info.limit === 2000, 'TooLarge over 2000');
  ensure((await it.get(id)) === undefined, 'nothing written');
}

async function d10(c: Ctx): Promise<void> {
  const ro = items(await open('db-conformance', { store: c.store, cap: c.capRo }));
  await fails(ro.insert(item(k(c, 'd10'), 'd10', 1)), (e) => e.info.kind === 'Refused' && e.info.need === 'state.dbconformance.items.write', 'Refused naming state.dbconformance.items.write');
  ensure((await items(c.db).get(k(c, 'd10'))) === undefined, 'nothing written');
}

// D12: a grant on collection `items` is refused on collection `notes` of the same app, and still works on `items`.
async function d12(c: Ctx): Promise<void> {
  await notes(c.db).get(k(c, 'd12-register')); // the full grant registers `notes`
  const db = await open('db-conformance', { store: c.store, cap: c.capItems });
  await items(db).insert(item(k(c, 'd12'), 'd12', 1));
  const n = notes(db);
  await fails(n.insert({ id: k(c, 'd12'), text: 'x' }), (e) => e.info.kind === 'Refused' && e.info.need === 'state.dbconformance.notes.write', 'insert on notes: Refused naming state.dbconformance.notes.write');
  await fails(n.get(k(c, 'd12')), (e) => e.info.kind === 'Refused' && e.info.need === 'state.dbconformance.notes.read', 'get on notes: Refused naming state.dbconformance.notes.read');
  ensure((await notes(c.db).get(k(c, 'd12'))) === undefined, 'nothing written to notes');
  db.close();
}

// D13: a grant of another app is refused on this app, naming a right of this app, and nothing is written.
async function d13(c: Ctx): Promise<void> {
  const want = (e: DbError) => e.info.kind === 'Refused' && e.info.need.startsWith('state.dbconformance.');
  let db: Db | null = null;
  try {
    db = await open('db-conformance', { store: c.store, cap: c.capOther });
  } catch (e) {
    if (e instanceof DbError && want(e)) return;
    throw new Error(`open: got ${kind(e)}`);
  }
  await fails(items(db).insert(item(k(c, 'd13'), 'd13', 1)), want, "another app's grant: Refused naming a state.dbconformance right");
  ensure((await items(c.db).get(k(c, 'd13'))) === undefined, 'nothing written');
  db.close();
}

// D11, the watcher: b and c exist and are seen, then READY; the driver kills the store, writes elsewhere, and brings
// it back. The watch must report offline, then resyncing, then deliver exactly the missed changes, then be live.
async function d11Watch(db: Db, tag: string): Promise<void> {
  const it = items(db);
  const p = `d11-${tag}/`;
  const [a, b, c] = [`${p}a`, `${p}b`, `${p}c`];
  await it.insert(item(b, 'd11', 1));
  await it.insert(item(c, 'd11', 1));
  const w = await it.prefix(p).watch();
  const seen: string[] = [];
  let liveAgain = false;
  const stop = await it.onLink((l: Link) => {
    if (seen[seen.length - 1] !== l) seen.push(l);
    if (l === 'live' && seen.includes('resyncing')) liveAgain = true;
  });
  const first: Change<Item>[] = [];
  for (;;) {
    const ch = await w.next(5000);
    if (!ch) throw new Error('no synced within 5 s');
    if (ch.kind === 'synced') break;
    first.push(ch);
  }
  ensure(first.length === 2, `before synced: ${kind(first)}`);
  console.log('READY');
  const t0 = Date.now();
  const got: Change<Item>[] = [];
  while (Date.now() - t0 < 40000 && !(got.length >= 3 && liveAgain)) {
    const ch = await w.next(50);
    if (!ch) continue;
    seen.push(`change ${ch.kind} ${ch.kind === 'removed' ? ch.key : ch.kind === 'synced' ? '' : ch.doc.id}`);
    got.push(ch);
  }
  stop();
  console.error(`d11: ${seen.join(' -> ')}`);
  const off = seen.indexOf('offline');
  ensure(off >= 0, `never offline: ${seen}`);
  const re = seen.indexOf('resyncing', off);
  ensure(re > off, `never resyncing after offline: ${seen}`);
  const firstChange = seen.findIndex((s) => s.startsWith('change'));
  ensure(firstChange > re, `a change arrived before resyncing: ${seen}`);
  ensure(liveAgain, `not live again: ${seen}`);
  const ok = [false, false, false];
  for (const ch of got) {
    if (ch.kind === 'added' && isDeepStrictEqual(ch.doc, item(a, 'd11', 1))) ok[0] = true;
    else if (ch.kind === 'changed' && ch.old.qty === 1 && isDeepStrictEqual(ch.doc, { ...item(b, 'd11', 1), qty: 2 })) ok[1] = true;
    else if (ch.kind === 'removed' && ch.key === c) ok[2] = true;
    else throw new Error(`an unexpected change ${kind(ch)}`);
  }
  ensure(ok.every(Boolean) && got.length === 3, `missed changes: added a ${ok[0]}, changed b ${ok[1]}, removed c ${ok[2]}; ${got.length} changes`);
  ensure((await w.next(300)) === undefined, 'no duplicate after');
  w.close();
}

async function d11Write(db: Db, tag: string): Promise<void> {
  const it = items(db);
  const p = `d11-${tag}/`;
  await it.insert(item(`${p}a`, 'd11', 1));
  await it.update(`${p}b`, (x) => {
    x.qty = 2;
  });
  await it.remove(`${p}c`);
}

async function main(): Promise<void> {
  const mode = process.argv[2] ?? '';
  const store = arg('--store', '127.0.0.1:7811');
  const cap = arg('--cap', 'rw');
  let db: Db;
  try {
    db = await open('db-conformance', { store, cap });
  } catch (e) {
    console.log(`CONF OPEN FAIL ${kind(e)}`);
    process.exit(1);
  }
  if (mode === 'd11-watch' || mode === 'd11-write') {
    const tag = arg('--tag', 'd11');
    try {
      if (mode === 'd11-watch') {
        await d11Watch(db, tag);
        console.log('CONF D11 PASS');
      } else {
        await d11Write(db, tag);
        console.log('WROTE d11');
      }
      process.exit(0);
    } catch (e) {
      console.log(`CONF D11 FAIL ${mode === 'd11-write' ? 'the writer: ' : ''}${(e as Error).message}`);
      process.exit(mode === 'd11-write' ? 1 : 0);
    }
  }
  if (mode === 'd8-write' || mode === 'd8-watch') {
    const key = arg('--key', 'd8');
    const it = items(db);
    if (mode === 'd8-write') {
      try {
        await it.insert(goldenA(key));
        console.log(`WROTE ${key}`);
        process.exit(0);
      } catch (e) {
        console.log(`WROTE ${key} FAIL ${kind(e)}`);
        process.exit(1);
      }
    }
    try {
      const w = await it.watchKey(key);
      ensure((await w.next(3000))?.kind === 'synced', 'synced');
      console.log('READY');
      const t0 = Date.now();
      const ch = await w.next(10000);
      ensure(ch !== undefined, 'no event within 10 s');
      ensure(ch!.kind === 'added' && isDeepStrictEqual((ch as { doc: Item }).doc, goldenA(key)), `watch event ${kind(ch)}`);
      ensure(isDeepStrictEqual(await it.get(key), goldenA(key)), 'get equals the golden record');
      console.error(`d8: seen ${Date.now() - t0} ms after READY`);
      console.log('CONF D8 PASS');
    } catch (e) {
      console.log(`CONF D8 FAIL ${(e as Error).message}`);
    }
    process.exit(0);
  }
  const c: Ctx = { db, store, cap, capRo: arg('--cap-ro', 'ro'), capItems: arg('--cap-items', 'items'), capOther: arg('--cap-other', 'other'), tag: arg('--tag', String(process.pid)) };
  const all: [string, (c: Ctx) => Promise<void>][] = [['D1', d1], ['D2', d2], ['D3', d3], ['D4', d4], ['D5', d5], ['D6', d6], ['D7', d7], ['D9', d9], ['D10', d10], ['D12', d12], ['D13', d13]];
  let fail = false;
  for (const [id, f] of all) {
    const t0 = Date.now();
    try {
      await f(c);
      console.log(`CONF ${id} PASS (${Date.now() - t0} ms)`);
    } catch (e) {
      fail = true;
      console.log(`CONF ${id} FAIL ${(e as Error).message}`);
    }
  }
  process.exit(fail ? 1 : 0);
}

void main();
