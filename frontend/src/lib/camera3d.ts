// 3D camera math: a map camera orbiting a ground target and a free fly camera, plus the matrices, projection,
// picking rays and frustum culling the renderer needs. World axes: X east, Z south, Y up; yaw 0 looks north
// (-Z) and grows clockwise seen from above. Matrices are column-major and camera-relative (world minus origin).

export type Mode = 'map' | 'fly';

export interface Camera3D {
  mode: Mode;
  /** Map: orbit target on the ground; fly: eye position. */
  x: number;
  y: number;
  z: number;
  /** Radians, 0 = north, clockwise. */
  yaw: number;
  /** Map: downward tilt MIN_PITCH..MAX_PITCH; fly: -FLY_PITCH..FLY_PITCH (+ = up). */
  pitch: number;
  /** Map: eye-to-target distance MIN_DISTANCE..MAX_DISTANCE (kept in fly mode for the way back). */
  distance: number;
}

type Vec3 = [number, number, number];

const DEG = Math.PI / 180;
export const FOV_Y = 55 * DEG;
export const MIN_PITCH = 15 * DEG;
export const MAX_PITCH = 85 * DEG;
export const FLY_PITCH = 89 * DEG;
export const MIN_DISTANCE = 8;
/** The zoom-out limit: keeps the number of regions on screen (and memory) bounded. */
export const MAX_DISTANCE = 1400;
export const DEFAULT_PITCH = 50 * DEG;
export const DEFAULT_YAW = 0;
export const DEFAULT_DISTANCE = 220;
/** How far in front of a point `flyTo` puts the fly camera, and how steeply it then looks down. */
const FLY_TO_DISTANCE = 48;
const FLY_TO_PITCH = 30 * DEG;

const TAU = Math.PI * 2;
const clamp = (value: number, min: number, max: number): number => Math.min(max, Math.max(min, value));

/** Blocks per screen pixel at `distance` in front of the camera. */
const blocksPerPixel = (distance: number, viewportHeight: number): number =>
  (2 * distance * Math.tan(FOV_Y / 2)) / viewportHeight;

/** Unit view direction for a yaw and an upward pitch. */
const direction = (yaw: number, pitchUp: number): Vec3 => [
  Math.cos(pitchUp) * Math.sin(yaw),
  Math.sin(pitchUp),
  -Math.cos(pitchUp) * Math.cos(yaw),
];

export const forward = (cam: Camera3D): Vec3 => direction(cam.yaw, cam.mode === 'map' ? -cam.pitch : cam.pitch);

export const eye = (cam: Camera3D): Vec3 => {
  if (cam.mode === 'fly') return [cam.x, cam.y, cam.z];
  const [fx, fy, fz] = forward(cam);
  return [cam.x - fx * cam.distance, cam.y - fy * cam.distance, cam.z - fz * cam.distance];
};

export const clampCamera = (cam: Camera3D): Camera3D => ({
  ...cam,
  yaw: ((cam.yaw % TAU) + TAU) % TAU,
  pitch: cam.mode === 'map' ? clamp(cam.pitch, MIN_PITCH, MAX_PITCH) : clamp(cam.pitch, -FLY_PITCH, FLY_PITCH),
  distance: clamp(cam.distance, MIN_DISTANCE, MAX_DISTANCE),
});

/** Drag pan on the ground plane: the ground under the pointer follows a drag of (dxPx right, dyPx down). */
export const panMap = (cam: Camera3D, dxPx: number, dyPx: number, viewportHeight: number): Camera3D => {
  const scale = blocksPerPixel(cam.distance, viewportHeight);
  // A vertical screen pixel covers more ground the flatter the view looks at it.
  const ahead = (dyPx * scale) / Math.sin(Math.max(cam.pitch, MIN_PITCH));
  const right = -dxPx * scale;
  return moveMap(cam, ahead, right);
};

export const orbit = (cam: Camera3D, dYaw: number, dPitch: number): Camera3D =>
  clampCamera({ ...cam, yaw: cam.yaw + dYaw, pitch: cam.pitch + dPitch });

/**
 * Scales the distance by `factor` (< 1 zooms in). With `toward` (the world point under the cursor) the eye moves
 * along its line to that point, so the point stays where it is on screen.
 */
export const zoomMap = (cam: Camera3D, factor: number, toward?: Vec3): Camera3D => {
  const distance = clamp(cam.distance * factor, MIN_DISTANCE, MAX_DISTANCE);
  if (!toward) return { ...cam, distance };
  const ratio = distance / cam.distance;
  return {
    ...cam,
    distance,
    x: toward[0] + (cam.x - toward[0]) * ratio,
    y: toward[1] + (cam.y - toward[1]) * ratio,
    z: toward[2] + (cam.z - toward[2]) * ratio,
  };
};

/** Moves the target along the ground: `forwardAmt` blocks the way the camera faces, `rightAmt` to its right. */
export const moveMap = (cam: Camera3D, forwardAmt: number, rightAmt: number): Camera3D => {
  const sin = Math.sin(cam.yaw);
  const cos = Math.cos(cam.yaw);
  return { ...cam, x: cam.x + sin * forwardAmt + cos * rightAmt, z: cam.z - cos * forwardAmt + sin * rightAmt };
};

/** Moves the fly eye along the view direction, strafes horizontally and rises along world Y. */
export const flyMove = (cam: Camera3D, forwardAmt: number, rightAmt: number, upAmt: number): Camera3D => {
  const [fx, fy, fz] = forward(cam);
  const sin = Math.sin(cam.yaw);
  const cos = Math.cos(cam.yaw);
  return {
    ...cam,
    x: cam.x + fx * forwardAmt + cos * rightAmt,
    y: cam.y + fy * forwardAmt + upAmt,
    z: cam.z + fz * forwardAmt + sin * rightAmt,
  };
};

export const flyLook = (cam: Camera3D, dYaw: number, dPitch: number): Camera3D =>
  clampCamera({ ...cam, yaw: cam.yaw + dYaw, pitch: cam.pitch + dPitch });

/** Fly camera at the map camera's eye, looking the same way. */
export const toFly = (cam: Camera3D): Camera3D => {
  if (cam.mode === 'fly') return cam;
  const [x, y, z] = eye(cam);
  return { ...cam, mode: 'fly', x, y, z, pitch: -cam.pitch };
};

/**
 * Map camera keeping the fly eye; it looks the same way when that is within the map pitch range, and its
 * target is where that view meets `groundY` (within the distance limits).
 */
export const toMap = (cam: Camera3D, groundY: number): Camera3D => {
  if (cam.mode === 'map') return cam;
  const pitch = clamp(-cam.pitch, MIN_PITCH, MAX_PITCH);
  const height = cam.y - groundY;
  const distance = clamp(height > 0 ? height / Math.sin(pitch) : cam.distance, MIN_DISTANCE, MAX_DISTANCE);
  const [fx, fy, fz] = direction(cam.yaw, -pitch);
  return {
    ...cam,
    mode: 'map',
    pitch,
    distance,
    x: cam.x + fx * distance,
    y: cam.y + fy * distance,
    z: cam.z + fz * distance,
  };
};

/** Camera heading, 0..360: 0 = north, 90 = east. */
export const compassDegrees = (cam: Camera3D): number => (((cam.yaw / DEG) % 360) + 360) % 360;

/**
 * Map camera centred on the 2D view's centre block, its distance chosen so a block at the target spans about
 * as many screen pixels as the 2D zoom (pixels per block).
 */
export const from2D = (
  view: { x: number; z: number; zoom: number },
  viewportHeight: number,
  groundY: number,
): Camera3D => ({
  mode: 'map',
  x: view.x,
  y: groundY,
  z: view.z,
  yaw: DEFAULT_YAW,
  pitch: DEFAULT_PITCH,
  distance: clamp(viewportHeight / (2 * Math.tan(FOV_Y / 2) * view.zoom), MIN_DISTANCE, MAX_DISTANCE),
});

/** The 2D view for the ground point at the screen centre (the fly eye's column in fly mode); inverse of from2D. */
export const to2D = (cam: Camera3D, viewportHeight: number): { x: number; z: number; zoom: number } => ({
  x: cam.x,
  z: cam.z,
  zoom: 1 / blocksPerPixel(cam.distance, viewportHeight),
});

/** Centres the map camera on a point, or puts the fly camera in front of it looking down at it. */
export const flyTo = (cam: Camera3D, x: number, y: number, z: number): Camera3D => {
  if (cam.mode === 'map') return { ...cam, x, y, z };
  const pitch = Math.min(cam.pitch, -FLY_TO_PITCH);
  const [fx, fy, fz] = direction(cam.yaw, pitch);
  return { ...cam, pitch, x: x - fx * FLY_TO_DISTANCE, y: y - fy * FLY_TO_DISTANCE, z: z - fz * FLY_TO_DISTANCE };
};

/** Look-at view matrix for the camera's eye relative to `origin`. */
export const viewMatrix = (cam: Camera3D, origin: Vec3): Float32Array => {
  const [fx, fy, fz] = forward(cam);
  const [ex, ey, ez] = eye(cam);
  const px = ex - origin[0];
  const py = ey - origin[1];
  const pz = ez - origin[2];
  // side = forward × up(0, 1, 0); |pitch| < 90° keeps it non-zero.
  const sideLength = Math.hypot(fz, fx);
  const sx = -fz / sideLength;
  const sz = fx / sideLength;
  // up = side × forward
  const ux = -sz * fy;
  const uy = sz * fx - sx * fz;
  const uz = sx * fy;
  const m = new Float32Array(16);
  m[0] = sx;
  m[4] = 0;
  m[8] = sz;
  m[12] = -(sx * px + sz * pz);
  m[1] = ux;
  m[5] = uy;
  m[9] = uz;
  m[13] = -(ux * px + uy * py + uz * pz);
  m[2] = -fx;
  m[6] = -fy;
  m[10] = -fz;
  m[14] = fx * px + fy * py + fz * pz;
  m[15] = 1;
  return m;
};

/** OpenGL perspective projection (clip depth -1..1) with the FOV_Y field of view. */
export const projectionMatrix = (aspect: number, near: number, far: number): Float32Array => {
  const f = 1 / Math.tan(FOV_Y / 2);
  const m = new Float32Array(16);
  m[0] = f / aspect;
  m[5] = f;
  m[10] = (far + near) / (near - far);
  m[11] = -1;
  m[14] = (2 * far * near) / (near - far);
  return m;
};

/** a × b (apply b first). */
export const multiply = (a: Float32Array, b: Float32Array): Float32Array => {
  const out = new Float32Array(16);
  for (let column = 0; column < 4; column++) {
    for (let row = 0; row < 4; row++) {
      let sum = 0;
      for (let k = 0; k < 4; k++) sum += a[k * 4 + row] * b[column * 4 + k];
      out[column * 4 + row] = sum;
    }
  }
  return out;
};

/** Inverse by cofactors (computed in float64), or null when singular. */
export const invert = (m: Float32Array): Float32Array | null => {
  const [a00, a01, a02, a03, a10, a11, a12, a13, a20, a21, a22, a23, a30, a31, a32, a33] = m;
  const b00 = a00 * a11 - a01 * a10;
  const b01 = a00 * a12 - a02 * a10;
  const b02 = a00 * a13 - a03 * a10;
  const b03 = a01 * a12 - a02 * a11;
  const b04 = a01 * a13 - a03 * a11;
  const b05 = a02 * a13 - a03 * a12;
  const b06 = a20 * a31 - a21 * a30;
  const b07 = a20 * a32 - a22 * a30;
  const b08 = a20 * a33 - a23 * a30;
  const b09 = a21 * a32 - a22 * a31;
  const b10 = a21 * a33 - a23 * a31;
  const b11 = a22 * a33 - a23 * a32;
  const det = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;
  if (det === 0 || !Number.isFinite(det)) return null;
  const inv = 1 / det;
  return new Float32Array([
    (a11 * b11 - a12 * b10 + a13 * b09) * inv,
    (a02 * b10 - a01 * b11 - a03 * b09) * inv,
    (a31 * b05 - a32 * b04 + a33 * b03) * inv,
    (a22 * b04 - a21 * b05 - a23 * b03) * inv,
    (a12 * b08 - a10 * b11 - a13 * b07) * inv,
    (a00 * b11 - a02 * b08 + a03 * b07) * inv,
    (a32 * b02 - a30 * b05 - a33 * b01) * inv,
    (a20 * b05 - a22 * b02 + a23 * b01) * inv,
    (a10 * b10 - a11 * b08 + a13 * b06) * inv,
    (a01 * b08 - a00 * b10 - a03 * b06) * inv,
    (a30 * b04 - a31 * b02 + a33 * b00) * inv,
    (a21 * b02 - a20 * b04 - a23 * b00) * inv,
    (a11 * b07 - a10 * b09 - a12 * b06) * inv,
    (a00 * b09 - a01 * b07 + a02 * b06) * inv,
    (a31 * b01 - a30 * b03 - a32 * b00) * inv,
    (a20 * b03 - a21 * b01 + a22 * b00) * inv,
  ]);
};

/** The six frustum planes (left, right, bottom, top, near, far) as normalised (a, b, c, d), inside ≥ 0. */
export const frustumPlanes = (viewProj: Float32Array): Float32Array => {
  const m = viewProj;
  const planes = new Float32Array(24);
  // Gribb/Hartmann: each plane is the matrix's last row plus or minus one of the others.
  for (let p = 0; p < 6; p++) {
    const row = p >> 1;
    const sign = p & 1 ? -1 : 1;
    const a = m[3] + sign * m[row];
    const b = m[7] + sign * m[4 + row];
    const c = m[11] + sign * m[8 + row];
    const d = m[15] + sign * m[12 + row];
    const length = Math.hypot(a, b, c);
    planes[p * 4] = a / length;
    planes[p * 4 + 1] = b / length;
    planes[p * 4 + 2] = c / length;
    planes[p * 4 + 3] = d / length;
  }
  return planes;
};

/** False only when the box is entirely outside one plane (conservative: may keep some invisible boxes). */
export const boxVisible = (planes: Float32Array, min: Vec3, max: Vec3): boolean => {
  for (let p = 0; p < 24; p += 4) {
    const a = planes[p];
    const b = planes[p + 1];
    const c = planes[p + 2];
    // The box corner furthest along the plane normal.
    const x = a >= 0 ? max[0] : min[0];
    const y = b >= 0 ? max[1] : min[1];
    const z = c >= 0 ? max[2] : min[2];
    if (a * x + b * y + c * z + planes[p + 3] < 0) return false;
  }
  return true;
};

/** Screen pixel position and depth 0..1 of a camera-relative point, or null behind the camera / out of depth. */
export const project = (viewProj: Float32Array, p: Vec3, width: number, height: number): Vec3 | null => {
  const m = viewProj;
  const [x, y, z] = p;
  const w = m[3] * x + m[7] * y + m[11] * z + m[15];
  if (w <= 1e-6) return null;
  const nx = (m[0] * x + m[4] * y + m[8] * z + m[12]) / w;
  const ny = (m[1] * x + m[5] * y + m[9] * z + m[13]) / w;
  const nz = (m[2] * x + m[6] * y + m[10] * z + m[14]) / w;
  if (nz < -1 || nz > 1) return null;
  return [((nx + 1) / 2) * width, ((1 - ny) / 2) * height, (nz + 1) / 2];
};

/** Camera-relative picking ray through a screen pixel: starts on the near plane, unit direction. */
export const screenRay = (
  invViewProj: Float32Array,
  sx: number,
  sy: number,
  width: number,
  height: number,
): { origin: Vec3; dir: Vec3 } => {
  const m = invViewProj;
  const nx = (2 * sx) / width - 1;
  const ny = 1 - (2 * sy) / height;
  const unproject = (nz: number): Vec3 => {
    const w = m[3] * nx + m[7] * ny + m[11] * nz + m[15];
    return [
      (m[0] * nx + m[4] * ny + m[8] * nz + m[12]) / w,
      (m[1] * nx + m[5] * ny + m[9] * nz + m[13]) / w,
      (m[2] * nx + m[6] * ny + m[10] * nz + m[14]) / w,
    ];
  };
  const near = unproject(-1);
  const far = unproject(1);
  const dx = far[0] - near[0];
  const dy = far[1] - near[1];
  const dz = far[2] - near[2];
  const length = Math.hypot(dx, dy, dz);
  return { origin: near, dir: [dx / length, dy / length, dz / length] };
};
