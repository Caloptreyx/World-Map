// Map geometry: blocks, chunks (16 blocks), regions (32 chunks, one 512 px tile each) and the
// screen. A view is the block at the screen's centre plus a zoom in screen pixels per block.

export const REGION_BLOCKS = 512;
export const CHUNK_BLOCKS = 16;
export const MIN_ZOOM = 1 / 16;
export const MAX_ZOOM = 16;
/** Beyond this many regions on screen, missing tiles are not requested (zoom in to load them). */
export const MAX_TILE_REQUESTS = 64;

export interface View {
  x: number;
  z: number;
  zoom: number;
}

export interface Size {
  width: number;
  height: number;
}

export interface Bounds {
  minX: number;
  minZ: number;
  maxX: number;
  maxZ: number;
}

/** An inclusive rectangle of chunks, as the delete endpoint takes it. */
export interface ChunkRect {
  x1: number;
  z1: number;
  x2: number;
  z2: number;
}

export const chunkOf = (block: number): number => Math.floor(block / CHUNK_BLOCKS);
export const regionOf = (block: number): number => Math.floor(block / REGION_BLOCKS);

export const clampZoom = (zoom: number): number => Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, zoom));

export const toScreen = (view: View, size: Size, x: number, z: number): [number, number] => [
  (x - view.x) * view.zoom + size.width / 2,
  (z - view.z) * view.zoom + size.height / 2,
];

export const toWorld = (view: View, size: Size, sx: number, sy: number): [number, number] => [
  view.x + (sx - size.width / 2) / view.zoom,
  view.z + (sy - size.height / 2) / view.zoom,
];

/** The block area on screen. */
export const visibleBounds = (view: View, size: Size): Bounds => {
  const [minX, minZ] = toWorld(view, size, 0, 0);
  const [maxX, maxZ] = toWorld(view, size, size.width, size.height);
  return { minX, minZ, maxX, maxZ };
};

/** Regions on screen, nearest to the centre first (the order tiles load in). */
export const visibleRegions = (view: View, size: Size): [number, number][] => {
  const bounds = visibleBounds(view, size);
  const regions: [number, number][] = [];
  for (let z = regionOf(bounds.minZ); z <= regionOf(bounds.maxZ); z++) {
    for (let x = regionOf(bounds.minX); x <= regionOf(bounds.maxX); x++) {
      regions.push([x, z]);
    }
  }
  const centre = (r: number) => r * REGION_BLOCKS + REGION_BLOCKS / 2;
  const distance = ([x, z]: [number, number]) => (centre(x) - view.x) ** 2 + (centre(z) - view.z) ** 2;
  return regions.sort((a, b) => distance(a) - distance(b));
};

/** Zooms by `factor` keeping the block under the screen point `(sx, sy)` in place. */
export const zoomAt = (view: View, size: Size, sx: number, sy: number, factor: number): View => {
  const zoom = clampZoom(view.zoom * factor);
  const [wx, wz] = toWorld(view, size, sx, sy);
  return {
    zoom,
    x: wx - (sx - size.width / 2) / zoom,
    z: wz - (sy - size.height / 2) / zoom,
  };
};

/** The chunk rectangle spanned by two blocks, corners ordered. */
export const chunkRect = (x1: number, z1: number, x2: number, z2: number): ChunkRect => ({
  x1: Math.min(chunkOf(x1), chunkOf(x2)),
  z1: Math.min(chunkOf(z1), chunkOf(z2)),
  x2: Math.max(chunkOf(x1), chunkOf(x2)),
  z2: Math.max(chunkOf(z1), chunkOf(z2)),
});

export const rectArea = (rect: ChunkRect): number => (rect.x2 - rect.x1 + 1) * (rect.z2 - rect.z1 + 1);

/** How many chunks a selection covers, overlaps counted once. Large selections (more than
 * `exactLimit` chunks before overlaps) count their rectangles' areas instead. */
export const selectedChunks = (rects: ChunkRect[], exactLimit = 200_000): number => {
  const total = rects.reduce((sum, rect) => sum + rectArea(rect), 0);
  if (total > exactLimit) return total;
  const seen = new Set<string>();
  for (const rect of rects) {
    for (let z = rect.z1; z <= rect.z2; z++) {
      for (let x = rect.x1; x <= rect.x2; x++) {
        seen.add(`${x},${z}`);
      }
    }
  }
  return seen.size;
};

const covers = (rect: ChunkRect, x: number, z: number): boolean =>
  x >= rect.x1 && x <= rect.x2 && z >= rect.z1 && z <= rect.z2;

/** Adds the chunk `(x, z)` to a selection, or removes it when it is selected: covering rectangles
 * are split around it (up to four pieces each), so the rest of them stays selected. */
export const toggleChunk = (rects: ChunkRect[], x: number, z: number): ChunkRect[] => {
  if (!rects.some((rect) => covers(rect, x, z))) return [...rects, { x1: x, z1: z, x2: x, z2: z }];
  return rects.flatMap((rect) => {
    if (!covers(rect, x, z)) return [rect];
    const pieces: ChunkRect[] = [
      { ...rect, z2: z - 1 },
      { ...rect, z1: z + 1 },
      { x1: rect.x1, z1: z, x2: x - 1, z2: z },
      { x1: x + 1, z1: z, x2: rect.x2, z2: z },
    ];
    return pieces.filter((piece) => piece.x1 <= piece.x2 && piece.z1 <= piece.z2);
  });
};

type Pair = [[number, number], [number, number]];

/** The view while two fingers pinch: the world point under the start midpoint follows the current
 * midpoint, and the zoom scales with the distance between the fingers. */
export const pinchView = (start: View, size: Size, from: Pair, to: Pair): View => {
  const mid = (pair: Pair): [number, number] => [(pair[0][0] + pair[1][0]) / 2, (pair[0][1] + pair[1][1]) / 2];
  const span = (pair: Pair) => Math.max(1, Math.hypot(pair[0][0] - pair[1][0], pair[0][1] - pair[1][1]));
  const zoom = clampZoom(start.zoom * (span(to) / span(from)));
  const [wx, wz] = toWorld(start, size, ...mid(from));
  const [sx, sy] = mid(to);
  return { zoom, x: wx - (sx - size.width / 2) / zoom, z: wz - (sy - size.height / 2) / zoom };
};

/** Screen cell size for grouping entities: coarse when zoomed out so groups do not hide the terrain. */
export const clusterCell = (zoom: number): number =>
  zoom < 1 ? 96 : zoom < 2 ? 72 : zoom < 4 ? 56 : zoom < 8 ? 40 : 28;

/** Block coordinates of a selection for the clipboard, one rectangle per line. */
export const selectionText = (rects: ChunkRect[]): string =>
  rects
    .map(
      (rect) =>
        `${rect.x1 * CHUNK_BLOCKS} ${rect.z1 * CHUNK_BLOCKS} -> ${rect.x2 * CHUNK_BLOCKS + 15} ${rect.z2 * CHUNK_BLOCKS + 15}`,
    )
    .join('\n');

/** A colour for a heat value: transparent blue to opaque red, on a logarithmic scale. */
export const heatColour = (count: number, max: number): string => {
  if (count <= 0 || max <= 0) return 'rgba(0, 0, 0, 0)';
  const t = Math.min(1, Math.log1p(count) / Math.log1p(max));
  const stops: [number, number, number][] = [
    [40, 90, 255],
    [40, 220, 120],
    [250, 220, 40],
    [240, 50, 40],
  ];
  const scaled = t * (stops.length - 1);
  const index = Math.min(stops.length - 2, Math.floor(scaled));
  const local = scaled - index;
  const [r, g, b] = stops[index].map((c, i) => Math.round(c + (stops[index + 1][i] - c) * local));
  const alpha = (0.25 + 0.5 * t).toFixed(2);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
};

export interface Point {
  x: number;
  z: number;
}

export interface Cluster<T extends Point> {
  /** Screen position of the cluster (the average of its members). */
  sx: number;
  sy: number;
  members: T[];
}

/** Groups points that fall into the same `cell`-pixel square on screen. */
export const cluster = <T extends Point>(points: T[], view: View, size: Size, cell: number): Cluster<T>[] => {
  const groups = new Map<string, Cluster<T>>();
  for (const point of points) {
    const [sx, sy] = toScreen(view, size, point.x, point.z);
    if (sx < -cell || sy < -cell || sx > size.width + cell || sy > size.height + cell) continue;
    const key = `${Math.floor(sx / cell)},${Math.floor(sy / cell)}`;
    const group = groups.get(key);
    if (group) {
      const n = group.members.length;
      group.sx = (group.sx * n + sx) / (n + 1);
      group.sy = (group.sy * n + sy) / (n + 1);
      group.members.push(point);
    } else {
      groups.set(key, { sx, sy, members: [point] });
    }
  }
  return [...groups.values()];
};
