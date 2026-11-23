import assert from 'node:assert/strict';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { describe, test } from 'node:test';
import {
  addChunks,
  addRegions,
  chunkIndex,
  count,
  emptySelection,
  fromCsv,
  fromWire,
  invert,
  isChunkSelected,
  isEmpty,
  merge,
  regionState,
  removeChunks,
  removeRegions,
  selectedRegions,
  selectionText,
  subtract,
  toCsv,
  toggleChunk,
  toggleRegion,
  toWire,
  type Selection,
} from '../frontend/src/lib/selection.ts';

const rect = (x1: number, z1: number, x2: number, z2: number) => ({ x1, z1, x2, z2 });

/** Every selected chunk in a window, for comparing selections chunk by chunk. */
const selectedIn = (s: Selection, x1: number, z1: number, x2: number, z2: number): string[] => {
  const out: string[] = [];
  for (let z = z1; z <= z2; z++) for (let x = x1; x <= x2; x++) if (isChunkSelected(s, x, z)) out.push(`${x},${z}`);
  return out;
};

const WINDOW = [-70, -70, 70, 70] as const;

describe('selection math', () => {
  test('chunk indices and empty selections', () => {
    assert.equal(chunkIndex(0, 0), 0);
    assert.equal(chunkIndex(-1, -1), 1023);
    assert.equal(chunkIndex(33, 2), 1 + 64);
    assert.ok(isEmpty(emptySelection()));
    assert.ok(!isEmpty(invert(emptySelection())));
  });

  test('adding a rectangle across region borders and negative coordinates', () => {
    const s = addChunks(emptySelection(), rect(-2, -1, 1, 0));
    assert.equal(s.regions.size, 4);
    assert.equal(count(s, []), 8);
    assert.ok(isChunkSelected(s, -2, -1) && isChunkSelected(s, 1, 0) && !isChunkSelected(s, 2, 0));
    assert.equal(regionState(s, -1, -1), 'partial');
    assert.equal(regionState(s, 5, 5), 'none');
    // corners given in any order
    assert.deepEqual(addChunks(emptySelection(), rect(1, 0, -2, -1)), s);
  });

  test('full regions collapse to null and removing splits them', () => {
    let s = addChunks(emptySelection(), rect(0, 0, 31, 30));
    assert.notEqual(s.regions.get('0,0'), null);
    s = addChunks(s, rect(0, 31, 31, 31));
    assert.equal(s.regions.get('0,0'), null);
    assert.equal(regionState(s, 0, 0), 'full');
    s = removeChunks(s, rect(5, 5, 5, 5));
    assert.equal(count(s, []), 1023);
    s = removeChunks(s, rect(0, 0, 31, 31));
    assert.equal(s.regions.size, 0);
  });

  test('functions never change their input', () => {
    const a = addChunks(emptySelection(), rect(0, 0, 3, 3));
    const before = toWire(a);
    addChunks(a, rect(0, 0, 40, 40));
    removeChunks(a, rect(1, 1, 1, 1));
    toggleChunk(a, 0, 0);
    merge(a, invert(a));
    assert.deepEqual(toWire(a), before);
  });

  test('toggling chunks and regions', () => {
    let s = toggleChunk(emptySelection(), -33, 40);
    assert.ok(isChunkSelected(s, -33, 40));
    s = toggleChunk(s, -33, 40);
    assert.ok(isEmpty(s));
    s = toggleRegion(s, -1, 1);
    assert.equal(regionState(s, -1, 1), 'full');
    s = toggleChunk(s, -32, 32);
    assert.equal(regionState(s, -1, 1), 'partial');
    s = toggleRegion(s, -1, 1);
    assert.equal(regionState(s, -1, 1), 'full');
    s = toggleRegion(s, -1, 1);
    assert.equal(regionState(s, -1, 1), 'none');
  });

  test('region rectangles', () => {
    const s = addRegions(emptySelection(), 1, 1, -1, 0);
    assert.equal(s.regions.size, 6);
    assert.equal(count(s, []), 6 * 1024);
    const r = removeRegions(s, 0, 0, 0, 1);
    assert.equal(r.regions.size, 4);
    assert.equal(regionState(r, 0, 1), 'none');
  });

  test('inverted selections select everything except the stored chunks', () => {
    const base = addChunks(emptySelection(), rect(0, 0, 1, 1));
    const inv = invert(base);
    assert.ok(inv.inverted);
    assert.ok(!isChunkSelected(inv, 0, 0));
    assert.ok(isChunkSelected(inv, 2, 2));
    assert.ok(isChunkSelected(inv, -500, 900));
    assert.equal(regionState(inv, 0, 0), 'partial');
    assert.equal(regionState(inv, 7, 7), 'full');
    // adding to an inverted selection removes from the stored set (MCA Selector addChunk)
    const added = addChunks(inv, rect(0, 0, 0, 0));
    assert.ok(isChunkSelected(added, 0, 0));
    assert.equal(added.regions.get('0,0') && count({ inverted: false, regions: added.regions }, []), 3);
    // adding a whole region drops it from the stored map (MCA Selector addRegion)
    assert.equal(addRegions(inv, 0, 0, 0, 0).regions.size, 0);
    // removing from an inverted selection stores the chunks; a whole region becomes null
    const removed = removeChunks(inv, rect(64, 64, 64 + 31, 64 + 31));
    assert.equal(removed.regions.get('2,2'), null);
    assert.equal(regionState(removed, 2, 2), 'none');
    // inverting twice restores the selection
    assert.deepEqual(selectedIn(invert(inv), ...WINDOW), selectedIn(base, ...WINDOW));
  });

  test('counts resolve inverted selections within existing regions', () => {
    const s = addChunks(emptySelection(), rect(0, 0, 9, 0));
    const existing: [number, number][] = [[0, 0], [1, 0], [-1, -1], [0, 0]];
    assert.equal(count(s, existing), 10);
    assert.equal(selectedRegions(s, existing), 1);
    const inv = invert(s);
    assert.equal(count(inv, existing), 1014 + 1024 + 1024);
    assert.equal(selectedRegions(inv, existing), 3);
    const withHole = removeRegions(inv, 1, 0, 1, 0);
    assert.equal(count(withHole, existing), 1014 + 1024);
    assert.equal(selectedRegions(withHole, existing), 2);
  });

  test('merge matches the union for every inverted combination', () => {
    const a = addChunks(addRegions(emptySelection(), 1, 1, 1, 1), rect(-3, -3, 3, 3));
    const b = addChunks(addRegions(emptySelection(), -1, 1, -1, 1), rect(0, 0, 40, 5));
    const cases: [Selection, Selection][] = [
      [a, b],
      [invert(a), b],
      [a, invert(b)],
      [invert(a), invert(b)],
      [a, invert(a)],
      [invert(a), invert(a)],
    ];
    for (const [x, y] of cases) {
      const merged = merge(x, y);
      const expected = new Set([...selectedIn(x, ...WINDOW), ...selectedIn(y, ...WINDOW)]);
      assert.deepEqual(new Set(selectedIn(merged, ...WINDOW)), expected);
      assert.equal(merged.inverted, x.inverted || y.inverted);
    }
    // MCA Selector: a non-inverted selection merged with an inverted one keeps only the other's stored regions
    const m = merge(a, invert(b));
    assert.ok([...m.regions.keys()].every((key) => b.regions.has(key)));
  });

  test('subtract matches the difference for every inverted combination', () => {
    const a = addChunks(addRegions(emptySelection(), 1, 1, 1, 1), rect(-40, -3, 3, 3));
    const b = addChunks(addRegions(emptySelection(), -2, -1, -2, -1), rect(0, 0, 40, 5));
    const pairs: [Selection, Selection][] = [
      [a, b],
      [invert(a), b],
      [a, invert(b)],
      [invert(a), invert(b)],
      [a, a],
      [invert(a), invert(a)],
      [a, invert(emptySelection())],
    ];
    for (const [x, y] of pairs) {
      const result = subtract(x, y);
      const removed = new Set(selectedIn(y, ...WINDOW));
      assert.deepEqual(
        selectedIn(result, ...WINDOW),
        selectedIn(x, ...WINDOW).filter((chunk) => !removed.has(chunk)),
      );
      // MCA Selector: only "inverted minus not inverted" stays inverted
      assert.equal(result.inverted, x.inverted && !y.inverted);
    }
    assert.ok(isEmpty(subtract(a, a)));
    const before = toWire(a);
    subtract(a, b);
    assert.deepEqual(toWire(a), before);
  });
});

describe('wire format', () => {
  test('exact base64 for a known bitset', () => {
    let s = emptySelection();
    s = toggleChunk(s, -32, 64);
    s = toggleChunk(s, -23, 64);
    s = toggleChunk(s, -1, 95);
    s = addRegions(s, 3, -4, 3, -4);
    const wire = toWire(s);
    assert.deepEqual(wire, {
      inverted: false,
      regions: [
        {
          x: -1,
          z: 2,
          chunks: `AQI${'A'.repeat(166)}IA=`,
        },
        { x: 3, z: -4, chunks: null },
      ],
    });
    assert.equal(wire.regions[0].chunks?.length, 172);
    assert.deepEqual(fromWire(wire), s);
  });

  test('round trips inverted selections and normalizes full and empty sets', () => {
    const s = invert(addChunks(emptySelection(), rect(-40, -40, 40, 10)));
    assert.deepEqual(fromWire(JSON.parse(JSON.stringify(toWire(s)))), s);
    const full = btoa(String.fromCharCode(...new Uint8Array(128).fill(255)));
    const empty = btoa(String.fromCharCode(...new Uint8Array(128)));
    const read = fromWire({ inverted: false, regions: [{ x: 0, z: 0, chunks: full }, { x: 1, z: 0, chunks: empty }] });
    assert.equal(read.regions.get('0,0'), null);
    assert.ok(!read.regions.has('1,0'));
  });

  test('rejects bad chunk sets', () => {
    assert.throws(() => fromWire({ inverted: false, regions: [{ x: 0, z: 0, chunks: 'AAAA' }] }), /3 bytes/);
    assert.throws(() => fromWire({ inverted: false, regions: [{ x: 0, z: 0, chunks: '***' }] }), /base64/);
  });
});

describe('CSV', () => {
  test('writes MCA Selector saveToFile lines', () => {
    let s = addRegions(emptySelection(), -1, 0, -1, 0);
    s = toggleChunk(s, 33, -1);
    s = toggleChunk(s, 32, -1);
    assert.equal(toCsv(s), '-1;0\n1;-1;32;-1\n1;-1;33;-1\n');
    assert.equal(toCsv(invert(s)), 'inverted\n-1;0\n1;-1;32;-1\n1;-1;33;-1\n');
    assert.equal(toCsv(emptySelection()), '');
  });

  test('round trips', () => {
    const s = invert(addChunks(addRegions(emptySelection(), 2, 2, 3, 3), rect(-35, -2, 4, 7)));
    assert.deepEqual(fromCsv(toCsv(s)), s);
    assert.deepEqual(fromCsv(toCsv(s).replace(/\n/g, '\r\n')), s);
  });

  test('reads readFromFile rules', () => {
    // a chunk inside a whole region adds nothing; "inverted" only counts on line 1; trailing ";" dropped
    const s = fromCsv('inverted\n0;0\n0;0;5;5\n-1;-1;-1;-1;\n');
    assert.ok(s.inverted);
    assert.equal(s.regions.get('0,0'), null);
    assert.ok(isChunkSelected({ inverted: false, regions: s.regions }, -1, -1));
    assert.throws(() => fromCsv('0;0\ninverted\n'), /^Error: line 2: invalid region or chunk coordinate format$/);
    assert.throws(() => fromCsv('0;0\n\n1;1'), /line 2: invalid region/);
    assert.throws(() => fromCsv('0;0;1'), /line 1: invalid region/);
    assert.throws(() => fromCsv('a;0'), /line 1: failed to read region coordinates/);
    assert.throws(() => fromCsv('0;0\n0; 1'), /line 2: failed to read region coordinates/);
    assert.throws(() => fromCsv('0;0;x;0'), /line 1: failed to read chunk coordinates/);
    assert.throws(() => fromCsv('0;0;0;0\n1;0;31;0'), /line 2: chunk 31;0 is not in region 1;0/);
    assert.throws(() => fromCsv('0;3000000000'), /failed to read region/);
  });

  test('MCA Selector made CSV files re-serialize identically', (t) => {
    const dir = new URL('./fixtures/mcaselector/', import.meta.url);
    const files = existsSync(dir)
      ? readdirSync(dir, { recursive: true, encoding: 'utf8' }).filter((name) => name.endsWith('.csv'))
      : [];
    if (files.length === 0) {
      t.skip('no tests/fixtures/mcaselector/*.csv');
      return;
    }
    for (const name of files) {
      const text = readFileSync(new URL(name, dir), 'utf8');
      const lines = (text: string) => text.split(/\r?\n/).filter((line) => line !== '');
      const original = lines(text);
      const written = lines(toCsv(fromCsv(text)));
      // the inverted marker stays first; MCA Selector writes regions in hash order
      assert.equal(written[0] === 'inverted', original[0] === 'inverted', name);
      assert.deepEqual([...written].sort(), [...original].sort(), name);
    }
  });
});

describe('copy text', () => {
  test('whole regions and merged chunk rectangles in block coordinates', () => {
    let s = addRegions(emptySelection(), -1, 0, -1, 0);
    s = addChunks(s, rect(0, 0, 1, 2));
    s = toggleChunk(s, 5, 0);
    assert.equal(selectionText(s), ['-512 0 -> -1 511', '0 0 -> 31 47', '80 0 -> 95 15'].join('\n'));
    assert.equal(selectionText(invert(emptySelection())), 'inverted');
  });
});
