// Stepped voxel-column mesher: turns a 64×64 block of a region's surface into Minecraft-like terraces, a flat
// top per column at its real height and vertical walls down to each lower neighbour, plus a separate water mesh.

import type { SurfaceData } from './surface.ts';
import { MISSING } from './surface.ts';

/**
 * Interleaved vertex, 12 bytes: Int16 x, y, z (x/z local to the block, y absolute world Y), Uint8 r, g, b
 * (unshaded colour), Uint8 face (0 top, 1 north -Z, 2 south +Z, 3 east +X, 4 west -X), Uint8 shade (ambient
 * occlusion 0..255, or the water depth on water vertices), Uint8 padding.
 */
export const VERTEX_BYTES = 12;
/** Columns per mesh block side (8×8 blocks per region). */
export const MESH_BLOCK = 64;

export type Step = 1 | 2 | 4 | 8;

export interface Sampler {
  /** Top-block height of the LOD cell whose min corner is the world column (x, z) at `step`, or MISSING.
   *  Works across region borders (returns MISSING when the neighbour region is not loaded). The mesher
   *  expects the LAND top: the floor (`height - depth`) for wet cells, see `landTop`. */
  height(x: number, z: number, step: Step): number;
}

export interface MeshInput {
  surface: SurfaceData;
  /** World block coords of the block's min corner (multiple of MESH_BLOCK). */
  originX: number;
  originZ: number;
  /** The block's min corner inside the region (multiple of MESH_BLOCK). */
  localX: number;
  localZ: number;
  step: Step;
  /** Heights of the columns just outside the region (it reads its own region's columns directly). */
  neighbours: Sampler;
  /** Y the boundary skirts drop to where a neighbour is MISSING. */
  base: number;
}

export interface MeshData {
  /** VERTEX_BYTES per vertex. */
  vertices: Uint8Array;
  /** Triangles, counter-clockwise seen from outside. */
  indices: Uint32Array;
  vertexCount: number;
  indexCount: number;
  /** Y range of the vertices (0, 0 when empty); the x/z box is origin .. origin + MESH_BLOCK. */
  minY: number;
  maxY: number;
}

export interface BlockMesh {
  land: MeshData;
  water: MeshData;
}

const FACE_TOP = 0;
const FACE_NORTH = 1;
const FACE_SOUTH = 2;
const FACE_EAST = 3;
const FACE_WEST = 4;
type Face = 0 | 1 | 2 | 3 | 4;

const LIT = 255;
/** Shade at the foot of a wall, where a real block would be in its neighbour's shadow. */
const WALL_FOOT_SHADE = 150;
/** Water depths merge into these buckets so a sea floor of varying depth still becomes few quads. */
const DEPTH_BUCKETS = [1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 128, 192, 255];
/** Height of a loaded neighbour outside the region whose water surface is unknown: no water wall toward it. */
const UNKNOWN_SURFACE = 32767;

const SIDES: { face: Face; di: number; dj: number }[] = [
  { face: FACE_NORTH, di: 0, dj: -1 },
  { face: FACE_SOUTH, di: 0, dj: 1 },
  { face: FACE_EAST, di: 1, dj: 0 },
  { face: FACE_WEST, di: -1, dj: 0 },
];

const depthBucket = (depth: number): number => {
  let bucket = 0;
  for (const value of DEPTH_BUCKETS) {
    if (value > depth) break;
    bucket = value;
  }
  return bucket;
};

/** Index of the tallest non-missing column of the step×step cell at (lx, lz), or -1 when all are missing. */
const pickColumn = (s: SurfaceData, lx: number, lz: number, step: Step): number => {
  let best = -1;
  let bestHeight = MISSING;
  const maxX = Math.min(lx + step, s.size);
  const maxZ = Math.min(lz + step, s.size);
  for (let z = lz; z < maxZ; z++) {
    for (let i = z * s.size + lx, end = z * s.size + maxX; i < end; i++) {
      const height = s.heights[i];
      if (height !== MISSING && (best < 0 || height > bestHeight)) {
        best = i;
        bestHeight = height;
      }
    }
  }
  return best;
};

/** Downsampled cell (tallest non-missing column of the step×step cell, with its colours/depth). */
export const lodCell = (
  s: SurfaceData,
  lx: number,
  lz: number,
  step: Step,
): { height: number; colour: number; depth: number; water: number } | null => {
  const i = pickColumn(s, lx, lz, step);
  if (i < 0) return null;
  return { height: s.heights[i], colour: s.colours[i], depth: s.depths[i], water: s.water[i] };
};

/** Land top block y of the LOD cell (the floor for wet cells), or MISSING: what a `Sampler` returns. */
export const landTop = (s: SurfaceData, lx: number, lz: number, step: Step): number => {
  const i = pickColumn(s, lx, lz, step);
  return i < 0 ? MISSING : s.heights[i] - s.depths[i];
};

/** Mesh detail for a block whose nearest point is `distance` blocks from the camera. */
export const stepForDistance = (distance: number): Step => {
  if (distance < 192) return 1;
  if (distance < 512) return 2;
  if (distance < 1024) return 4;
  return 8;
};

/** Growable vertex/index buffers; capacity doubles so a block costs only a few reallocations. */
class MeshWriter {
  bytes = new Uint8Array(256 * VERTEX_BYTES);
  view = new DataView(this.bytes.buffer);
  indices = new Uint32Array(384);
  vertexCount = 0;
  indexCount = 0;
  minY = Infinity;
  maxY = -Infinity;

  /** Appends a quad; corners counter-clockwise seen from the side it faces. */
  quad(face: Face, rgb: number, corners: number[], shades: number[]): void {
    if ((this.vertexCount + 4) * VERTEX_BYTES > this.bytes.length) {
      const bytes = new Uint8Array(this.bytes.length * 2);
      bytes.set(this.bytes);
      this.bytes = bytes;
      this.view = new DataView(bytes.buffer);
    }
    if (this.indexCount + 6 > this.indices.length) {
      const indices = new Uint32Array(this.indices.length * 2);
      indices.set(this.indices);
      this.indices = indices;
    }
    const first = this.vertexCount;
    for (let v = 0; v < 4; v++) {
      const offset = (first + v) * VERTEX_BYTES;
      // World Y is absolute, so clamp it into the Int16 position range.
      const y = Math.max(-32768, Math.min(32767, corners[v * 3 + 1]));
      this.view.setInt16(offset, corners[v * 3], true);
      this.view.setInt16(offset + 2, y, true);
      this.view.setInt16(offset + 4, corners[v * 3 + 2], true);
      this.bytes[offset + 6] = rgb >> 16;
      this.bytes[offset + 7] = (rgb >> 8) & 0xff;
      this.bytes[offset + 8] = rgb & 0xff;
      this.bytes[offset + 9] = face;
      this.bytes[offset + 10] = shades[v];
      this.bytes[offset + 11] = 0;
      if (y < this.minY) this.minY = y;
      if (y > this.maxY) this.maxY = y;
    }
    const at = this.indexCount;
    const indices = this.indices;
    indices[at] = first;
    indices[at + 1] = first + 1;
    indices[at + 2] = first + 2;
    indices[at + 3] = first;
    indices[at + 4] = first + 2;
    indices[at + 5] = first + 3;
    this.vertexCount += 4;
    this.indexCount += 6;
  }

  finish(): MeshData {
    const empty = this.vertexCount === 0;
    return {
      vertices: this.bytes.subarray(0, this.vertexCount * VERTEX_BYTES),
      indices: this.indices.subarray(0, this.indexCount),
      vertexCount: this.vertexCount,
      indexCount: this.indexCount,
      minY: empty ? 0 : this.minY,
      maxY: empty ? 0 : this.maxY,
    };
  }
}

/** Cells of one block plus a one-cell ring of neighbours, row-major with stride `pad` (= cells + 2). */
interface Grid {
  cells: number;
  pad: number;
  step: Step;
  palette: Uint8Array;
}

const cellIndex = (grid: Grid, i: number, j: number): number => (j + 1) * grid.pad + i + 1;

const rgbOf = (palette: Uint8Array, index: number): number =>
  index * 3 + 2 < palette.length
    ? (palette[index * 3] << 16) | (palette[index * 3 + 1] << 8) | palette[index * 3 + 2]
    : 0;

/** Greedy-merged top quads at `top + 1` for every cell with a `top`, merged on equal top, tint and shade. */
const emitTops = (out: MeshWriter, grid: Grid, top: Int32Array, tint: Uint16Array, shade: Uint8Array): void => {
  const { cells, pad, step } = grid;
  const done = new Uint8Array(pad * pad);
  const matches = (k: number, c: number) =>
    !done[k] && top[k] === top[c] && tint[k] === tint[c] && shade[k] === shade[c];
  for (let j = 0; j < cells; j++) {
    for (let i = 0; i < cells; i++) {
      const c = cellIndex(grid, i, j);
      if (done[c] || top[c] === MISSING) continue;
      let width = 1;
      while (i + width < cells && matches(c + width, c)) width++;
      let depth = 1;
      rows: while (j + depth < cells) {
        const row = c + depth * pad;
        for (let k = 0; k < width; k++) {
          if (!matches(row + k, c)) break rows;
        }
        depth++;
      }
      for (let k = 0; k < depth; k++) done.fill(1, c + k * pad, c + k * pad + width);
      const y = top[c] + 1;
      const x0 = i * step;
      const x1 = (i + width) * step;
      const z0 = j * step;
      const z1 = (j + depth) * step;
      const s = shade[c];
      out.quad(FACE_TOP, rgbOf(grid.palette, tint[c]), [x0, y, z0, x0, y, z1, x1, y, z1, x1, y, z0], [s, s, s, s]);
    }
  }
};

/**
 * Vertical walls on each side of every cell with a `top`, from `bottomOf(cell, neighbour)` up to the cell's top
 * face (none when the bottom is not below it), merged along the edge on equal bottom, top, tint and shade.
 * `foot` darkens the bottom vertices (ambient occlusion); otherwise they keep the cell's shade.
 */
const emitWalls = (
  out: MeshWriter,
  grid: Grid,
  top: Int32Array,
  tint: Uint16Array,
  shade: Uint8Array,
  foot: number | null,
  bottomOf: (cell: number, neighbour: number) => number,
): void => {
  const { cells, pad, step } = grid;
  for (const { face, di, dj } of SIDES) {
    // North/south walls run along x, east/west walls along z.
    const alongX = dj !== 0;
    const offset = dj * pad + di;
    for (let line = 0; line < cells; line++) {
      const at = (k: number) => (alongX ? cellIndex(grid, k, line) : cellIndex(grid, line, k));
      let k = 0;
      while (k < cells) {
        const c = at(k);
        if (top[c] === MISSING) {
          k++;
          continue;
        }
        const roof = top[c] + 1;
        const bottom = bottomOf(c, c + offset);
        if (bottom >= roof) {
          k++;
          continue;
        }
        let end = k + 1;
        for (; end < cells; end++) {
          const d = at(end);
          if (top[d] !== top[c] || tint[d] !== tint[c] || shade[d] !== shade[c] || bottomOf(d, d + offset) !== bottom) {
            break;
          }
        }
        const from = k * step;
        const to = end * step;
        const edge = (dj > 0 || di > 0 ? line + 1 : line) * step;
        // The two bottom corners, ordered so the quad winds counter-clockwise seen from outside.
        let ax: number, az: number, bx: number, bz: number;
        if (face === FACE_NORTH) [ax, az, bx, bz] = [to, edge, from, edge];
        else if (face === FACE_SOUTH) [ax, az, bx, bz] = [from, edge, to, edge];
        else if (face === FACE_EAST) [ax, az, bx, bz] = [edge, to, edge, from];
        else [ax, az, bx, bz] = [edge, from, edge, to];
        const lit = shade[c];
        const low = foot ?? lit;
        out.quad(
          face,
          rgbOf(grid.palette, tint[c]),
          [ax, bottom, az, bx, bottom, bz, bx, roof, bz, ax, roof, az],
          [low, low, lit, lit],
        );
        k = end;
      }
    }
  }
};

export const buildBlockMesh = (input: MeshInput): BlockMesh => {
  const { surface: s, step, localX, localZ, originX, originZ, neighbours, base } = input;
  const cells = MESH_BLOCK / step;
  const pad = cells + 2;
  const grid: Grid = { cells, pad, step, palette: s.palette };
  const area = pad * pad;
  /** Land top block y (the floor under water). */
  const land = new Int32Array(area).fill(MISSING);
  /** Visible top block y: the water surface for wet cells, the land top for dry ones. */
  const surface = new Int32Array(area).fill(MISSING);
  /** Water surface block y of wet cells only. */
  const wet = new Int32Array(area).fill(MISSING);
  const colour = new Uint16Array(area);
  const waterColour = new Uint16Array(area);
  const depth = new Uint8Array(area);
  const lit = new Uint8Array(area).fill(LIT);

  const read = (i: number, j: number) => {
    const c = cellIndex(grid, i, j);
    const lx = localX + i * step;
    const lz = localZ + j * step;
    if (lx < 0 || lz < 0 || lx >= s.size || lz >= s.size) {
      const height = neighbours.height(originX + i * step, originZ + j * step, step);
      land[c] = height;
      surface[c] = height === MISSING ? MISSING : UNKNOWN_SURFACE;
      return;
    }
    const k = pickColumn(s, lx, lz, step);
    if (k < 0) return;
    const height = s.heights[k];
    const d = s.depths[k];
    land[c] = height - d;
    surface[c] = height;
    colour[c] = s.colours[k];
    if (d > 0) {
      wet[c] = height;
      waterColour[c] = s.water[k];
      depth[c] = depthBucket(d);
    }
  };
  for (let j = 0; j < cells; j++) {
    for (let i = 0; i < cells; i++) read(i, j);
  }
  for (let k = 0; k < cells; k++) {
    read(k, -1);
    read(k, cells);
    read(-1, k);
    read(cells, k);
  }
  // Ring cells only feed the walls: drop their water so no water quad or wall is built for them.
  for (let k = 0; k < cells; k++) {
    wet[cellIndex(grid, k, -1)] = MISSING;
    wet[cellIndex(grid, k, cells)] = MISSING;
    wet[cellIndex(grid, -1, k)] = MISSING;
    wet[cellIndex(grid, cells, k)] = MISSING;
  }

  const landOut = new MeshWriter();
  emitTops(landOut, grid, land, colour, lit);
  emitWalls(landOut, grid, land, colour, lit, WALL_FOOT_SHADE, (_, n) => (land[n] === MISSING ? base : land[n] + 1));

  const waterOut = new MeshWriter();
  emitTops(waterOut, grid, wet, waterColour, depth);
  // Water sides only where the neighbour's visible surface is lower (shores, cut-offs at missing chunks),
  // starting at this cell's floor so they never overlap the land walls.
  emitWalls(waterOut, grid, wet, waterColour, depth, null, (c, n) =>
    Math.max(surface[n] === MISSING ? -Infinity : surface[n] + 1, land[c] + 1),
  );

  return { land: landOut.finish(), water: waterOut.finish() };
};
