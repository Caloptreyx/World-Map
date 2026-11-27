import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import {
  buildBlockMesh,
  landTop,
  lodCell,
  MESH_BLOCK,
  stepForDistance,
  VERTEX_BYTES,
  type MeshData,
  type MeshInput,
  type Sampler,
  type Step,
} from '../frontend/src/lib/mesh.ts';
import { MISSING, type SurfaceData } from '../frontend/src/lib/surface.ts';

const SIZE = 128;
const BASE = -64;

/** A SIZE×SIZE surface, flat at `height` with colour 1 everywhere. */
const flat = (height: number): SurfaceData => ({
  size: SIZE,
  ceiling: false,
  palette: new Uint8Array([0, 0, 0, 100, 150, 50, 200, 180, 120, 30, 60, 200]),
  heights: new Int16Array(SIZE * SIZE).fill(height),
  colours: new Uint16Array(SIZE * SIZE).fill(1),
  depths: new Uint8Array(SIZE * SIZE),
  water: new Uint16Array(SIZE * SIZE),
});

const set = (s: SurfaceData, x: number, z: number, height: number, colour = 1, depth = 0, water = 0) => {
  const i = z * s.size + x;
  s.heights[i] = height;
  s.colours[i] = colour;
  s.depths[i] = depth;
  s.water[i] = water;
};

const constant = (height: number): Sampler => ({ height: () => height });

const build = (surface: SurfaceData, options: Partial<MeshInput> = {}) =>
  buildBlockMesh({
    surface,
    originX: 0,
    originZ: 0,
    localX: 0,
    localZ: 0,
    step: 1,
    neighbours: constant(MISSING),
    base: BASE,
    ...options,
  });

interface Vertex {
  x: number;
  y: number;
  z: number;
  rgb: [number, number, number];
  face: number;
  shade: number;
}

const vertices = (mesh: MeshData): Vertex[] => {
  const view = new DataView(mesh.vertices.buffer, mesh.vertices.byteOffset, mesh.vertices.byteLength);
  const out: Vertex[] = [];
  for (let v = 0; v < mesh.vertexCount; v++) {
    const o = v * VERTEX_BYTES;
    out.push({
      x: view.getInt16(o, true),
      y: view.getInt16(o + 2, true),
      z: view.getInt16(o + 4, true),
      rgb: [view.getUint8(o + 6), view.getUint8(o + 7), view.getUint8(o + 8)],
      face: view.getUint8(o + 9),
      shade: view.getUint8(o + 10),
    });
  }
  return out;
};

/** Quads as their four vertices (the mesher writes each quad as 4 consecutive vertices). */
const quads = (mesh: MeshData): Vertex[][] => {
  const all = vertices(mesh);
  const out: Vertex[][] = [];
  for (let q = 0; q < all.length; q += 4) out.push(all.slice(q, q + 4));
  return out;
};

const walls = (mesh: MeshData) => quads(mesh).filter((quad) => quad[0].face !== 0);
const tops = (mesh: MeshData) => quads(mesh).filter((quad) => quad[0].face === 0);
const ys = (quad: Vertex[]) => quad.map((v) => v.y);

/** Rough natural terrain with every kind of step. */
const noisy = (): SurfaceData => {
  const s = flat(0);
  for (let z = 0; z < SIZE; z++) {
    for (let x = 0; x < SIZE; x++) {
      set(
        s,
        x,
        z,
        60 + Math.round(8 * Math.sin(x / 5) + 6 * Math.cos(z / 7) + ((x * 13 + z * 7) % 3)),
        1 + ((x + z) % 2),
      );
    }
  }
  return s;
};

describe('stepped column mesher', () => {
  test('a flat area merges into a single top quad', () => {
    const mesh = build(flat(10), { neighbours: constant(10) });
    assert.equal(mesh.land.vertexCount, 4);
    assert.equal(mesh.land.indexCount, 6);
    assert.deepEqual([...mesh.land.indices], [0, 1, 2, 0, 2, 3]);
    const [quad] = tops(mesh.land);
    assert.deepEqual(ys(quad), [11, 11, 11, 11]);
    assert.deepEqual(new Set(quad.map((v) => `${v.x},${v.z}`)), new Set(['0,0', '0,64', '64,64', '64,0']));
    assert.deepEqual(quad[0].rgb, [100, 150, 50]);
    assert.equal(quad[0].shade, 255);
    assert.equal(mesh.land.minY, 11);
    assert.equal(mesh.land.maxY, 11);
    assert.equal(mesh.water.vertexCount, 0);
    assert.equal(mesh.land.vertices.length, 4 * VERTEX_BYTES);
  });

  test('tops differing in height or colour are not merged', () => {
    const s = flat(10);
    set(s, 5, 5, 10, 2);
    const mesh = build(s, { neighbours: constant(10) });
    assert.equal(walls(mesh.land).length, 0);
    const colours = tops(mesh.land).map((quad) => quad[0].rgb[0]);
    assert.equal(colours.filter((r) => r === 200).length, 1);
    assert.ok(tops(mesh.land).length <= 5);
  });

  test('a raised column gets four walls and its lower neighbours none', () => {
    const s = flat(10);
    set(s, 10, 20, 13);
    const mesh = build(s, { neighbours: constant(10) });
    const sides = walls(mesh.land);
    assert.deepEqual(sides.map((quad) => quad[0].face).sort(), [1, 2, 3, 4]);
    for (const quad of sides) {
      assert.deepEqual(new Set(ys(quad)), new Set([11, 14]));
      for (const v of quad) assert.ok(v.x >= 10 && v.x <= 11 && v.z >= 20 && v.z <= 21);
      // Ambient occlusion: darker at the foot of the wall.
      for (const v of quad) assert.equal(v.shade, v.y === 11 ? 150 : 255);
    }
    const raised = tops(mesh.land).filter((quad) => quad[0].y === 14);
    assert.equal(raised.length, 1);
  });

  test('no wall where the neighbour is at least as high', () => {
    const s = flat(10);
    for (let x = 0; x < SIZE; x++) set(s, x, 30, 12);
    for (let x = 0; x < SIZE; x++) set(s, x, 31, 12);
    // Outside neighbours as high as the ridge, so the ridge's west end needs no wall either.
    const mesh = build(s, { neighbours: constant(12) });
    const sides = walls(mesh.land);
    // Only the north wall of row 30 and the south wall of row 31, each merged along the whole block edge.
    assert.equal(sides.length, 2);
    assert.deepEqual(sides.map((quad) => quad[0].face).sort(), [1, 2]);
    for (const quad of sides) {
      assert.deepEqual(new Set(quad.map((v) => v.x)), new Set([0, 64]));
      assert.deepEqual(new Set(ys(quad)), new Set([11, 13]));
    }
  });

  test('skirts drop to the base toward missing neighbours outside the block', () => {
    const mesh = build(flat(10));
    // North and west of the block lie outside the region (sampler says MISSING); east and south are loaded.
    const sides = walls(mesh.land);
    assert.deepEqual(sides.map((quad) => quad[0].face).sort(), [1, 4]);
    for (const quad of sides) assert.deepEqual(new Set(ys(quad)), new Set([BASE, 11]));
    assert.equal(mesh.land.minY, BASE);
    assert.equal(mesh.land.maxY, 11);
  });

  test('the sampler is asked for the cells just outside the region at the block step', () => {
    const asked: string[] = [];
    const sampler: Sampler = {
      height: (x, z, step) => {
        asked.push(`${x},${z},${step}`);
        return 10;
      },
    };
    build(flat(10), { neighbours: sampler, originX: 1024, originZ: -512, step: 4 });
    assert.ok(asked.includes('1020,-512,4'));
    assert.ok(asked.includes('1024,-516,4'));
    assert.ok(asked.includes('1084,-516,4'));
    assert.ok(!asked.some((key) => key.startsWith('1088,')), 'east neighbours are read from the region');
    assert.equal(asked.length, 32);
  });

  test('skirts toward missing columns inside the block', () => {
    const s = flat(10);
    set(s, 20, 20, MISSING, 0);
    const mesh = build(s, { neighbours: constant(10) });
    const sides = walls(mesh.land);
    assert.equal(sides.length, 4);
    for (const quad of sides) assert.deepEqual(new Set(ys(quad)), new Set([BASE, 11]));
    const faces = sides.map((quad) => quad[0].face).sort();
    assert.deepEqual(faces, [1, 2, 3, 4]);
    // The missing column itself has no top.
    for (const quad of tops(mesh.land)) {
      const xs = quad.map((v) => v.x);
      const zs = quad.map((v) => v.z);
      const covers = Math.min(...xs) <= 20 && Math.max(...xs) >= 21 && Math.min(...zs) <= 20 && Math.max(...zs) >= 21;
      assert.ok(!covers);
    }
  });

  test('water: floor as land top, a separate water plane with depth shade, shore walls', () => {
    const s = flat(62);
    for (let z = 0; z < 16; z++) {
      for (let x = 0; x < 16; x++) set(s, x, z, 62, 2, 5, 3);
    }
    const mesh = build(s, { neighbours: constant(62) });
    const water = quads(mesh.water);
    assert.equal(water.length, 1);
    assert.deepEqual(ys(water[0]), [63, 63, 63, 63]);
    assert.deepEqual(water[0][0].rgb, [30, 60, 200]);
    assert.ok(water[0].every((v) => v.shade > 0 && v.shade <= 5));
    const floor = tops(mesh.land).filter((quad) => quad[0].y === 58);
    assert.equal(floor.length, 1);
    assert.deepEqual(floor[0][0].rgb, [200, 180, 120]);
    // The dry shore (land top 62) walls the basin from its floor up: west face at x=16, north face at z=16.
    const shore = walls(mesh.land);
    assert.deepEqual(shore.map((quad) => quad[0].face).sort(), [1, 4]);
    for (const quad of shore) assert.deepEqual(new Set(ys(quad)), new Set([58, 63]));
  });

  test('water sides close the water body toward missing chunks only', () => {
    const s = flat(62);
    for (let z = 0; z < SIZE; z++) {
      for (let x = 0; x < SIZE; x++) set(s, x, z, 62, 2, 10, 3);
    }
    const loaded = build(s, { neighbours: constant(52) });
    assert.equal(walls(loaded.water).length, 0);
    const cut = build(s);
    const sides = walls(cut.water);
    assert.deepEqual(sides.map((quad) => quad[0].face).sort(), [1, 4]);
    for (const quad of sides) assert.deepEqual(new Set(ys(quad)), new Set([53, 63]));
  });

  test('triangles wind counter-clockwise seen from the side they face', () => {
    const mesh = build(noisy());
    const all = vertices(mesh.land);
    const normals: Record<number, [number, number, number]> = {
      0: [0, 1, 0],
      1: [0, 0, -1],
      2: [0, 0, 1],
      3: [1, 0, 0],
      4: [-1, 0, 0],
    };
    assert.ok(walls(mesh.land).length > 50);
    const index = mesh.land.indices;
    for (let t = 0; t < mesh.land.indexCount; t += 3) {
      const [a, b, c] = [all[index[t]], all[index[t + 1]], all[index[t + 2]]];
      const u = [b.x - a.x, b.y - a.y, b.z - a.z];
      const v = [c.x - a.x, c.y - a.y, c.z - a.z];
      const n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
      const expected = normals[a.face];
      const dot = n[0] * expected[0] + n[1] * expected[1] + n[2] * expected[2];
      assert.ok(dot > 0, `triangle ${t / 3} faces away from face ${a.face}`);
    }
  });

  test('every position stays inside the block', () => {
    for (const step of [1, 2, 4, 8] as Step[]) {
      const mesh = build(noisy(), { step, localX: 64, localZ: 64 });
      for (const v of vertices(mesh.land)) {
        assert.ok(v.x >= 0 && v.x <= MESH_BLOCK && v.z >= 0 && v.z <= MESH_BLOCK);
        assert.equal(v.x % step, 0);
        assert.equal(v.z % step, 0);
      }
    }
  });

  test('huge heights are clamped to the Int16 range', () => {
    const mesh = build(flat(32767), { neighbours: constant(32767) });
    assert.deepEqual(ys(tops(mesh.land)[0]), [32767, 32767, 32767, 32767]);
  });
});

describe('level of detail', () => {
  test('lodCell keeps the tallest non-missing column of the cell', () => {
    const s = flat(10);
    set(s, 0, 0, MISSING, 0);
    set(s, 1, 1, 14, 2, 3, 3);
    set(s, 3, 3, 20);
    assert.deepEqual(lodCell(s, 0, 0, 1), null);
    assert.deepEqual(lodCell(s, 0, 0, 2), { height: 14, colour: 2, depth: 3, water: 3 });
    assert.equal(landTop(s, 0, 0, 2), 11);
    assert.deepEqual(lodCell(s, 0, 0, 4), { height: 20, colour: 1, depth: 0, water: 0 });
    assert.equal(landTop(s, 0, 0, 1), MISSING);
    for (let z = 0; z < 4; z++) for (let x = 0; x < 4; x++) set(s, x, z, MISSING, 0);
    assert.equal(lodCell(s, 0, 0, 4), null);
  });

  test('2×2 and 4×4 downsampling use step-sized cells and fewer vertices', () => {
    const s = noisy();
    const counts = ([1, 2, 4, 8] as Step[]).map((step) => build(s, { step }).land.vertexCount);
    for (let i = 1; i < counts.length; i++) assert.ok(counts[i] < counts[i - 1], `${counts}`);
    const coarse = build(flat(10), { step: 4, neighbours: constant(10) });
    assert.equal(coarse.land.vertexCount, 4);
  });

  test('a lone peak survives downsampling at full height', () => {
    const s = flat(10);
    set(s, 13, 13, 40);
    const mesh = build(s, { step: 4, neighbours: constant(10) });
    const peak = tops(mesh.land).filter((quad) => quad[0].y === 41);
    assert.equal(peak.length, 1);
    assert.deepEqual(new Set(peak[0].map((v) => `${v.x},${v.z}`)), new Set(['12,12', '12,16', '16,16', '16,12']));
  });

  test('stepForDistance', () => {
    assert.equal(stepForDistance(0), 1);
    assert.equal(stepForDistance(191), 1);
    assert.equal(stepForDistance(192), 2);
    assert.equal(stepForDistance(600), 4);
    assert.equal(stepForDistance(5000), 8);
  });
});
