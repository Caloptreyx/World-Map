// The 3D surface payload of one region: per column the top block height, colours and water depth, as the
// backend's /surface endpoint sends it (a zlib stream of the little-endian body below).

export const SURFACE_SIZE = 512;
/** Height of columns without data (missing, unfinished or unreadable chunks, nothing visible). */
export const MISSING = -32768;

const MAGIC = [0x57, 0x4d, 0x33, 0x44]; // "WM3D"
const VERSION = 1;
const HEADER_BYTES = 10;
/** Bytes per column after the palette: i16 height, u16 colour, u8 depth, u16 water colour. */
const COLUMN_BYTES = 7;
const FLAG_CEILING = 1;

export interface SurfaceData {
  size: number;
  /** Scanned below a ceiling (the Nether roof). */
  ceiling: boolean;
  /** RGB triples; entry 0 is unused so a colour index of 0 means none. */
  palette: Uint8Array;
  /** Top block y per column (the water surface block for wet columns), MISSING where empty. Row-major. */
  heights: Int16Array;
  /** Palette index of the dry surface block, or of the floor under water. */
  colours: Uint16Array;
  /** Blocks of water above the floor (0 = dry), saturated at 255. */
  depths: Uint8Array;
  /** Palette index of the water colour; 0 on dry columns. */
  water: Uint16Array;
}

/** Decodes an uncompressed body; throws when it is not a surface payload this version understands. */
export const parseSurface = (bytes: Uint8Array): SurfaceData => {
  if (bytes.length < HEADER_BYTES || MAGIC.some((byte, i) => bytes[i] !== byte)) {
    throw new Error('Not a World-Map surface payload');
  }
  if (bytes[4] !== VERSION) {
    throw new Error(`Unsupported surface format version ${bytes[4]}`);
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const size = view.getUint16(6, true);
  const paletteLength = view.getUint16(8, true);
  const cells = size * size;
  const expected = HEADER_BYTES + paletteLength * 3 + cells * COLUMN_BYTES;
  if (size === 0 || paletteLength === 0 || bytes.length !== expected) {
    throw new Error(`Surface payload has ${bytes.length} bytes, expected ${expected}`);
  }

  let offset = HEADER_BYTES;
  const palette = bytes.slice(offset, offset + paletteLength * 3);
  offset += paletteLength * 3;

  // Heights are delta-coded along each row; wrapping i16 arithmetic makes the MISSING sentinel exact.
  const heights = new Int16Array(cells);
  for (let row = 0; row < cells; row += size) {
    let previous = 0;
    for (let i = row; i < row + size; i++) {
      previous = ((view.getInt16(offset, true) + previous) << 16) >> 16;
      heights[i] = previous;
      offset += 2;
    }
  }
  const colours = new Uint16Array(cells);
  for (let i = 0; i < cells; i++, offset += 2) colours[i] = view.getUint16(offset, true);
  const depths = bytes.slice(offset, offset + cells);
  offset += cells;
  const water = new Uint16Array(cells);
  for (let i = 0; i < cells; i++, offset += 2) water[i] = view.getUint16(offset, true);

  return { size, ceiling: (bytes[5] & FLAG_CEILING) !== 0, palette, heights, colours, depths, water };
};

/** Inflates a zlib (RFC 1950) stream, which is what the browser calls 'deflate'. */
export const inflate = async (compressed: ArrayBuffer | Uint8Array): Promise<Uint8Array> => {
  const stream = new Blob([compressed as BlobPart]).stream().pipeThrough(new DecompressionStream('deflate'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
};

export const decodeSurface = async (compressed: ArrayBuffer | Uint8Array): Promise<SurfaceData> =>
  parseSurface(await inflate(compressed));

/** Test helper and reference encoder (mirrors the Rust encoder, uncompressed body). */
export const encodeSurfaceBody = (s: Omit<SurfaceData, 'size'> & { size?: number }): Uint8Array => {
  const size = s.size ?? SURFACE_SIZE;
  const cells = size * size;
  const paletteLength = s.palette.length / 3;
  const bytes = new Uint8Array(HEADER_BYTES + s.palette.length + cells * COLUMN_BYTES);
  const view = new DataView(bytes.buffer);
  bytes.set(MAGIC, 0);
  bytes[4] = VERSION;
  bytes[5] = s.ceiling ? FLAG_CEILING : 0;
  view.setUint16(6, size, true);
  view.setUint16(8, paletteLength, true);
  let offset = HEADER_BYTES;
  bytes.set(s.palette, offset);
  offset += s.palette.length;
  for (let row = 0; row < cells; row += size) {
    let previous = 0;
    for (let i = row; i < row + size; i++, offset += 2) {
      view.setInt16(offset, ((s.heights[i] - previous) << 16) >> 16, true);
      previous = s.heights[i];
    }
  }
  for (let i = 0; i < cells; i++, offset += 2) view.setUint16(offset, s.colours[i], true);
  bytes.set(s.depths.subarray(0, cells), offset);
  offset += cells;
  for (let i = 0; i < cells; i++, offset += 2) view.setUint16(offset, s.water[i], true);
  return bytes;
};
