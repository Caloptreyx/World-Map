import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { raycastColumns } from '../frontend/src/lib/raycast.ts';

const unit = (x: number, y: number, z: number): [number, number, number] => {
  const length = Math.hypot(x, y, z);
  return [x / length, y / length, z / length];
};

describe('column raycast', () => {
  test('hits the top face of flat ground', () => {
    const hit = raycastColumns([0.5, 50, 0.5], unit(1, -1, 0), () => 10, 500);
    // Y = 11 is reached after 39 blocks of descent, 39 blocks east.
    assert.deepEqual(
      { ...hit!, distance: Math.round(hit!.distance * 1e6) / 1e6 },
      {
        x: 39,
        y: 10,
        z: 0,
        face: 0,
        distance: Math.round(39 * Math.SQRT2 * 1e6) / 1e6,
      },
    );
  });

  test('a straight-down ray hits the column below', () => {
    const hit = raycastColumns([-3.5, 100, -7.25], [0, -1, 0], (x, z) => (x === -4 && z === -8 ? 64 : null), 500);
    assert.deepEqual(hit, { x: -4, y: 64, z: -8, face: 0, distance: 35 });
  });

  test('hits the side wall of a taller column with the face it entered', () => {
    const wall = (x: number) => (x === 5 ? 20 : 0);
    const east = raycastColumns([0.5, 5, 0.5], [1, 0, 0], (x) => wall(x), 100);
    assert.deepEqual(east, { x: 5, y: 20, z: 0, face: 4, distance: 4.5 });
    const west = raycastColumns([9.5, 5, 0.5], [-1, 0, 0], (x) => wall(x), 100);
    assert.deepEqual(west, { x: 5, y: 20, z: 0, face: 3, distance: 3.5 });
    const south = raycastColumns([0.5, 5, -3.5], [0, 0, 1], (_, z) => (z === 2 ? 20 : 0), 100);
    assert.deepEqual(south, { x: 0, y: 20, z: 2, face: 1, distance: 5.5 });
    const north = raycastColumns([0.5, 5, 9.5], [0, 0, -1], (_, z) => (z === 2 ? 20 : 0), 100);
    assert.deepEqual(north, { x: 0, y: 20, z: 2, face: 2, distance: 6.5 });
  });

  test('a ray passing over a column top is not stopped by it', () => {
    const hit = raycastColumns([0.5, 30, 0.5], unit(1, -0.5, 0), (x) => (x === 5 ? 20 : 0), 200);
    assert.ok(hit);
    assert.equal(hit.face, 0);
    assert.equal(hit.y, 0);
    assert.ok(hit.x > 5);
  });

  test('passes over missing columns and misses when nothing is there', () => {
    const top = (x: number) => (x < 20 ? null : 0);
    const hit = raycastColumns([0.5, 40, 0.5], unit(1, -1, 0), top, 500);
    assert.equal(hit?.x, 39);
    assert.equal(hit?.face, 0);
    assert.equal(
      raycastColumns([0.5, 40, 0.5], unit(1, -1, 0), () => null, 500),
      null,
    );
  });

  test('stops at maxDistance', () => {
    assert.equal(
      raycastColumns([0.5, 50, 0.5], unit(1, -1, 0), () => 10, 50),
      null,
    );
    assert.equal(
      raycastColumns([0.5, 50, 0.5], [0, 1, 0], () => 10, 50),
      null,
    );
    assert.ok(raycastColumns([0.5, 50, 0.5], unit(1, -1, 0), () => 10, 56));
  });

  test('an eye inside a column does not hit that column', () => {
    // The ray enters x = 3 at y = 45, below that column's top face (49).
    const top = (x: number) => (x === 0 ? 100 : x === 3 ? 48 : null);
    const hit = raycastColumns([0.5, 50, 0.5], unit(1, -2, 0), top, 500);
    assert.deepEqual(hit && { x: hit.x, face: hit.face }, { x: 3, face: 4 });
  });
});
