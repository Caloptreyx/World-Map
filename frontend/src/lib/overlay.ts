/**
 * Overlay colours and bounds like MCA Selector (tile/OverlayPool.getColorGrade, overlay/overlays/*):
 * values at or below min get the min hue (blue), at or above max the max hue (red), in between a linear
 * hue gradient at full saturation and brightness, drawn at half opacity.
 */
import { parseDuration, parseTimestamp } from './filter.ts';

/** MCA Selector's default hues (Overlay.minHue / maxHue). */
const MIN_HUE = 2 / 3;
const MAX_HUE = 0;
/** MCA Selector draws overlay tiles with globalAlpha 0.5. */
const ALPHA = 0.5;
const INT_MIN = -2147483648;
const INT_MAX = 2147483647;

/** java.awt.Color.HSBtoRGB with saturation and brightness 1. */
const hueRgb = (hue: number): [number, number, number] => {
  const h = (hue - Math.floor(hue)) * 6;
  const f = h - Math.floor(h);
  const up = Math.trunc(f * 255 + 0.5);
  const down = Math.trunc((1 - f) * 255 + 0.5);
  switch (Math.floor(h)) {
    case 0:
      return [255, up, 0];
    case 1:
      return [down, 255, 0];
    case 2:
      return [0, 255, up];
    case 3:
      return [0, down, 255];
    case 4:
      return [up, 0, 255];
    default:
      return [255, 0, down];
  }
};

const rgba = ([r, g, b]: [number, number, number]): string => `rgba(${r}, ${g}, ${b}, ${ALPHA})`;

/** The overlay colour of a value between the bounds (MCA Selector's getColorGrade), as CSS rgba. */
export const overlayColour = (value: number, min: number, max: number): string => {
  if (value <= min) return rgba(hueRgb(MIN_HUE));
  if (value >= max) return rgba(hueRgb(MAX_HUE));
  const percent = (value - min) / (max - min);
  return rgba(hueRgb(MIN_HUE + percent * (MAX_HUE - MIN_HUE)));
};

/** CSS gradient from min (left) to max (right) matching overlayColour: the stops sit on the HSB sector
 * borders, between which the RGB channels change linearly. */
export const LEGEND_GRADIENT = `linear-gradient(to right, ${[0, 0.25, 0.5, 0.75, 1]
  .map((t) => rgba(hueRgb(MIN_HUE + t * (MAX_HUE - MIN_HUE))))
  .join(', ')})`;

const parseInt32 = (text: string): number | null => {
  if (!/^[+-]?\d+$/.test(text)) return null;
  const value = Number(text);
  return value >= INT_MIN && value <= INT_MAX ? value : null;
};

/**
 * Parses an overlay's min or max like the overlay's setMin/setMax: InhabitedTime and LastUpdate take ticks
 * or a duration ("5 minutes", converted to ticks), Timestamp takes unix seconds or "YYYY-MM-DD hh:mm:ss"
 * (local time), the counting overlays take whole numbers of 0 or more, AverageHeight any whole number.
 * Null for empty or invalid text.
 */
export const parseOverlayBound = (overlay: string, text: string): number | null => {
  const raw = text.trim();
  if (raw === '') return null;
  const number = parseInt32(raw);
  switch (overlay) {
    case 'InhabitedTime':
    case 'LastUpdate': {
      if (number !== null) return number;
      const seconds = parseDuration(raw);
      return seconds === null || seconds < 0 || seconds > Math.trunc(INT_MAX / 20) ? null : seconds * 20;
    }
    case 'Timestamp':
      return number ?? parseTimestamp(raw);
    case 'AverageHeight':
      return number;
    default:
      return number !== null && number >= 0 ? number : null;
  }
};

const DURATION_PARTS: [string, number][] = [
  ['y', 31536000],
  ['d', 86400],
  ['h', 3600],
  ['min', 60],
  ['s', 1],
];

/** Seconds as the two most significant units, e.g. "2d 5h" (parseDuration reads it back). */
const formatDuration = (seconds: number): string => {
  if (seconds < 0) return `-${formatDuration(-seconds)}`;
  const parts: string[] = [];
  let rest = Math.floor(seconds);
  for (const [unit, size] of DURATION_PARTS) {
    if (rest >= size && parts.length < 2) {
      parts.push(`${Math.floor(rest / size)}${unit}`);
      rest %= size;
    } else if (parts.length > 0) break;
  }
  return parts.length > 0 ? parts.join(' ') : '0s';
};

const pad = (n: number): string => String(n).padStart(2, '0');

/** A value for legends and tooltips: durations for tick overlays, local dates for Timestamp, byte sizes
 * for Size, one decimal for AverageHeight, whole numbers otherwise. */
export const formatOverlayValue = (overlay: string, value: number): string => {
  switch (overlay) {
    case 'InhabitedTime':
    case 'LastUpdate':
      return value < 20 && value > -20 ? `${Math.round(value)} ticks` : formatDuration(value / 20);
    case 'Timestamp': {
      const d = new Date(value * 1000);
      return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
    }
    case 'Size':
      if (value < 1024) return `${Math.round(value)} B`;
      if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB`;
      return `${(value / 1024 / 1024).toFixed(1)} MiB`;
    case 'AverageHeight':
      return (Math.round(value * 10) / 10).toString();
    default:
      return Math.round(value).toString();
  }
};
