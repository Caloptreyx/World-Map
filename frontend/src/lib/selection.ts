/**
 * MCA Selector's chunk selection model (net.querz.mcaselector.selection.Selection): every region maps to
 * either `null` (the whole region) or a 1024-bit set of its chunks (bit `(x & 31) + (z & 31) * 32`).
 * With `inverted`, the map holds what is NOT selected ("everything except"), resolved within the regions
 * that exist, like `Selection.getTrueSelection`. Every function returns new objects; inputs are never
 * changed. Sets are kept normalized: empty sets are dropped and full sets become `null`.
 */
import type { ChunkRect } from './geometry.ts';

/** A selection: key `${x},${z}` → 128-byte bitset of the region's chunks, or `null` for the whole region. */
export interface Selection {
  inverted: boolean;
  regions: Map<string, Uint8Array | null>;
}

/** The JSON shape shared with the backend: chunks are base64 of the 128-byte bitset, `null` = whole region. */
export interface WireSelection {
  inverted: boolean;
  regions: { x: number; z: number; chunks: string | null }[];
}

const SET_BYTES = 128;
const REGION_CHUNKS = 1024;

export const emptySelection = (): Selection => ({ inverted: false, regions: new Map() });

/** Nothing selected: not inverted and no regions (an inverted selection selects everything else). */
export const isEmpty = (s: Selection): boolean => !s.inverted && s.regions.size === 0;

export const regionKey = (x: number, z: number): string => `${x},${z}`;

/** The chunk's bit index inside its region. */
export const chunkIndex = (x: number, z: number): number => (x & 31) + (z & 31) * 32;

const parseKey = (key: string): [number, number] => {
  const comma = key.indexOf(',');
  return [Number(key.slice(0, comma)), Number(key.slice(comma + 1))];
};

const getBit = (set: Uint8Array, i: number): boolean => (set[i >> 3] & (1 << (i & 7))) !== 0;

const popcount = (set: Uint8Array): number => {
  let n = 0;
  for (let byte of set) {
    while (byte) {
      byte &= byte - 1;
      n++;
    }
  }
  return n;
};

/** `a | b`, where null means all chunks. */
const union = (a: Uint8Array | null, b: Uint8Array | null): Uint8Array | null =>
  a === null || b === null ? null : a.map((byte, i) => byte | b[i]);

/** `a & b`, where null means all chunks. */
const intersection = (a: Uint8Array | null, b: Uint8Array | null): Uint8Array | null => {
  if (a === null) return b === null ? null : b.slice();
  if (b === null) return a.slice();
  return a.map((byte, i) => byte & b[i]);
};

/** `a & ~b`, where null means all chunks. */
const difference = (a: Uint8Array | null, b: Uint8Array | null): Uint8Array => {
  if (b === null) return new Uint8Array(SET_BYTES);
  return (a ?? new Uint8Array(SET_BYTES).fill(0xff)).map((byte, i) => byte & ~b[i] & 0xff);
};

/** Stores a set normalized: empty sets are removed, full sets become null. */
const put = (regions: Map<string, Uint8Array | null>, key: string, set: Uint8Array | null): void => {
  if (set === null) {
    regions.set(key, null);
    return;
  }
  const n = popcount(set);
  if (n === 0) regions.delete(key);
  else regions.set(key, n === REGION_CHUNKS ? null : set);
};

/** Whether the chunk at absolute chunk coordinates is selected. */
export const isChunkSelected = (s: Selection, x: number, z: number): boolean => {
  const key = regionKey(x >> 5, z >> 5);
  if (!s.regions.has(key)) return s.inverted;
  const set = s.regions.get(key) as Uint8Array | null;
  return (set === null || getBit(set, chunkIndex(x, z))) !== s.inverted;
};

/** How much of a region is selected. */
export const regionState = (s: Selection, rx: number, rz: number): 'none' | 'partial' | 'full' => {
  const key = regionKey(rx, rz);
  if (!s.regions.has(key)) return s.inverted ? 'full' : 'none';
  if (s.regions.get(key) === null) return s.inverted ? 'none' : 'full';
  return 'partial';
};

/** Calls `visit` for every region a rectangle touches with the bitset of the covered chunks (null: all). */
const eachRegionOf = (rect: ChunkRect, visit: (key: string, mask: Uint8Array | null) => void): void => {
  const x1 = Math.min(rect.x1, rect.x2);
  const x2 = Math.max(rect.x1, rect.x2);
  const z1 = Math.min(rect.z1, rect.z2);
  const z2 = Math.max(rect.z1, rect.z2);
  for (let rz = z1 >> 5; rz <= z2 >> 5; rz++) {
    for (let rx = x1 >> 5; rx <= x2 >> 5; rx++) {
      const cx1 = Math.max(x1, rx * 32) & 31;
      const cx2 = Math.min(x2, rx * 32 + 31) & 31;
      const cz1 = Math.max(z1, rz * 32) & 31;
      const cz2 = Math.min(z2, rz * 32 + 31) & 31;
      if (cx1 === 0 && cz1 === 0 && cx2 === 31 && cz2 === 31) {
        visit(regionKey(rx, rz), null);
        continue;
      }
      const mask = new Uint8Array(SET_BYTES);
      for (let cz = cz1; cz <= cz2; cz++) {
        for (let cx = cx1; cx <= cx2; cx++) {
          const i = cx + cz * 32;
          mask[i >> 3] |= 1 << (i & 7);
        }
      }
      visit(regionKey(rx, rz), mask);
    }
  }
};

/** Selects chunks of one region (MCA Selector addChunk / addRegion / addAll semantics). */
const select = (inverted: boolean, regions: Map<string, Uint8Array | null>, key: string, mask: Uint8Array | null) => {
  const present = regions.has(key);
  const current = regions.get(key) ?? null;
  if (inverted) {
    // stored chunks are the unselected ones: selecting removes them from the set
    if (present) put(regions, key, difference(current, mask));
  } else if (!present) {
    put(regions, key, mask === null ? null : mask.slice());
  } else if (current !== null) {
    put(regions, key, union(current, mask));
  }
};

/** Deselects chunks of one region (MCA Selector removeChunk / removeRegion semantics). */
const deselect = (inverted: boolean, regions: Map<string, Uint8Array | null>, key: string, mask: Uint8Array | null) => {
  const present = regions.has(key);
  const current = regions.get(key) ?? null;
  if (inverted) {
    // stored chunks are the unselected ones: deselecting adds them to the set
    if (!present) put(regions, key, mask === null ? null : mask.slice());
    else if (current !== null) put(regions, key, union(current, mask));
  } else if (present) {
    put(regions, key, difference(current, mask));
  }
};

const regionRect = (rx1: number, rz1: number, rx2: number, rz2: number): ChunkRect => ({
  x1: Math.min(rx1, rx2) * 32,
  z1: Math.min(rz1, rz2) * 32,
  x2: Math.max(rx1, rx2) * 32 + 31,
  z2: Math.max(rz1, rz2) * 32 + 31,
});

const applyRect = (s: Selection, rect: ChunkRect, op: typeof select): Selection => {
  const regions = new Map(s.regions);
  eachRegionOf(rect, (key, mask) => op(s.inverted, regions, key, mask));
  return { inverted: s.inverted, regions };
};

/** Selects every chunk in an inclusive chunk rectangle. */
export const addChunks = (s: Selection, rect: ChunkRect): Selection => applyRect(s, rect, select);

/** Deselects every chunk in an inclusive chunk rectangle. */
export const removeChunks = (s: Selection, rect: ChunkRect): Selection => applyRect(s, rect, deselect);

/** Selects the chunk, or deselects it when it is selected. */
export const toggleChunk = (s: Selection, x: number, z: number): Selection => {
  const rect = { x1: x, z1: z, x2: x, z2: z };
  return isChunkSelected(s, x, z) ? removeChunks(s, rect) : addChunks(s, rect);
};

/** Selects every region in an inclusive region rectangle. */
export const addRegions = (s: Selection, rx1: number, rz1: number, rx2: number, rz2: number): Selection =>
  addChunks(s, regionRect(rx1, rz1, rx2, rz2));

/** Deselects every region in an inclusive region rectangle. */
export const removeRegions = (s: Selection, rx1: number, rz1: number, rx2: number, rz2: number): Selection =>
  removeChunks(s, regionRect(rx1, rz1, rx2, rz2));

/** Selects the whole region, or deselects it when it is fully selected. */
export const toggleRegion = (s: Selection, rx: number, rz: number): Selection =>
  regionState(s, rx, rz) === 'full' ? removeRegions(s, rx, rz, rx, rz) : addRegions(s, rx, rz, rx, rz);

/** Selects exactly what was not selected (MCA Selector "Invert selection": flips `inverted`). */
export const invert = (s: Selection): Selection => ({ inverted: !s.inverted, regions: new Map(s.regions) });

/** Adds `b` to `a` (MCA Selector Selection.merge, every inverted combination). */
export const merge = (a: Selection, b: Selection): Selection => {
  const regions = new Map(a.regions);
  if (!a.inverted && !b.inverted) {
    for (const [key, set] of b.regions) {
      put(regions, key, regions.has(key) ? union(regions.get(key) ?? null, set) : set && set.slice());
    }
    return { inverted: false, regions };
  }
  if (a.inverted && !b.inverted) {
    // a stores what it does not select: b's chunks leave that set
    for (const [key, set] of b.regions) {
      if (regions.has(key)) put(regions, key, difference(regions.get(key) ?? null, set));
    }
    return { inverted: true, regions };
  }
  if (!a.inverted) {
    // b is inverted: the result is inverted and stores what neither selects
    const result = new Map<string, Uint8Array | null>();
    for (const [key, set] of b.regions) {
      if (regions.has(key)) put(result, key, difference(set, regions.get(key) ?? null));
      else put(result, key, set && set.slice());
    }
    return { inverted: true, regions: result };
  }
  // both inverted: unselected only where both do not select
  const result = new Map<string, Uint8Array | null>();
  for (const [key, set] of regions) {
    if (b.regions.has(key)) put(result, key, intersection(set, b.regions.get(key) ?? null));
  }
  return { inverted: true, regions: result };
};

/** Removes `b`'s chunks from `a` (MCA Selector Selection.subtract, every inverted combination). */
export const subtract = (a: Selection, b: Selection): Selection => {
  const regions = new Map(a.regions);
  if (!a.inverted && !b.inverted) {
    for (const [key, set] of b.regions) {
      if (regions.has(key)) put(regions, key, difference(regions.get(key) ?? null, set));
    }
    return { inverted: false, regions };
  }
  if (a.inverted && !b.inverted) {
    // a stores what it does not select: b's chunks join that set
    for (const [key, set] of b.regions) {
      if (!regions.has(key)) put(regions, key, set && set.slice());
      else if (regions.get(key) !== null) put(regions, key, union(regions.get(key) ?? null, set));
    }
    return { inverted: true, regions };
  }
  if (!a.inverted) {
    // b is inverted: keep a's chunks that b leaves unselected; regions b selects fully go
    for (const [key, set] of a.regions) {
      if (b.regions.has(key)) put(regions, key, intersection(set, b.regions.get(key) ?? null));
      else regions.delete(key);
    }
    return { inverted: false, regions };
  }
  // both inverted: what b leaves unselected minus what a leaves unselected
  const result = new Map<string, Uint8Array | null>();
  for (const [key, set] of b.regions) {
    put(result, key, a.regions.has(key) ? difference(set, a.regions.get(key) ?? null) : set && set.slice());
  }
  return { inverted: false, regions: result };
};

const uniqueRegions = (existing: Iterable<[number, number]>): Set<string> => {
  const keys = new Set<string>();
  for (const [x, z] of existing) keys.add(regionKey(x, z));
  return keys;
};

/** Selected chunks; inverted selections count only within the `existing` regions. */
export const count = (s: Selection, existing: Iterable<[number, number]>): number => {
  let total = 0;
  if (!s.inverted) {
    for (const set of s.regions.values()) total += set === null ? REGION_CHUNKS : popcount(set);
    return total;
  }
  for (const key of uniqueRegions(existing)) {
    if (!s.regions.has(key)) total += REGION_CHUNKS;
    else {
      const set = s.regions.get(key) as Uint8Array | null;
      if (set !== null) total += REGION_CHUNKS - popcount(set);
    }
  }
  return total;
};

/** Regions with at least one selected chunk; inverted selections count only within the `existing` regions. */
export const selectedRegions = (s: Selection, existing: Iterable<[number, number]>): number => {
  if (!s.inverted) return s.regions.size;
  let total = 0;
  for (const key of uniqueRegions(existing)) if (s.regions.get(key) !== null) total++;
  return total;
};

const sortedEntries = (s: Selection): [number, number, Uint8Array | null][] =>
  [...s.regions]
    .map(([key, set]): [number, number, Uint8Array | null] => [...parseKey(key), set])
    .sort((a, b) => a[0] - b[0] || a[1] - b[1]);

const toBase64 = (bytes: Uint8Array): string => {
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
};

const fromBase64 = (text: string): Uint8Array => {
  let binary: string;
  try {
    binary = atob(text);
  } catch {
    throw new Error('invalid base64 chunk set');
  }
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
};

/** The JSON shape for the backend: regions sorted by x then z, empty sets dropped, full sets as null. */
export const toWire = (s: Selection): WireSelection => ({
  inverted: s.inverted,
  regions: sortedEntries(s).flatMap(([x, z, set]) => {
    if (set === null) return [{ x, z, chunks: null }];
    const n = popcount(set);
    if (n === 0) return [];
    return [{ x, z, chunks: n === REGION_CHUNKS ? null : toBase64(set) }];
  }),
});

/** Reads the backend's JSON shape; throws on bad base64, bitset sizes or coordinates. */
export const fromWire = (w: WireSelection): Selection => {
  const regions = new Map<string, Uint8Array | null>();
  for (const region of w.regions) {
    if (!Number.isInteger(region.x) || !Number.isInteger(region.z)) {
      throw new Error(`invalid region coordinates ${region.x}, ${region.z}`);
    }
    const key = regionKey(region.x, region.z);
    if (region.chunks === null) {
      regions.set(key, null);
      continue;
    }
    const set = fromBase64(region.chunks);
    if (set.length !== SET_BYTES) {
      throw new Error(`region ${region.x}, ${region.z}: chunk set has ${set.length} bytes, expected ${SET_BYTES}`);
    }
    // a region listed twice: both lists count
    put(regions, key, regions.has(key) ? union(regions.get(key) ?? null, set) : set);
  }
  return { inverted: w.inverted, regions };
};

/** MCA Selector's selection CSV (saveToFile): optional `inverted` line, then `rx;rz` for whole regions and
 * `rx;rz;cx;cz` (absolute chunk coordinates) for single chunks; regions sorted, chunks by index. */
export const toCsv = (s: Selection): string => {
  let out = s.inverted ? 'inverted\n' : '';
  for (const [rx, rz, set] of sortedEntries(s)) {
    if (set === null) {
      out += `${rx};${rz}\n`;
      continue;
    }
    for (let i = 0; i < REGION_CHUNKS; i++) {
      if (getBit(set, i)) out += `${rx};${rz};${rx * 32 + (i & 31)};${rz * 32 + (i >> 5)}\n`;
    }
  }
  return out;
};

/** Java's Integer.parseInt: optional sign, ASCII digits, 32-bit range. */
const parseJavaInt = (text: string): number | null => {
  if (!/^[+-]?\d+$/.test(text)) return null;
  const value = Number(text);
  return value >= -2147483648 && value <= 2147483647 ? value : null;
};

/** Java's String.split(";"): trailing empty fields are dropped. */
const javaSplit = (line: string): string[] => {
  if (line === '') return [''];
  const parts = line.split(';');
  while (parts.length > 0 && parts[parts.length - 1] === '') parts.pop();
  return parts;
};

/** Reads MCA Selector's selection CSV with readFromFile's rules; throws `Error("line N: ...")`. */
export const fromCsv = (text: string): Selection => {
  const lines = text.split(/\r\n|\r|\n/);
  // BufferedReader.readLine does not report an empty line after the final line break
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();
  let inverted = false;
  const regions = new Map<string, Uint8Array | null>();
  lines.forEach((line, index) => {
    const num = index + 1;
    if (num === 1 && line === 'inverted') {
      inverted = true;
      return;
    }
    const elements = javaSplit(line);
    if (elements.length !== 2 && elements.length !== 4) {
      throw new Error(`line ${num}: invalid region or chunk coordinate format`);
    }
    const x = parseJavaInt(elements[0]);
    const z = parseJavaInt(elements[1]);
    if (x === null || z === null) throw new Error(`line ${num}: failed to read region coordinates`);
    const key = regionKey(x, z);
    if (elements.length === 2) {
      regions.set(key, null);
      return;
    }
    const cx = parseJavaInt(elements[2]);
    const cz = parseJavaInt(elements[3]);
    if (cx === null || cz === null) throw new Error(`line ${num}: failed to read chunk coordinates`);
    if (cx >> 5 !== x || cz >> 5 !== z) {
      throw new Error(`line ${num}: chunk ${cx};${cz} is not in region ${x};${z}`);
    }
    const mask = new Uint8Array(SET_BYTES);
    const i = chunkIndex(cx, cz);
    mask[i >> 3] |= 1 << (i & 7);
    select(false, regions, key, mask);
  });
  return { inverted, regions };
};

const blockRect = (x1: number, z1: number, x2: number, z2: number): string =>
  `${x1 * 16} ${z1 * 16} -> ${x2 * 16 + 15} ${z2 * 16 + 15}`;

/** Block coordinates of the stored regions for the clipboard, one rectangle per line: whole regions, and
 * the chunks of partial regions merged into rectangles. Inverted selections start with an `inverted`
 * line (the rectangles are then what is NOT selected). */
export const selectionText = (s: Selection): string => {
  const lines: string[] = s.inverted ? ['inverted'] : [];
  for (const [rx, rz, set] of sortedEntries(s)) {
    if (set === null) {
      lines.push(blockRect(rx * 32, rz * 32, rx * 32 + 31, rz * 32 + 31));
      continue;
    }
    // horizontal runs per row, extended downwards while the next row has the same run
    let open = new Map<string, ChunkRect>();
    const done: ChunkRect[] = [];
    for (let cz = 0; cz < 32; cz++) {
      const next = new Map<string, ChunkRect>();
      let cx = 0;
      while (cx < 32) {
        if (!getBit(set, cx + cz * 32)) {
          cx++;
          continue;
        }
        const start = cx;
        while (cx < 32 && getBit(set, cx + cz * 32)) cx++;
        const run = `${start}-${cx - 1}`;
        const rect = open.get(run);
        if (rect) {
          rect.z2 = cz;
          open.delete(run);
          next.set(run, rect);
        } else next.set(run, { x1: start, z1: cz, x2: cx - 1, z2: cz });
      }
      done.push(...open.values());
      open = next;
    }
    done.push(...open.values());
    done.sort((a, b) => a.z1 - b.z1 || a.x1 - b.x1);
    for (const r of done) lines.push(blockRect(rx * 32 + r.x1, rz * 32 + r.z1, rx * 32 + r.x2, rz * 32 + r.z2));
  }
  return lines.join('\n');
};
