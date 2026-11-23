import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { LEGEND_GRADIENT, formatOverlayValue, overlayColour, parseOverlayBound } from '../frontend/src/lib/overlay.ts';

describe('overlay colours', () => {
  test('blue at min, red at max, MCA Selector hue gradient between', () => {
    assert.equal(overlayColour(0, 0, 100), 'rgba(0, 0, 255, 0.5)');
    assert.equal(overlayColour(-5, 0, 100), 'rgba(0, 0, 255, 0.5)');
    assert.equal(overlayColour(100, 0, 100), 'rgba(255, 0, 0, 0.5)');
    assert.equal(overlayColour(1e9, 0, 100), 'rgba(255, 0, 0, 0.5)');
    assert.equal(overlayColour(50, 0, 100), 'rgba(0, 255, 0, 0.5)');
    assert.equal(overlayColour(25, 0, 100), 'rgba(0, 255, 255, 0.5)');
    assert.equal(overlayColour(75, 0, 100), 'rgba(255, 255, 0, 0.5)');
    // hue 0.5833: between cyan and blue
    assert.equal(overlayColour(12.5, 0, 100), 'rgba(0, 128, 255, 0.5)');
  });

  test('legend stops follow the colours', () => {
    assert.equal(
      LEGEND_GRADIENT,
      'linear-gradient(to right, rgba(0, 0, 255, 0.5), rgba(0, 255, 255, 0.5), rgba(0, 255, 0, 0.5), rgba(255, 255, 0, 0.5), rgba(255, 0, 0, 0.5))',
    );
  });
});

describe('overlay bounds', () => {
  test('durations become ticks for InhabitedTime and LastUpdate', () => {
    assert.equal(parseOverlayBound('InhabitedTime', '1 minute'), 1200);
    assert.equal(parseOverlayBound('InhabitedTime', '600'), 600);
    assert.equal(parseOverlayBound('LastUpdate', '2 hours'), 144000);
    assert.equal(parseOverlayBound('LastUpdate', '-20'), -20);
    assert.equal(parseOverlayBound('InhabitedTime', '1000 years'), null);
    assert.equal(parseOverlayBound('InhabitedTime', 'soon'), null);
    assert.equal(parseOverlayBound('InhabitedTime', ''), null);
  });

  test('timestamps take unix seconds or local dates', () => {
    assert.equal(parseOverlayBound('Timestamp', '1700000000'), 1700000000);
    assert.equal(parseOverlayBound('Timestamp', '2024-01-02 03:04'), new Date(2024, 0, 2, 3, 4).getTime() / 1000);
    assert.equal(parseOverlayBound('Timestamp', 'yesterday'), null);
  });

  test('amount overlays need whole numbers of 0 or more', () => {
    assert.equal(parseOverlayBound('#Entities', '10'), 10);
    assert.equal(parseOverlayBound('#Blocks', '-1'), null);
    assert.equal(parseOverlayBound('DataVersion', '1.5'), null);
    assert.equal(parseOverlayBound('Size', '4096'), 4096);
    assert.equal(parseOverlayBound('AverageHeight', '-64'), -64);
    assert.equal(parseOverlayBound('#TileEntities', '2147483648'), null);
  });
});

describe('overlay values', () => {
  test('formats per overlay', () => {
    assert.equal(formatOverlayValue('InhabitedTime', 10), '10 ticks');
    assert.equal(formatOverlayValue('InhabitedTime', 1200), '1min');
    assert.equal(formatOverlayValue('LastUpdate', (2 * 86400 + 5 * 3600 + 7) * 20), '2d 5h');
    const ts = new Date(2024, 0, 2, 3, 4, 5).getTime() / 1000;
    assert.equal(formatOverlayValue('Timestamp', ts), '2024-01-02 03:04:05');
    assert.equal(formatOverlayValue('Size', 512), '512 B');
    assert.equal(formatOverlayValue('Size', 1536), '1.5 KiB');
    assert.equal(formatOverlayValue('AverageHeight', 63.456), '63.5');
    assert.equal(formatOverlayValue('#Entities', 3), '3');
  });

  test('formatted durations parse back', () => {
    for (const ticks of [1200, 72000, 20 * 31536000 + 20 * 86400]) {
      const text = formatOverlayValue('InhabitedTime', ticks);
      assert.equal(parseOverlayBound('InhabitedTime', text), ticks, text);
    }
  });
});
