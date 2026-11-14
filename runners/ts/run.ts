// The ce-db-ts runner of db-conformance (SCENARIOS.md). Drives only @ce-net/db's public API and the generated
// sdk/ts/db-conformance.ts (ce-wire-gen --records from schema/src/lib.rs).
//   node dist/runners/ts/run.js run --store ADDR --cap CAP --cap-ro CAP [--tag T]
//   node dist/runners/ts/run.js d8-watch|d8-write --store ADDR --cap CAP --key K
import * as fs from 'node:fs';
import { isDeepStrictEqual } from 'node:util';
import { open, encode, decode, DbError, type Db, type Change } from '@ce-net/db';
import { items, itemNext, ItemMeta, type Item } from '../../sdk/ts/db-conformance.js';

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
  const c: Ctx = { db, store, cap, capRo: arg('--cap-ro', 'ro'), tag: arg('--tag', String(process.pid)) };
  const all: [string, (c: Ctx) => Promise<void>][] = [['D1', d1], ['D2', d2], ['D3', d3], ['D4', d4], ['D5', d5], ['D6', d6], ['D7', d7], ['D9', d9], ['D10', d10]];
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
