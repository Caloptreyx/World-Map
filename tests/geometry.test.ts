import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import {
  chunkOf,
  chunkRect,
  cluster,
  heatColour,
  MAX_ZOOM,
  MIN_ZOOM,
  regionOf,
  selectedChunks,
  selectionText,
  toScreen,
  toWorld,
  visibleBounds,
  visibleRegions,
  zoomAt,
} from '../frontend/src/lib/geometry.ts';

const size = { width: 800, height: 600 };

describe('coordinates', () => {
  test('chunks and regions floor towards negative infinity', () => {
    assert.equal(chunkOf(15.9), 0);
    assert.equal(chunkOf(16), 1);
    assert.equal(chunkOf(-0.1), -1);
    assert.equal(chunkOf(-16), -1);
    assert.equal(chunkOf(-17), -2);
    assert.equal(regionOf(511), 0);
    assert.equal(regionOf(512), 1);
    assert.equal(regionOf(-1), -1);
    assert.equal(regionOf(-513), -2);
  });

  test('screen and world round-trip', () => {
    const view = { x: -300, z: 1200.5, zoom: 2.5 };
    const [sx, sy] = toScreen(view, size, -250, 1100);
    const [x, z] = toWorld(view, size, sx, sy);
    assert.ok(Math.abs(x + 250) < 1e-9 && Math.abs(z - 1100) < 1e-9);
    assert.deepEqual(toScreen(view, size, view.x, view.z), [400, 300]);
  });

  test('visible regions cover the screen, nearest first', () => {
    const view = { x: 0, z: 0, zoom: 1 };
    const regions = visibleRegions(view, size);
    // 800 x 600 blocks around the origin touch regions -1..0 in both directions
    assert.equal(regions.length, 4);
    assert.deepEqual(new Set(regions.map(([x, z]) => `${x},${z}`)), new Set(['-1,-1', '0,-1', '-1,0', '0,0']));
    const far = visibleRegions({ x: 256, z: 256, zoom: 1 }, size);
    assert.deepEqual(far[0], [0, 0]);
    const bounds = visibleBounds(view, size);
    assert.deepEqual(bounds, { minX: -400, minZ: -300, maxX: 400, maxZ: 300 });
  });

  test('zooming keeps the block under the cursor in place and stays in range', () => {
    const view = { x: 100, z: 100, zoom: 1 };
    const zoomed = zoomAt(view, size, 600, 150, 2);
    assert.equal(zoomed.zoom, 2);
    const before = toWorld(view, size, 600, 150);
    const after = toWorld(zoomed, size, 600, 150);
    assert.ok(Math.abs(before[0] - after[0]) < 1e-9 && Math.abs(before[1] - after[1]) < 1e-9);
    assert.equal(zoomAt(view, size, 0, 0, 1000).zoom, MAX_ZOOM);
    assert.equal(zoomAt(view, size, 0, 0, 0.0001).zoom, MIN_ZOOM);
  });
});

describe('chunk selection', () => {
  test('rectangles span the chunks under both corners', () => {
    assert.deepEqual(chunkRect(31, -1, 0, 20), { x1: 0, z1: -1, x2: 1, z2: 1 });
    assert.deepEqual(chunkRect(-17, -17, -17, -17), { x1: -2, z1: -2, x2: -2, z2: -2 });
  });

  test('overlapping rectangles count each chunk once', () => {
    const rects = [
      { x1: 0, z1: 0, x2: 3, z2: 3 },
      { x1: 2, z1: 2, x2: 5, z2: 5 },
    ];
    assert.equal(selectedChunks(rects), 16 + 16 - 4);
    assert.equal(selectedChunks(rects, 10), 32);
    assert.equal(selectedChunks([]), 0);
  });

  test('copies block coordinates of every rectangle', () => {
    assert.equal(
      selectionText([
        { x1: -1, z1: 0, x2: 0, z2: 1 },
        { x1: 2, z1: 2, x2: 2, z2: 2 },
      ]),
      '-16 0 -> 15 31\n32 32 -> 47 47',
    );
  });
});

describe('layers', () => {
  test('heat colours grow from transparent to opaque', () => {
    assert.equal(heatColour(0, 10), 'rgba(0, 0, 0, 0)');
    assert.equal(heatColour(5, 0), 'rgba(0, 0, 0, 0)');
    assert.equal(heatColour(10, 10), 'rgba(240, 50, 40, 0.75)');
    const alpha = (colour: string) => Number(colour.slice(colour.lastIndexOf(',') + 1, -1));
    assert.ok(alpha(heatColour(1, 100)) < alpha(heatColour(50, 100)));
  });

  test('nearby points cluster, distant ones do not, off-screen ones are dropped', () => {
    const view = { x: 0, z: 0, zoom: 1 };
    const groups = cluster(
      [
        { x: 1, z: 1 },
        { x: 2, z: 3 },
        { x: 200, z: 0 },
        { x: 5000, z: 0 },
      ],
      view,
      size,
      28,
    );
    assert.equal(groups.length, 2);
    const sizes = groups.map((group) => group.members.length).sort();
    assert.deepEqual(sizes, [1, 2]);
  });
});
