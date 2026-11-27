import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import {
  boxVisible,
  clampCamera,
  compassDegrees,
  DEFAULT_DISTANCE,
  DEFAULT_PITCH,
  DEFAULT_YAW,
  eye,
  FLY_PITCH,
  flyLook,
  flyMove,
  flyTo,
  forward,
  from2D,
  frustumPlanes,
  invert,
  MAX_DISTANCE,
  MAX_PITCH,
  MIN_DISTANCE,
  MIN_PITCH,
  moveMap,
  multiply,
  orbit,
  panMap,
  project,
  projectionMatrix,
  screenRay,
  to2D,
  toFly,
  toMap,
  viewMatrix,
  zoomMap,
  type Camera3D,
} from '../frontend/src/lib/camera3d.ts';

const WIDTH = 1200;
const HEIGHT = 800;

const mapCamera = (overrides: Partial<Camera3D> = {}): Camera3D => ({
  mode: 'map',
  x: 1000,
  y: 64,
  z: -2000,
  yaw: DEFAULT_YAW,
  pitch: DEFAULT_PITCH,
  distance: DEFAULT_DISTANCE,
  ...overrides,
});

const close = (actual: number, expected: number, tolerance = 1e-6, label = '') =>
  assert.ok(Math.abs(actual - expected) <= tolerance, `${label} ${actual} != ${expected} ± ${tolerance}`);

const closeVec = (actual: number[], expected: number[], tolerance = 1e-6) =>
  actual.forEach((value, i) => close(value, expected[i], tolerance, `[${i}]`));

/** View-projection relative to an origin near the camera, as the renderer builds it. */
const matrices = (cam: Camera3D) => {
  const origin: [number, number, number] = [Math.round(cam.x), Math.round(cam.y), Math.round(cam.z)];
  const viewProj = multiply(projectionMatrix(WIDTH / HEIGHT, 0.5, 4000), viewMatrix(cam, origin));
  return { origin, viewProj, inverse: invert(viewProj)! };
};

const screenOf = (cam: Camera3D, p: [number, number, number]) => {
  const { origin, viewProj } = matrices(cam);
  return project(viewProj, [p[0] - origin[0], p[1] - origin[1], p[2] - origin[2]], WIDTH, HEIGHT);
};

describe('map camera', () => {
  test('yaw 0 looks north and down, the eye sits behind and above the target', () => {
    const cam = mapCamera();
    const [fx, fy, fz] = forward(cam);
    close(fx, 0);
    assert.ok(fy < 0 && fz < 0);
    close(Math.hypot(fx, fy, fz), 1);
    const [ex, ey, ez] = eye(cam);
    close(Math.hypot(ex - cam.x, ey - cam.y, ez - cam.z), cam.distance, 1e-9);
    assert.ok(ey > cam.y && ez > cam.z);
    const east = forward(mapCamera({ yaw: Math.PI / 2 }));
    assert.ok(east[0] > 0);
    close(east[2], 0);
  });

  test('the target is at the screen centre', () => {
    const [sx, sy] = screenOf(mapCamera(), [1000, 64, -2000])!;
    close(sx, WIDTH / 2, 1e-3);
    close(sy, HEIGHT / 2, 1e-3);
  });

  test('drag pan moves the ground with the pointer', () => {
    const cam = mapCamera({ yaw: 0.7 });
    const panned = panMap(cam, 120, 0, HEIGHT);
    const [sx, sy] = screenOf(panned, [cam.x, cam.y, cam.z])!;
    close(sx, WIDTH / 2 + 120, 0.5);
    close(sy, HEIGHT / 2, 0.5);
    const down = panMap(cam, 0, 40, HEIGHT);
    const [, dy] = screenOf(down, [cam.x, cam.y, cam.z])!;
    close(dy, HEIGHT / 2 + 40, 4);
    // Facing north, dragging down moves the target north.
    const north = panMap(mapCamera(), 0, 50, HEIGHT);
    assert.ok(north.z < -2000);
    close(north.x, 1000);
    assert.equal(north.y, 64);
  });

  test('orbit turns and tilts with clamped pitch and wrapped yaw', () => {
    const cam = orbit(mapCamera(), -Math.PI / 2, 0.1);
    close(cam.yaw, (3 * Math.PI) / 2);
    close(cam.pitch, DEFAULT_PITCH + 0.1);
    assert.equal(orbit(cam, 0, 10).pitch, MAX_PITCH);
    assert.equal(orbit(cam, 0, -10).pitch, MIN_PITCH);
    assert.equal(cam.x, 1000);
    assert.equal(cam.distance, DEFAULT_DISTANCE);
  });

  test('zoom scales the distance within limits', () => {
    assert.equal(zoomMap(mapCamera(), 0.5).distance, DEFAULT_DISTANCE / 2);
    assert.equal(zoomMap(mapCamera(), 1e-6).distance, MIN_DISTANCE);
    assert.equal(zoomMap(mapCamera(), 1e6).distance, MAX_DISTANCE);
  });

  test('zoom toward a point keeps it in place on screen', () => {
    const cam = mapCamera({ yaw: 2 });
    const point: [number, number, number] = [cam.x + 40, 70, cam.z - 25];
    const before = screenOf(cam, point)!;
    for (const factor of [0.5, 1.7]) {
      const zoomed = zoomMap(cam, factor, point);
      const after = screenOf(zoomed, point)!;
      close(after[0], before[0], 0.05);
      close(after[1], before[1], 0.05);
      close(zoomed.distance, cam.distance * factor, 1e-9);
    }
  });

  test('WASD moves the target relative to the heading', () => {
    const east = moveMap(mapCamera({ yaw: Math.PI / 2 }), 10, 0);
    close(east.x, 1010);
    close(east.z, -2000);
    const strafe = moveMap(mapCamera(), 0, 5);
    close(strafe.x, 1005);
    close(strafe.z, -2000);
  });

  test('compass yaw', () => {
    close(compassDegrees(mapCamera()), 0);
    close(compassDegrees(mapCamera({ yaw: Math.PI / 2 })), 90);
    close(compassDegrees(mapCamera({ yaw: -Math.PI / 2 })), 270);
    close(compassDegrees(mapCamera({ yaw: 5 * Math.PI })), 180);
  });

  test('clampCamera keeps every field in range', () => {
    const cam = clampCamera(mapCamera({ pitch: 2, distance: 1e9, yaw: -0.5 }));
    assert.equal(cam.pitch, MAX_PITCH);
    assert.equal(cam.distance, MAX_DISTANCE);
    close(cam.yaw, Math.PI * 2 - 0.5);
    assert.equal(clampCamera({ ...cam, mode: 'fly', pitch: -3 }).pitch, -FLY_PITCH);
  });
});

describe('fly camera', () => {
  const flyCamera = (overrides: Partial<Camera3D> = {}): Camera3D => ({
    ...mapCamera(),
    mode: 'fly',
    pitch: 0,
    ...overrides,
  });

  test('moves along the view direction, strafes and rises', () => {
    const cam = flyCamera();
    closeVec(eye(cam), [1000, 64, -2000]);
    const ahead = flyMove(cam, 10, 0, 0);
    closeVec([ahead.x, ahead.y, ahead.z], [1000, 64, -2010]);
    const right = flyMove(cam, 0, 3, 2);
    closeVec([right.x, right.y, right.z], [1003, 66, -2000]);
    const diving = flyMove(flyCamera({ pitch: -Math.PI / 4, yaw: Math.PI / 2 }), Math.SQRT2, 0, 0);
    closeVec([diving.x, diving.y, diving.z], [1001, 63, -2000]);
  });

  test('looks around with clamped pitch', () => {
    const cam = flyLook(flyCamera(), 0.25, 0.5);
    close(cam.yaw, 0.25);
    close(cam.pitch, 0.5);
    assert.ok(forward(cam)[1] > 0);
    assert.equal(flyLook(cam, 0, 5).pitch, FLY_PITCH);
    assert.equal(flyLook(cam, 0, -5).pitch, -FLY_PITCH);
  });

  test('switching modes keeps the eye and the look direction', () => {
    const map = mapCamera({ yaw: 1.2, pitch: 0.6, distance: 150 });
    const fly = toFly(map);
    assert.equal(fly.mode, 'fly');
    closeVec(eye(fly), eye(map), 1e-9);
    closeVec(forward(fly), forward(map), 1e-12);
    const back = toMap(fly, 64);
    assert.equal(back.mode, 'map');
    closeVec(eye(back), eye(map), 1e-9);
    closeVec([back.x, back.y, back.z], [map.x, map.y, map.z], 1e-9);
    close(back.distance, 150, 1e-9);
  });

  test('toMap clamps a level fly view into the map pitch range, still from the same eye', () => {
    const fly = flyCamera({ pitch: 0.3, y: 200 });
    const map = toMap(fly, 64);
    assert.equal(map.pitch, MIN_PITCH);
    closeVec(eye(map), [fly.x, fly.y, fly.z], 1e-9);
    close(map.y, 64, 1e-9);
  });

  test('flyTo centres the map camera or flies in front of the point', () => {
    const map = flyTo(mapCamera(), 5, 70, 6);
    closeVec([map.x, map.y, map.z], [5, 70, 6]);
    const fly = flyTo(flyCamera(), 5, 70, 6);
    const [fx, fy, fz] = forward(fly);
    const toPoint = [5 - fly.x, 70 - fly.y, 6 - fly.z];
    const length = Math.hypot(...toPoint);
    closeVec(
      [fx, fy, fz],
      toPoint.map((v) => v / length),
      1e-9,
    );
    assert.ok(fly.y > 70);
  });
});

describe('2D and 3D sync', () => {
  test('from2D centres the view and matches its scale at the target', () => {
    const view = { x: -321.5, z: 4567, zoom: 2 };
    const cam = from2D(view, HEIGHT, 70);
    assert.equal(cam.mode, 'map');
    closeVec([cam.x, cam.y, cam.z], [-321.5, 70, 4567]);
    // A block east of the target spans about `zoom` pixels on screen.
    const a = screenOf(cam, [cam.x, 70, cam.z])!;
    const b = screenOf(cam, [cam.x + 1, 70, cam.z])!;
    close(b[0] - a[0], 2, 0.05);
  });

  test('2D → 3D → 2D round-trips', () => {
    for (const view of [
      { x: 0, z: 0, zoom: 1 },
      { x: -29_999_000.25, z: 12_345.5, zoom: 4 },
      { x: 800, z: -800, zoom: 0.75 },
    ]) {
      const back = to2D(from2D(view, HEIGHT, 64), HEIGHT);
      close(back.x, view.x, 1e-9);
      close(back.z, view.z, 1e-9);
      close(back.zoom, view.zoom, 1e-9);
    }
    // Panning and orbiting in 3D keep the centred point in sync.
    const moved = to2D(orbit(moveMap(from2D({ x: 0, z: 0, zoom: 1 }, HEIGHT, 64), 0, 30), 1, 0), HEIGHT);
    close(moved.x, 30);
    close(moved.z, 0);
  });

  test('zoom beyond the 3D limits is clamped', () => {
    assert.equal(from2D({ x: 0, z: 0, zoom: 1 / 16 }, HEIGHT, 64).distance, MAX_DISTANCE);
    assert.equal(from2D({ x: 0, z: 0, zoom: 1000 }, HEIGHT, 64).distance, MIN_DISTANCE);
  });
});

describe('matrices, projection and culling', () => {
  test('invert undoes the view-projection', () => {
    const { viewProj, inverse } = matrices(mapCamera({ yaw: 0.4 }));
    const identity = multiply(viewProj, inverse);
    for (let i = 0; i < 16; i++) close(identity[i], i % 5 === 0 ? 1 : 0, 1e-3);
    assert.equal(invert(new Float32Array(16)), null);
  });

  test('project and screenRay agree', () => {
    const cam = mapCamera({ yaw: 2.5, pitch: 0.7 });
    const { origin, viewProj, inverse } = matrices(cam);
    for (const world of [
      [cam.x, cam.y, cam.z],
      [cam.x + 30, cam.y + 5, cam.z - 12],
      [cam.x - 50, cam.y - 20, cam.z + 40],
    ]) {
      const p: [number, number, number] = [world[0] - origin[0], world[1] - origin[1], world[2] - origin[2]];
      const [sx, sy, depth] = project(viewProj, p, WIDTH, HEIGHT)!;
      assert.ok(depth > 0 && depth < 1);
      const ray = screenRay(inverse, sx, sy, WIDTH, HEIGHT);
      close(Math.hypot(...ray.dir), 1, 1e-9);
      // The point lies on the ray: its offset from the ray origin is parallel to the direction.
      const v = p.map((value, i) => value - ray.origin[i]);
      const along = v[0] * ray.dir[0] + v[1] * ray.dir[1] + v[2] * ray.dir[2];
      assert.ok(along > 0);
      const miss = Math.hypot(...v.map((value, i) => value - along * ray.dir[i]));
      close(miss, 0, 0.05);
    }
    // The centre ray points along the camera's forward vector and starts near the eye.
    const centre = screenRay(inverse, WIDTH / 2, HEIGHT / 2, WIDTH, HEIGHT);
    closeVec(centre.dir, forward(cam), 1e-4);
    const e = eye(cam);
    close(Math.hypot(...centre.origin.map((value, i) => value + origin[i] - e[i])), 0.5, 0.01);
  });

  test('project returns null behind the camera', () => {
    const cam = mapCamera();
    const e = eye(cam);
    const f = forward(cam);
    assert.equal(screenOf(cam, [e[0] - f[0] * 10, e[1] - f[1] * 10, e[2] - f[2] * 10]), null);
  });

  test('frustum keeps boxes in view and culls the rest', () => {
    const cam = mapCamera({ yaw: 1 });
    const { origin, viewProj } = matrices(cam);
    const planes = frustumPlanes(viewProj);
    for (let p = 0; p < 24; p += 4) close(Math.hypot(planes[p], planes[p + 1], planes[p + 2]), 1, 1e-5);
    const box = (cx: number, cy: number, cz: number, half: number) =>
      boxVisible(
        planes,
        [cx - half - origin[0], cy - half - origin[1], cz - half - origin[2]],
        [cx + half - origin[0], cy + half - origin[1], cz + half - origin[2]],
      );
    assert.ok(box(cam.x, cam.y, cam.z, 1));
    const e = eye(cam);
    const f = forward(cam);
    // Behind the eye, far off to the side, beyond the far plane.
    assert.ok(!box(e[0] - f[0] * 100, e[1] - f[1] * 100, e[2] - f[2] * 100, 10));
    const right = [Math.cos(cam.yaw), 0, Math.sin(cam.yaw)];
    assert.ok(!box(cam.x + right[0] * 2000, cam.y, cam.z + right[2] * 2000, 10));
    assert.ok(!box(e[0] + f[0] * 5000, e[1] + f[1] * 5000, e[2] + f[2] * 5000, 10));
    // A huge box straddling the frustum stays visible.
    assert.ok(box(cam.x + right[0] * 2000, cam.y, cam.z + right[2] * 2000, 2100));
  });
});
