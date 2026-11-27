// WebGL2 terrain engine of the 3D view: downloads region surfaces near the camera, meshes them into stepped
// voxel columns block by block (time-sliced, LOD by distance) and draws them on demand. Everything the GPU sees
// is relative to a per-frame origin near the eye so float32 stays exact far from 0,0.

import {
  boxVisible,
  type Camera3D,
  eye,
  frustumPlanes,
  invert,
  MAX_DISTANCE,
  multiply,
  project,
  projectionMatrix,
  screenRay,
  viewMatrix,
} from './camera3d.ts';
import { createProgram, createTexture, deleteMesh, type GpuMesh, uniformLocations, uploadMesh } from './gl.ts';
import {
  buildBlockMesh,
  landTop,
  MESH_BLOCK,
  type MeshData,
  type Sampler,
  type Step,
  stepForDistance,
} from './mesh.ts';
import { type Hit, raycastColumns } from './raycast.ts';
import { decodeSurface, MISSING, SURFACE_SIZE, type SurfaceData } from './surface.ts';

// Tunables.
/** Region downloads at the same time (the backend extracts a few regions at once anyway). */
const MAX_PARALLEL_FETCHES = 4;
/** Decoded regions kept in memory (~1.8 MB each); the nearest ones on screen are never evicted. */
const MAX_REGIONS = 40;
/** Vertices kept on the GPU (12 bytes each plus indices) before meshes not drawn recently are evicted. */
const VERTEX_BUDGET = 6_000_000;
/** Milliseconds of mesh building per animation frame, so loading never stutters the camera. */
const MESH_BUDGET_MS = 6;
/** Highest device pixel ratio rendered (sharper costs fill rate for little visible gain). */
const MAX_DPR = 2;
/** View range in map mode: this many blocks plus a share of the camera distance, at most MAX_VIEW_RANGE. */
const VIEW_RANGE_BASE = 512;
const VIEW_RANGE_PER_DISTANCE = 1.5;
const MAX_VIEW_RANGE = 2560;
/** View range in fly mode. */
const FLY_VIEW_RANGE = 1280;
/** Regions this close to the eye load even outside the frustum, so turning around shows no holes. */
const PREFETCH_RADIUS = 384;
/** A block keeps its LOD while the distance is within this fraction of a step boundary (no rebuild flicker). */
const LOD_HYSTERESIS = 0.15;
/** Boundary skirts drop this far below the region's lowest land. */
const SKIRT_DROP = 4;
/** Y range assumed for regions not loaded yet (frustum test only). */
const WORLD_MIN_Y = -64;
const WORLD_MAX_Y = 320;
/** Water is drawn this far below its block top so shorelines read as a water line, not a flush plane. */
const WATER_LIFT = -0.1;
/** Highlight box: grown a little beyond the block so it is not hidden by the faces it surrounds. */
const HIGHLIGHT_GROW = 0.02;
const HIGHLIGHT_FILL: [number, number, number, number] = [1, 1, 1, 0.22];
const HIGHLIGHT_LINE: [number, number, number, number] = [0.05, 0.05, 0.05, 0.85];
/** Per-region tint texture: one RGBA texel per chunk. */
const TINT_SIZE = 32;
const TINT_BYTES = TINT_SIZE * TINT_SIZE * 4;

const BLOCKS_PER_REGION = SURFACE_SIZE / MESH_BLOCK;

export type Atmosphere = 'overworld' | 'nether' | 'end';

interface AtmosphereLook {
  /** Clear colour and fog colour (the same, so distant terrain melts into the backdrop). */
  sky: [number, number, number];
  sunColour: [number, number, number];
  /** Share of the light that does not depend on the face direction. */
  ambient: number;
  /** Fog starts at this fraction of the view range (denser in the Nether). */
  fogStart: number;
}

const rgb = (hex: number): [number, number, number] => [(hex >> 16) / 255, ((hex >> 8) & 255) / 255, (hex & 255) / 255];

const ATMOSPHERES: Record<Atmosphere, AtmosphereLook> = {
  overworld: { sky: rgb(0x9ec5e8), sunColour: [1, 0.98, 0.93], ambient: 0.45, fogStart: 0.45 },
  nether: { sky: rgb(0x2a0f0f), sunColour: [1, 0.84, 0.74], ambient: 0.6, fogStart: 0.2 },
  end: { sky: rgb(0x0b0912), sunColour: [0.88, 0.85, 1], ambient: 0.55, fogStart: 0.4 },
};

/** Light from the south-east and above: top faces brightest, then south, east, west, north all distinct. */
const SUN_DIRECTION = ((x: number, y: number, z: number): [number, number, number] => {
  const length = Math.hypot(x, y, z);
  return [x / length, y / length, z / length];
})(0.35, 0.9, 0.55);

const TERRAIN_VERTEX = `#version 300 es
layout(location = 0) in vec3 aPos;
layout(location = 1) in vec3 aColour;
layout(location = 2) in uint aFace;
layout(location = 3) in float aShade;
uniform mat4 uViewProj;
uniform vec3 uOffset;
uniform vec2 uRegionLocal;
uniform float uLift;
out vec3 vRel;
out vec3 vLocal;
out vec3 vColour;
out float vShade;
out vec2 vTintUv;
flat out uint vFace;
void main() {
  vLocal = aPos;
  vRel = aPos + uOffset + vec3(0.0, uLift, 0.0);
  vColour = aColour;
  vShade = aShade;
  vFace = aFace;
  vTintUv = (uRegionLocal + aPos.xz) / ${SURFACE_SIZE.toFixed(1)};
  gl_Position = uViewProj * vec4(vRel, 1.0);
}`;

const TERRAIN_FRAGMENT = `#version 300 es
precision highp float;
uniform vec3 uEye;
uniform vec3 uSunDir;
uniform vec3 uSunColour;
uniform float uAmbient;
uniform vec3 uFogColour;
uniform vec2 uFog;
uniform float uStep;
uniform int uWater;
uniform sampler2D uTint;
in vec3 vRel;
in vec3 vLocal;
in vec3 vColour;
in float vShade;
in vec2 vTintUv;
flat in uint vFace;
out vec4 fragColour;

const vec3 NORMALS[5] = vec3[5](
  vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, -1.0), vec3(0.0, 0.0, 1.0), vec3(1.0, 0.0, 0.0), vec3(-1.0, 0.0, 0.0));

// 1 on the block grid lines, fading out where lines would be thinner than a pixel and alias.
float gridLine(vec2 p) {
  vec2 width = max(fwidth(p), vec2(1e-4));
  vec2 pixels = abs(fract(p - 0.5) - 0.5) / width;
  float line = 1.0 - min(min(pixels.x, pixels.y), 1.0);
  return line * (1.0 - smoothstep(0.15, 0.5, max(width.x, width.y)));
}

void main() {
  uint face = min(vFace, 4u);
  vec4 tint = texture(uTint, vTintUv);
  vec3 base = mix(vColour, tint.rgb, tint.a);
  // Half-Lambert keeps faces turned away from the sun readable and every side a different shade.
  float sun = dot(NORMALS[face], uSunDir) * 0.5 + 0.5;
  vec3 light = uSunColour * (uAmbient + (1.0 - uAmbient) * sun);
  vec3 colour;
  float alpha = 1.0;
  if (uWater == 1) {
    float deep = clamp(vShade * 255.0 / 16.0, 0.0, 1.0);
    colour = base * light * mix(1.0, 0.55, deep);
    alpha = mix(0.5, 0.88, deep);
  } else {
    vec2 grid = face == 0u ? vLocal.xz / uStep
      : face <= 2u ? vec2(vLocal.x / uStep, vLocal.y) : vec2(vLocal.z / uStep, vLocal.y);
    float ao = mix(0.5, 1.0, vShade);
    colour = base * light * ao * (1.0 - 0.14 * gridLine(grid));
  }
  float fog = smoothstep(uFog.x, uFog.y, length(vRel - uEye));
  fragColour = vec4(mix(colour, uFogColour, fog), mix(alpha, 1.0, fog));
}`;

const HIGHLIGHT_VERTEX = `#version 300 es
layout(location = 0) in vec3 aPos;
uniform mat4 uViewProj;
uniform vec3 uMin;
uniform vec3 uSize;
void main() {
  gl_Position = uViewProj * vec4(uMin + aPos * uSize, 1.0);
}`;

const HIGHLIGHT_FRAGMENT = `#version 300 es
precision mediump float;
uniform vec4 uColour;
out vec4 fragColour;
void main() {
  fragColour = uColour;
}`;

const TERRAIN_UNIFORMS = [
  'uViewProj',
  'uOffset',
  'uRegionLocal',
  'uLift',
  'uEye',
  'uSunDir',
  'uSunColour',
  'uAmbient',
  'uFogColour',
  'uFog',
  'uStep',
  'uWater',
  'uTint',
] as const;
const HIGHLIGHT_UNIFORMS = ['uViewProj', 'uMin', 'uSize', 'uColour'] as const;

/** Unit cube corners (bit 0 = x, bit 1 = y, bit 2 = z), its 12 triangles and then its 12 edges. */
const CUBE_TRIANGLES = 36;
const CUBE_LINES = 24;
const cubeGeometry = (): { positions: Float32Array; indices: Uint8Array } => {
  const positions = new Float32Array(24);
  for (let i = 0; i < 8; i++) positions.set([i & 1, (i >> 1) & 1, (i >> 2) & 1], i * 3);
  const triangles: number[] = [];
  for (const [axis, u, v] of [
    [1, 2, 4],
    [2, 4, 1],
    [4, 1, 2],
  ]) {
    for (const side of [0, axis]) triangles.push(side, side | u, side | u | v, side, side | u | v, side | v);
  }
  const lines: number[] = [];
  for (let i = 0; i < 8; i++) {
    for (const bit of [1, 2, 4]) if (!(i & bit)) lines.push(i, i | bit);
  }
  return { positions, indices: Uint8Array.from([...triangles, ...lines]) };
};

export interface TerrainStatus {
  /** Regions on screen still downloading/decoding. */
  loading: number;
  /** Mesh blocks queued. */
  meshing: number;
  /** Regions in memory. */
  regions: number;
  /** The region/memory cap or MAX_DISTANCE limits what is shown. */
  capped: boolean;
  /** Last region error (human text from the fetcher's thrown error). */
  error: string | null;
}

export interface TerrainOptions {
  /** Downloads the zlib body of one region; reject to mark it failed. Regions not in setPlace are never asked. */
  fetchSurface: (rx: number, rz: number, signal: AbortSignal) => Promise<ArrayBuffer>;
  onStatus: (status: TerrainStatus) => void;
  onContextLost: () => void;
  /** Called after each drawn frame (markers re-project then). */
  onFrame?: () => void;
}

type Vec3 = [number, number, number];

interface Region {
  rx: number;
  rz: number;
  key: string;
  /** `modified-size` of the data held (or of the attempt that failed); refetched when the file changes. */
  version: string;
  surface: SurfaceData | null;
  /** True when the region has no column with data: nothing to mesh. */
  empty: boolean;
  /** Skirt floor and the land/water Y range of the whole region (frustum box before blocks are meshed). */
  base: number;
  maxY: number;
  request: AbortController | null;
  error: string | null;
  lastUsed: number;
}

interface MeshBlock {
  key: string;
  regionKey: string;
  step: Step;
  land: GpuMesh | null;
  water: GpuMesh | null;
  minY: number;
  maxY: number;
  vertices: number;
  lastDrawn: number;
  /** Its data or a neighbour's changed: rebuild when next visible (the old mesh is drawn until then). */
  dirty: boolean;
}

/** A visible block this frame (it may not have a mesh yet). */
interface Slot {
  key: string;
  region: Region;
  originX: number;
  originZ: number;
  localX: number;
  localZ: number;
  distance: number;
}

interface QueuedBlock {
  slot: Slot;
  step: Step;
}

interface FrameView {
  cam: Camera3D;
  eye: Vec3;
  origin: Vec3;
  range: number;
  viewProj: Float32Array;
  inverse: Float32Array | null;
  planes: Float32Array;
}

interface GlState {
  terrain: WebGLProgram;
  terrainUniforms: Record<(typeof TERRAIN_UNIFORMS)[number], WebGLUniformLocation | null>;
  highlight: WebGLProgram;
  highlightUniforms: Record<(typeof HIGHLIGHT_UNIFORMS)[number], WebGLUniformLocation | null>;
  cubeVao: WebGLVertexArrayObject | null;
  cubeBuffers: WebGLBuffer[];
  /** 1×1 transparent tint for regions without one. */
  noTint: WebGLTexture | null;
  tints: Map<string, WebGLTexture>;
}

const regionKey = (rx: number, rz: number): string => `${rx},${rz}`;
/** Key of mesh block (bx, bz) of region (rx, rz), in global block-grid coordinates. */
const blockKey = (rx: number, rz: number, bx: number, bz: number): string =>
  `${rx * BLOCKS_PER_REGION + bx},${rz * BLOCKS_PER_REGION + bz}`;

/** Horizontal distance from a point to a square of `size` blocks at (x0, z0). */
const squareDistance = (px: number, pz: number, x0: number, z0: number, size: number): number =>
  Math.hypot(Math.max(x0 - px, 0, px - (x0 + size)), Math.max(z0 - pz, 0, pz - (z0 + size)));

/** Distance from a point to a box. */
const boxDistance = (p: Vec3, min: Vec3, max: Vec3): number =>
  Math.hypot(
    Math.max(min[0] - p[0], 0, p[0] - max[0]),
    Math.max(min[1] - p[1], 0, p[1] - max[1]),
    Math.max(min[2] - p[2], 0, p[2] - max[2]),
  );

/** A block keeps its step while the distance is within LOD_HYSTERESIS of where that step applies. */
const keepsStep = (step: Step, distance: number): boolean =>
  stepForDistance(distance * (1 - LOD_HYSTERESIS)) <= step && step <= stepForDistance(distance * (1 + LOD_HYSTERESIS));

const sameStatus = (a: TerrainStatus | null, b: TerrainStatus): boolean =>
  a !== null &&
  a.loading === b.loading &&
  a.meshing === b.meshing &&
  a.regions === b.regions &&
  a.capped === b.capped &&
  a.error === b.error;

let webgl2Support: boolean | null = null;

export class Terrain3D {
  /** WebGL2 available (probed once). */
  static supported(): boolean {
    if (webgl2Support === null) {
      try {
        const probe = document.createElement('canvas').getContext('webgl2');
        webgl2Support = probe !== null;
        // Browsers cap live contexts; release the probe instead of waiting for garbage collection.
        probe?.getExtension('WEBGL_lose_context')?.loseContext();
      } catch {
        webgl2Support = false;
      }
    }
    return webgl2Support;
  }

  private readonly canvas: HTMLCanvasElement;
  private readonly gl: WebGL2RenderingContext;
  private readonly options: TerrainOptions;
  private glState: GlState | null = null;
  private lost = false;
  private disposed = false;
  private raf = 0;
  /** Frame counter: LRU clock for regions and meshes. */
  private tick = 0;

  private placeKey: string | null = null;
  private atmosphere: Atmosphere = 'overworld';
  /** Region files of the current place: key → `modified-size`. */
  private files = new Map<string, string>();
  private readonly regions = new Map<string, Region>();
  private inFlight = 0;
  private readonly blocks = new Map<string, MeshBlock>();
  private vertexTotal = 0;

  private cam: Camera3D | null = null;
  private view: FrameView | null = null;
  private width: number;
  private height: number;
  private highlight: { x: number; y: number; z: number } | null = null;
  private tints: Map<string, Uint8Array> | null = null;
  private lastStatus: TerrainStatus | null = null;

  /** Heights of the columns just outside a region, for the mesher's border walls. */
  private readonly sampler: Sampler = {
    height: (x, z, step) => {
      const rx = Math.floor(x / SURFACE_SIZE);
      const rz = Math.floor(z / SURFACE_SIZE);
      const surface = this.regions.get(regionKey(rx, rz))?.surface;
      return surface ? landTop(surface, x - rx * SURFACE_SIZE, z - rz * SURFACE_SIZE, step) : MISSING;
    },
  };

  constructor(canvas: HTMLCanvasElement, options: TerrainOptions) {
    const gl = canvas.getContext('webgl2', { antialias: true, alpha: false });
    if (!gl) throw new Error('WebGL2 is not available');
    this.canvas = canvas;
    this.gl = gl;
    this.options = options;
    this.width = Math.max(1, canvas.clientWidth || canvas.width);
    this.height = Math.max(1, canvas.clientHeight || canvas.height);
    this.glState = this.createGlState();
    canvas.addEventListener('webglcontextlost', this.onContextLost);
    canvas.addEventListener('webglcontextrestored', this.onContextRestored);
    this.schedule();
  }

  /** New world/dimension (key changes) drops every region/mesh/buffer; same key just updates the file list.
   *  A changed modified/size refetches that region (its old data stays on screen until the new one arrives). */
  setPlace(
    key: string,
    regions: { x: number; z: number; modified: number; size: number }[],
    atmosphere: Atmosphere,
  ): void {
    if (this.disposed) return;
    if (key !== this.placeKey) {
      // deleting the visited entry while iterating a Map is well defined
      for (const region of this.regions.values()) this.removeRegion(region);
      this.placeKey = key;
    }
    this.atmosphere = atmosphere;
    this.files = new Map(regions.map((r) => [regionKey(r.x, r.z), `${r.modified}-${r.size}`]));
    for (const region of this.regions.values()) {
      const version = this.files.get(region.key);
      if (version === undefined) {
        this.removeRegion(region);
      } else if (version !== region.version) {
        // A request for an older version is no longer useful; the next frame asks for the new one.
        this.cancelRequest(region);
        region.error = null;
        if (!region.surface) this.regions.delete(region.key);
      }
    }
    this.schedule();
  }

  setCamera(cam: Camera3D): void {
    this.cam = cam;
    this.updateView();
    this.schedule();
  }

  resize(width: number, height: number, dpr: number): void {
    this.width = Math.max(1, width);
    this.height = Math.max(1, height);
    const scale = Math.min(Math.max(dpr, 1), MAX_DPR);
    this.canvas.width = Math.max(1, Math.round(this.width * scale));
    this.canvas.height = Math.max(1, Math.round(this.height * scale));
    this.updateView();
    this.schedule();
  }

  /** Top block y of the loaded column (water surface for wet ones), or null. */
  heightAt(x: number, z: number): number | null {
    const bx = Math.floor(x);
    const bz = Math.floor(z);
    const rx = Math.floor(bx / SURFACE_SIZE);
    const rz = Math.floor(bz / SURFACE_SIZE);
    const surface = this.regions.get(regionKey(rx, rz))?.surface;
    if (!surface) return null;
    const height = surface.heights[(bz - rz * SURFACE_SIZE) * SURFACE_SIZE + (bx - rx * SURFACE_SIZE)];
    return height === MISSING ? null : height;
  }

  /** Raycast from a CSS-pixel screen point against loaded columns (water surfaces count as hits). */
  pick(sx: number, sy: number): Hit | null {
    const view = this.view;
    if (!view?.inverse) return null;
    const ray = screenRay(view.inverse, sx, sy, this.width, this.height);
    const [ox, oy, oz] = view.origin;
    const origin: Vec3 = [ray.origin[0] + ox, ray.origin[1] + oy, ray.origin[2] + oz];
    return raycastColumns(origin, ray.dir, (x, z) => this.heightAt(x, z), view.range);
  }

  /** CSS-pixel screen position of a world point, or null when behind the camera. */
  project(x: number, y: number, z: number): [number, number] | null {
    const view = this.view;
    if (!view) return null;
    const [ox, oy, oz] = view.origin;
    const screen = project(view.viewProj, [x - ox, y - oy, z - oz], this.width, this.height);
    return screen ? [screen[0], screen[1]] : null;
  }

  setHighlight(column: { x: number; y: number; z: number } | null): void {
    this.highlight = column;
    this.schedule();
  }

  /** Per-chunk tint per region key `${rx},${rz}`: 32*32 RGBA bytes (index cx + cz*32), alpha = strength. */
  setTints(tints: Map<string, Uint8Array> | null): void {
    this.tints = tints;
    if (this.glState && !this.lost) this.uploadTints(this.glState);
    this.schedule();
  }

  /** Frees every GL object, aborts fetches and removes listeners. The instance is unusable afterwards. */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    if (this.raf) cancelAnimationFrame(this.raf);
    this.raf = 0;
    this.canvas.removeEventListener('webglcontextlost', this.onContextLost);
    this.canvas.removeEventListener('webglcontextrestored', this.onContextRestored);
    for (const region of this.regions.values()) this.cancelRequest(region);
    if (!this.lost) {
      for (const block of this.blocks.values()) this.freeBlockMeshes(block);
      if (this.glState) this.deleteGlState(this.glState);
    }
    this.glState = null;
    this.blocks.clear();
    this.vertexTotal = 0;
    this.regions.clear();
    this.files.clear();
    this.tints = null;
    this.cam = null;
    this.view = null;
    this.highlight = null;
  }

  // GL resources

  private createGlState(): GlState {
    const gl = this.gl;
    const terrain = createProgram(gl, TERRAIN_VERTEX, TERRAIN_FRAGMENT);
    let highlight: WebGLProgram;
    try {
      highlight = createProgram(gl, HIGHLIGHT_VERTEX, HIGHLIGHT_FRAGMENT);
    } catch (err) {
      gl.deleteProgram(terrain);
      throw err;
    }
    const cube = cubeGeometry();
    const cubeVao = gl.createVertexArray();
    const positions = gl.createBuffer();
    const indices = gl.createBuffer();
    gl.bindVertexArray(cubeVao);
    gl.bindBuffer(gl.ARRAY_BUFFER, positions);
    gl.bufferData(gl.ARRAY_BUFFER, cube.positions, gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 0, 0);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, indices);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, cube.indices, gl.STATIC_DRAW);
    gl.bindVertexArray(null);
    gl.bindBuffer(gl.ARRAY_BUFFER, null);
    const state: GlState = {
      terrain,
      terrainUniforms: uniformLocations(gl, terrain, TERRAIN_UNIFORMS),
      highlight,
      highlightUniforms: uniformLocations(gl, highlight, HIGHLIGHT_UNIFORMS),
      cubeVao,
      cubeBuffers: [positions, indices].filter((buffer): buffer is WebGLBuffer => buffer !== null),
      noTint: createTexture(gl, 1, 1, new Uint8Array(4)),
      tints: new Map(),
    };
    this.uploadTints(state);
    return state;
  }

  private deleteGlState(state: GlState): void {
    const gl = this.gl;
    gl.deleteProgram(state.terrain);
    gl.deleteProgram(state.highlight);
    if (state.cubeVao) gl.deleteVertexArray(state.cubeVao);
    for (const buffer of state.cubeBuffers) gl.deleteBuffer(buffer);
    if (state.noTint) gl.deleteTexture(state.noTint);
    for (const texture of state.tints.values()) gl.deleteTexture(texture);
    state.tints.clear();
  }

  private uploadTints(state: GlState): void {
    const gl = this.gl;
    for (const texture of state.tints.values()) gl.deleteTexture(texture);
    state.tints.clear();
    if (!this.tints) return;
    for (const [key, pixels] of this.tints) {
      if (pixels.length < TINT_BYTES) continue;
      const texture = createTexture(gl, TINT_SIZE, TINT_SIZE, pixels.subarray(0, TINT_BYTES));
      if (texture) state.tints.set(key, texture);
    }
  }

  private readonly onContextLost = (event: Event): void => {
    // Without preventDefault the browser never offers to restore the context.
    event.preventDefault();
    this.lost = true;
    if (this.raf) cancelAnimationFrame(this.raf);
    this.raf = 0;
    // The GL objects died with the context; decoded regions stay so a restore only has to remesh.
    this.glState = null;
    this.blocks.clear();
    this.vertexTotal = 0;
    this.options.onContextLost();
  };

  private readonly onContextRestored = (): void => {
    if (this.disposed) return;
    try {
      this.glState = this.createGlState();
    } catch {
      this.options.onContextLost();
      return;
    }
    this.lost = false;
    this.schedule();
  };

  // Regions

  private cancelRequest(region: Region): void {
    if (!region.request) return;
    region.request.abort();
    region.request = null;
    this.inFlight--;
  }

  /** Drops a region with its meshes; its neighbours' borders turn back into skirts. */
  private removeRegion(region: Region): void {
    this.cancelRequest(region);
    this.regions.delete(region.key);
    for (let bz = 0; bz < BLOCKS_PER_REGION; bz++) {
      for (let bx = 0; bx < BLOCKS_PER_REGION; bx++) {
        const block = this.blocks.get(blockKey(region.rx, region.rz, bx, bz));
        if (block) this.deleteBlock(block);
      }
    }
    if (region.surface) this.invalidateNeighbours(region.rx, region.rz);
  }

  private startFetch(rx: number, rz: number, key: string, version: string, existing: Region | undefined): void {
    const region: Region = existing ?? {
      rx,
      rz,
      key,
      version: '',
      surface: null,
      empty: true,
      base: WORLD_MIN_Y,
      maxY: WORLD_MAX_Y,
      request: null,
      error: null,
      lastUsed: this.tick,
    };
    this.regions.set(key, region);
    const controller = new AbortController();
    region.request = controller;
    this.inFlight++;
    const current = () => !this.disposed && this.regions.get(key) === region && region.request === controller;
    this.options
      .fetchSurface(rx, rz, controller.signal)
      .then((body) => decodeSurface(body))
      .then(
        (surface) => {
          if (!current()) return;
          region.request = null;
          this.inFlight--;
          region.version = version;
          region.error = null;
          this.setSurface(region, surface);
          this.schedule();
        },
        (err: unknown) => {
          if (!current()) return;
          region.request = null;
          this.inFlight--;
          // Recording the version keeps a failing region from being asked again until its file changes.
          region.version = version;
          region.error = err instanceof Error ? err.message : String(err);
          this.schedule();
        },
      );
  }

  private setSurface(region: Region, surface: SurfaceData): void {
    let minLand = Infinity;
    let maxTop = -Infinity;
    const { heights, depths } = surface;
    for (let i = 0; i < heights.length; i++) {
      const height = heights[i];
      if (height === MISSING) continue;
      if (height > maxTop) maxTop = height;
      const land = height - depths[i];
      if (land < minLand) minLand = land;
    }
    region.surface = surface;
    region.empty = maxTop === -Infinity;
    region.base = region.empty ? 0 : minLand - SKIRT_DROP;
    region.maxY = region.empty ? 0 : maxTop + 1;
    this.invalidateBlocks(region.rx, region.rz, 0, BLOCKS_PER_REGION - 1, 0, BLOCKS_PER_REGION - 1);
    this.invalidateNeighbours(region.rx, region.rz);
  }

  /** Marks the border blocks of the four neighbours facing (rx, rz) for a rebuild, so seams stitch. */
  private invalidateNeighbours(rx: number, rz: number): void {
    const last = BLOCKS_PER_REGION - 1;
    this.invalidateBlocks(rx - 1, rz, last, last, 0, last);
    this.invalidateBlocks(rx + 1, rz, 0, 0, 0, last);
    this.invalidateBlocks(rx, rz - 1, 0, last, last, last);
    this.invalidateBlocks(rx, rz + 1, 0, last, 0, 0);
  }

  private invalidateBlocks(rx: number, rz: number, bx0: number, bx1: number, bz0: number, bz1: number): void {
    for (let bz = bz0; bz <= bz1; bz++) {
      for (let bx = bx0; bx <= bx1; bx++) {
        const block = this.blocks.get(blockKey(rx, rz, bx, bz));
        if (block) block.dirty = true;
      }
    }
  }

  /** Picks the regions to show (nearest first, inside the view range and frustum), starts downloads and evicts
   *  the least recently used regions beyond the cap. */
  private updateRegions(view: FrameView): { ready: Region[]; loading: number; capped: boolean; error: string | null } {
    const [ex, , ez] = view.eye;
    const [ox, oy, oz] = view.origin;
    const range = view.range;
    const candidates: { rx: number; rz: number; key: string; version: string; distance: number }[] = [];
    const minRx = Math.floor((ex - range) / SURFACE_SIZE);
    const maxRx = Math.floor((ex + range) / SURFACE_SIZE);
    const minRz = Math.floor((ez - range) / SURFACE_SIZE);
    const maxRz = Math.floor((ez + range) / SURFACE_SIZE);
    for (let rz = minRz; rz <= maxRz; rz++) {
      for (let rx = minRx; rx <= maxRx; rx++) {
        const key = regionKey(rx, rz);
        const version = this.files.get(key);
        if (version === undefined) continue;
        const x0 = rx * SURFACE_SIZE;
        const z0 = rz * SURFACE_SIZE;
        const distance = squareDistance(ex, ez, x0, z0, SURFACE_SIZE);
        if (distance > range) continue;
        if (distance > PREFETCH_RADIUS) {
          const region = this.regions.get(key);
          const known = region?.surface && !region.empty ? region : null;
          const minY = known ? known.base : WORLD_MIN_Y;
          const maxY = known ? known.maxY : WORLD_MAX_Y;
          const min: Vec3 = [x0 - ox, minY - oy, z0 - oz];
          const max: Vec3 = [x0 + SURFACE_SIZE - ox, maxY - oy, z0 + SURFACE_SIZE - oz];
          if (!boxVisible(view.planes, min, max)) continue;
        }
        candidates.push({ rx, rz, key, version, distance });
      }
    }
    candidates.sort((a, b) => a.distance - b.distance);
    const wanted = candidates.slice(0, MAX_REGIONS);
    const wantedKeys = new Set(wanted.map((c) => c.key));

    // Downloads that left the view only block the slots for the ones that matter now.
    for (const region of this.regions.values()) {
      if (!region.request || wantedKeys.has(region.key)) continue;
      this.cancelRequest(region);
      if (!region.surface) this.regions.delete(region.key);
    }

    const ready: Region[] = [];
    let loading = 0;
    let error: string | null = null;
    for (const c of wanted) {
      const region = this.regions.get(c.key);
      if (region) {
        region.lastUsed = this.tick;
        if (region.surface && !region.empty) ready.push(region);
        if (region.error !== null) error ??= region.error;
        if (region.request) {
          loading++;
          continue;
        }
        if (region.version === c.version) continue;
      }
      loading++;
      if (this.inFlight < MAX_PARALLEL_FETCHES) this.startFetch(c.rx, c.rz, c.key, c.version, region);
    }

    if (this.regions.size > MAX_REGIONS) {
      const idle = [...this.regions.values()]
        .filter((region) => !wantedKeys.has(region.key) && !region.request)
        .sort((a, b) => a.lastUsed - b.lastUsed);
      for (const region of idle) {
        if (this.regions.size <= MAX_REGIONS) break;
        this.removeRegion(region);
      }
    }
    return { ready, loading, capped: candidates.length > MAX_REGIONS, error };
  }

  // Meshes

  /** Visible blocks of the shown regions, and the ones that need a (re)build at their LOD, nearest first. */
  private collectBlocks(view: FrameView, regions: Region[]): { slots: Slot[]; queue: QueuedBlock[] } {
    const [ox, oy, oz] = view.origin;
    const slots: Slot[] = [];
    const queue: QueuedBlock[] = [];
    for (const region of regions) {
      for (let bz = 0; bz < BLOCKS_PER_REGION; bz++) {
        for (let bx = 0; bx < BLOCKS_PER_REGION; bx++) {
          const key = blockKey(region.rx, region.rz, bx, bz);
          const block = this.blocks.get(key);
          if (block && !block.land && !block.water && !block.dirty) continue;
          const localX = bx * MESH_BLOCK;
          const localZ = bz * MESH_BLOCK;
          const originX = region.rx * SURFACE_SIZE + localX;
          const originZ = region.rz * SURFACE_SIZE + localZ;
          if (squareDistance(view.eye[0], view.eye[2], originX, originZ, MESH_BLOCK) > view.range) continue;
          const minY = block ? block.minY : region.base;
          const maxY = block ? block.maxY : region.maxY;
          const min: Vec3 = [originX, minY, originZ];
          const max: Vec3 = [originX + MESH_BLOCK, maxY, originZ + MESH_BLOCK];
          const relMin: Vec3 = [min[0] - ox, min[1] - oy, min[2] - oz];
          const relMax: Vec3 = [max[0] - ox, max[1] - oy, max[2] - oz];
          if (!boxVisible(view.planes, relMin, relMax)) continue;
          const distance = boxDistance(view.eye, min, max);
          const slot: Slot = { key, region, originX, originZ, localX, localZ, distance };
          slots.push(slot);
          if (block) block.lastDrawn = this.tick;
          if (!block || block.dirty || !keepsStep(block.step, distance)) {
            queue.push({ slot, step: stepForDistance(distance) });
          }
        }
      }
    }
    slots.sort((a, b) => a.distance - b.distance);
    queue.sort((a, b) => a.slot.distance - b.slot.distance);
    return { slots, queue };
  }

  /** Builds queued blocks until the frame's time budget is spent; returns how many are left. */
  private buildQueued(queue: QueuedBlock[]): number {
    const deadline = performance.now() + MESH_BUDGET_MS;
    let built = 0;
    while (built < queue.length) {
      this.buildBlock(queue[built]);
      built++;
      if (performance.now() >= deadline) break;
    }
    return queue.length - built;
  }

  private buildBlock({ slot, step }: QueuedBlock): void {
    const { region } = slot;
    if (!region.surface) return;
    const mesh = buildBlockMesh({
      surface: region.surface,
      originX: slot.originX,
      originZ: slot.originZ,
      localX: slot.localX,
      localZ: slot.localZ,
      step,
      neighbours: this.sampler,
      base: region.base,
    });
    const old = this.blocks.get(slot.key);
    if (old) this.deleteBlock(old);
    const land = uploadMesh(this.gl, mesh.land);
    const water = uploadMesh(this.gl, mesh.water);
    const parts = [mesh.land, mesh.water].filter((part: MeshData) => part.indexCount > 0);
    const block: MeshBlock = {
      key: slot.key,
      regionKey: region.key,
      step,
      land,
      water,
      minY: parts.length ? Math.min(...parts.map((part) => part.minY)) : region.base,
      maxY: parts.length ? Math.max(...parts.map((part) => part.maxY)) : region.maxY,
      vertices: (land?.vertexCount ?? 0) + (water?.vertexCount ?? 0),
      lastDrawn: this.tick,
      dirty: false,
    };
    this.blocks.set(slot.key, block);
    this.vertexTotal += block.vertices;
  }

  private freeBlockMeshes(block: MeshBlock): void {
    if (block.land) deleteMesh(this.gl, block.land);
    if (block.water) deleteMesh(this.gl, block.water);
    block.land = null;
    block.water = null;
  }

  private deleteBlock(block: MeshBlock): void {
    this.freeBlockMeshes(block);
    this.vertexTotal -= block.vertices;
    this.blocks.delete(block.key);
  }

  /** Evicts meshes not drawn this frame, least recently drawn first; true when the visible ones alone exceed
   *  the budget. */
  private enforceBudget(): boolean {
    if (this.vertexTotal <= VERTEX_BUDGET) return false;
    const idle = [...this.blocks.values()]
      .filter((block) => block.lastDrawn !== this.tick && block.vertices > 0)
      .sort((a, b) => a.lastDrawn - b.lastDrawn);
    for (const block of idle) {
      if (this.vertexTotal <= VERTEX_BUDGET) break;
      this.deleteBlock(block);
    }
    return this.vertexTotal > VERTEX_BUDGET;
  }

  // Frames

  private updateView(): void {
    const cam = this.cam;
    if (!cam) {
      this.view = null;
      return;
    }
    const eyePos = eye(cam);
    const origin: Vec3 = [Math.floor(eyePos[0]), Math.floor(eyePos[1]), Math.floor(eyePos[2])];
    const range =
      cam.mode === 'map'
        ? Math.min(MAX_VIEW_RANGE, VIEW_RANGE_BASE + cam.distance * VIEW_RANGE_PER_DISTANCE)
        : FLY_VIEW_RANGE;
    // A near plane that grows with the orbit distance keeps depth precision where the terrain is.
    const near = cam.mode === 'map' ? Math.min(4, Math.max(0.1, cam.distance * 0.01)) : 0.1;
    const far = range * 1.1 + 64;
    const viewProj = multiply(projectionMatrix(this.width / this.height, near, far), viewMatrix(cam, origin));
    const inverse = invert(viewProj);
    this.view = { cam, eye: eyePos, origin, range, viewProj, inverse, planes: frustumPlanes(viewProj) };
  }

  private schedule(): void {
    if (this.raf || this.lost || this.disposed) return;
    this.raf = requestAnimationFrame(this.frame);
  }

  private readonly frame = (): void => {
    this.raf = 0;
    const state = this.glState;
    if (this.disposed || this.lost || !state) return;
    this.tick++;
    const gl = this.gl;
    const look = ATMOSPHERES[this.atmosphere];
    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.clearColor(look.sky[0], look.sky[1], look.sky[2], 1);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);

    const view = this.view;
    let remaining = 0;
    if (view) {
      const regions = this.updateRegions(view);
      const { slots, queue } = this.collectBlocks(view, regions.ready);
      remaining = this.buildQueued(queue);
      const overBudget = this.enforceBudget();
      this.draw(state, view, look, slots);
      const atLimit = view.cam.mode === 'map' && view.cam.distance >= MAX_DISTANCE - 1e-6;
      this.emitStatus({
        loading: regions.loading,
        meshing: remaining,
        regions: this.countLoaded(),
        capped: atLimit || regions.capped || overBudget,
        error: regions.error,
      });
    } else {
      this.emitStatus({ loading: 0, meshing: 0, regions: this.countLoaded(), capped: false, error: null });
    }
    this.options.onFrame?.();
    // Keep going only while there is mesh work; downloads schedule a frame when they land.
    if (remaining > 0) this.schedule();
  };

  private countLoaded(): number {
    let count = 0;
    for (const region of this.regions.values()) if (region.surface) count++;
    return count;
  }

  private emitStatus(status: TerrainStatus): void {
    if (sameStatus(this.lastStatus, status)) return;
    this.lastStatus = status;
    this.options.onStatus(status);
  }

  private draw(state: GlState, view: FrameView, look: AtmosphereLook, slots: Slot[]): void {
    const gl = this.gl;
    const u = state.terrainUniforms;
    const [ox, oy, oz] = view.origin;
    gl.enable(gl.DEPTH_TEST);
    gl.depthFunc(gl.LEQUAL);
    gl.enable(gl.CULL_FACE);
    gl.cullFace(gl.BACK);
    gl.disable(gl.BLEND);
    gl.depthMask(true);

    gl.useProgram(state.terrain);
    gl.uniformMatrix4fv(u.uViewProj, false, view.viewProj);
    gl.uniform3f(u.uEye, view.eye[0] - ox, view.eye[1] - oy, view.eye[2] - oz);
    gl.uniform3fv(u.uSunDir, SUN_DIRECTION);
    gl.uniform3fv(u.uSunColour, look.sunColour);
    gl.uniform1f(u.uAmbient, look.ambient);
    gl.uniform3fv(u.uFogColour, look.sky);
    gl.uniform2f(u.uFog, view.range * look.fogStart, view.range * 0.97);
    gl.uniform1i(u.uTint, 0);
    gl.activeTexture(gl.TEXTURE0);

    let boundTint: WebGLTexture | null | undefined;
    const drawPart = (slot: Slot, block: MeshBlock, mesh: GpuMesh) => {
      const tint = state.tints.get(block.regionKey) ?? state.noTint;
      if (tint !== boundTint) {
        gl.bindTexture(gl.TEXTURE_2D, tint);
        boundTint = tint;
      }
      gl.uniform3f(u.uOffset, slot.originX - ox, -oy, slot.originZ - oz);
      gl.uniform2f(u.uRegionLocal, slot.localX, slot.localZ);
      gl.uniform1f(u.uStep, block.step);
      gl.bindVertexArray(mesh.vao);
      gl.drawElements(gl.TRIANGLES, mesh.indexCount, gl.UNSIGNED_INT, 0);
    };

    // Land front to back (cheap early depth rejection), then translucent water back to front.
    gl.uniform1i(u.uWater, 0);
    gl.uniform1f(u.uLift, 0);
    for (const slot of slots) {
      const block = this.blocks.get(slot.key);
      if (block?.land) drawPart(slot, block, block.land);
    }
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    gl.depthMask(false);
    gl.uniform1i(u.uWater, 1);
    gl.uniform1f(u.uLift, WATER_LIFT);
    for (let i = slots.length - 1; i >= 0; i--) {
      const block = this.blocks.get(slots[i].key);
      if (block?.water) drawPart(slots[i], block, block.water);
    }

    if (this.highlight && state.cubeVao) {
      const h = state.highlightUniforms;
      const { x, y, z } = this.highlight;
      gl.disable(gl.CULL_FACE);
      gl.useProgram(state.highlight);
      gl.uniformMatrix4fv(h.uViewProj, false, view.viewProj);
      gl.uniform3f(h.uMin, x - HIGHLIGHT_GROW - ox, y - HIGHLIGHT_GROW - oy, z - HIGHLIGHT_GROW - oz);
      const size = 1 + 2 * HIGHLIGHT_GROW;
      gl.uniform3f(h.uSize, size, size, size);
      gl.bindVertexArray(state.cubeVao);
      gl.uniform4fv(h.uColour, HIGHLIGHT_FILL);
      gl.drawElements(gl.TRIANGLES, CUBE_TRIANGLES, gl.UNSIGNED_BYTE, 0);
      gl.uniform4fv(h.uColour, HIGHLIGHT_LINE);
      gl.drawElements(gl.LINES, CUBE_LINES, gl.UNSIGNED_BYTE, CUBE_TRIANGLES);
    }

    gl.bindVertexArray(null);
    gl.depthMask(true);
    gl.disable(gl.BLEND);
  }
}
