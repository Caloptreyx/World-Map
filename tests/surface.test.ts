import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { deflateSync } from 'node:zlib';
import {
  decodeSurface,
  encodeSurfaceBody,
  MISSING,
  parseSurface,
  SURFACE_SIZE,
  type SurfaceData,
} from '../frontend/src/lib/surface.ts';

const sample = (size: number): SurfaceData => {
  const cells = size * size;
  const heights = new Int16Array(cells);
  const colours = new Uint16Array(cells);
  const depths = new Uint8Array(cells);
  const water = new Uint16Array(cells);
  for (let i = 0; i < cells; i++) {
    heights[i] = ((i * 37) % 400) - 64;
    colours[i] = 1 + (i % 3);
    if (i % 5 === 0) {
      depths[i] = i % 256;
      water[i] = 4;
    }
  }
  // Extremes next to each other exercise the wrapping delta code.
  heights[0] = MISSING;
  heights[1] = 32767;
  heights[2] = MISSING;
  heights[3] = -1;
  heights[size] = -64;
  heights[size + 1] = MISSING;
  colours[0] = 0;
  colours[2] = 0;
  colours[size + 1] = 0;
  const palette = new Uint8Array([0, 0, 0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 1, 2, 3]);
  return { size, ceiling: true, palette, heights, colours, depths, water };
};

describe('surface payload', () => {
  test('encode and parse round-trip, including MISSING and negative heights', () => {
    const surface = sample(8);
    const body = encodeSurfaceBody(surface);
    assert.equal(body.length, 10 + 15 + 64 * 7);
    assert.deepEqual(parseSurface(body), surface);
  });

  test('the flags byte carries the ceiling scan', () => {
    const surface = { ...sample(4), ceiling: false };
    assert.equal(parseSurface(encodeSurfaceBody(surface)).ceiling, false);
  });

  test('decodes a full zlib-compressed region', async () => {
    const surface = sample(SURFACE_SIZE);
    const compressed = deflateSync(encodeSurfaceBody(surface));
    const decoded = await decodeSurface(compressed);
    assert.equal(decoded.size, SURFACE_SIZE);
    assert.deepEqual(decoded, surface);
    const { buffer, byteOffset, length } = compressed;
    const fromBuffer = await decodeSurface(buffer.slice(byteOffset, byteOffset + length));
    assert.deepEqual(fromBuffer.heights, surface.heights);
  });

  test('parses a body that does not start at offset 0 of its buffer', () => {
    const body = encodeSurfaceBody(sample(4));
    const shifted = new Uint8Array(body.length + 3);
    shifted.set(body, 3);
    assert.deepEqual(parseSurface(shifted.subarray(3)), sample(4));
  });

  test('rejects bad magic, unknown versions and wrong lengths', () => {
    const body = encodeSurfaceBody(sample(4));
    const badMagic = body.slice();
    badMagic[0] = 0x58;
    assert.throws(() => parseSurface(badMagic), /Not a World-Map surface/);
    assert.throws(() => parseSurface(new Uint8Array(3)), /Not a World-Map surface/);
    const badVersion = body.slice();
    badVersion[4] = 2;
    assert.throws(() => parseSurface(badVersion), /version 2/);
    assert.throws(() => parseSurface(body.subarray(0, body.length - 1)), /expected/);
  });

  test('rejects corrupt compressed data', async () => {
    await assert.rejects(decodeSurface(new Uint8Array([1, 2, 3, 4, 5])));
  });
});
