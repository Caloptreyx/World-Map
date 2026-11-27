// Picking against stepped columns: each column is a solid from its top face (Y = top + 1) down, so a ray hits
// either a top face or the side of a column it runs into below that column's top.

export interface Hit {
  /** The hit column and its top block y. */
  x: number;
  y: number;
  z: number;
  /** 0 top, 1 north (-Z), 2 south (+Z), 3 east (+X), 4 west (-X), as in the mesh vertex layout. */
  face: 0 | 1 | 2 | 3 | 4;
  /** Along the ray from `origin`, in blocks. */
  distance: number;
}

/** Walks the columns a ray crosses (2D DDA over x/z) and returns the first stepped column it hits: the top
 *  face (Y = top+1) or a side wall. `top(x, z)` returns the column's land/floor or water surface top block y,
 *  or null when unknown. World coordinates (add the origin before calling). */
export const raycastColumns = (
  origin: [number, number, number],
  dir: [number, number, number],
  top: (x: number, z: number) => number | null,
  maxDistance: number,
): Hit | null => {
  const length = Math.hypot(dir[0], dir[1], dir[2]);
  if (length === 0) return null;
  const [ox, oy, oz] = origin;
  const dx = dir[0] / length;
  const dy = dir[1] / length;
  const dz = dir[2] / length;

  let x = Math.floor(ox);
  let z = Math.floor(oz);
  const stepX = Math.sign(dx);
  const stepZ = Math.sign(dz);
  // Ray distance to the next x / z cell border, and between borders.
  const deltaX = Math.abs(1 / dx);
  const deltaZ = Math.abs(1 / dz);
  let nextX = dx > 0 ? (x + 1 - ox) * deltaX : dx < 0 ? (ox - x) * deltaX : Infinity;
  let nextZ = dz > 0 ? (z + 1 - oz) * deltaZ : dz < 0 ? (oz - z) * deltaZ : Infinity;
  let enter = 0;
  /** The side the ray came in through, null in the starting column. */
  let side: 1 | 2 | 3 | 4 | null = null;

  while (enter <= maxDistance) {
    const exit = Math.min(nextX, nextZ);
    const height = top(x, z);
    if (height !== null) {
      const roof = height + 1;
      if (side !== null && oy + dy * enter <= roof) return { x, y: height, z, face: side, distance: enter };
      if (dy < 0) {
        const t = (roof - oy) / dy;
        // A starting column the eye is already inside (t < 0) is not hit.
        if (t >= enter && t <= exit && t <= maxDistance) return { x, y: height, z, face: 0, distance: t };
      }
    }
    if (nextX < nextZ) {
      x += stepX;
      enter = nextX;
      nextX += deltaX;
      side = stepX > 0 ? 4 : 3;
    } else {
      z += stepZ;
      enter = nextZ;
      nextZ += deltaZ;
      side = stepZ > 0 ? 1 : 2;
    }
  }
  return null;
};
