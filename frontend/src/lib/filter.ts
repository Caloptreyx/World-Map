/**
 * Structural mirror of MCA Selector's query language (net.querz.mcaselector.filter.FilterParser) for instant
 * feedback and the query builder; the backend (`POST /filter/check`) stays authoritative. Values that MCA
 * Selector checks against its block / biome / entity / structure registries are only checked for their
 * syntax here.
 */

export type Comparator = '=' | '!=' | '<' | '>' | '<=' | '>=' | 'contains' | '!contains' | 'intersects' | '!intersects';
export type Operator = 'AND' | 'OR';
export type FilterNode =
  | { kind: 'group'; operator: Operator; negated: boolean; children: FilterNode[] }
  | { kind: 'filter'; operator: Operator; type: string; comparator: Comparator; value: string };

export interface FilterType {
  name: string;
  format: 'number' | 'text';
  comparators: Comparator[];
  hint: string;
  example: string;
}

const NUMBER: Comparator[] = ['=', '!=', '>', '<', '>=', '<='];
const EQUALITY: Comparator[] = ['=', '!='];
const TEXT: Comparator[] = ['contains', '!contains', 'intersects'];
const COMPARATORS: Comparator[] = [
  '=',
  '!=',
  '<',
  '>',
  '>=',
  '<=',
  'contains',
  '!contains',
  'intersects',
  '!intersects',
];

/** MCA Selector's chunk status names (mapping/registry/status.json). */
export const STATUSES = [
  'base',
  'biomes',
  'carved',
  'carvers',
  'decorated',
  'empty',
  'features',
  'finalized',
  'full',
  'fullchunk',
  'heightmaps',
  'initialize_light',
  'light',
  'lighted',
  'liquid_carved',
  'liquid_carvers',
  'mobs_spawned',
  'noise',
  'postprocessed',
  'spawn',
  'structure_references',
  'structure_starts',
  'surface',
];

/** MCA Selector's CompressionType names with their region-file bytes. */
const COMPRESSIONS: [string, number][] = [
  ['NONE', 0],
  ['GZIP', 1],
  ['ZLIB', 2],
  ['UNCOMPRESSED', 3],
  ['LZ4', 4],
  ['NONE_EXT', -128],
  ['GZIP_EXT', -127],
  ['ZLIB_EXT', -126],
  ['UNCOMPRESSED_EXT', -125],
  ['LZ4_EXT', -124],
];

/** The queryable MCA Selector filters World Map evaluates, plus World Map's `Size`. */
export const FILTER_TYPES: FilterType[] = [
  {
    name: 'DataVersion',
    format: 'number',
    comparators: NUMBER,
    hint: 'Chunk data version (0 or more)',
    example: 'DataVersion >= 3953',
  },
  {
    name: 'InhabitedTime',
    format: 'number',
    comparators: NUMBER,
    hint: 'Ticks players spent in the chunk, or a duration like "5 minutes", "1 hour 30 mins"',
    example: 'InhabitedTime < "5 minutes"',
  },
  { name: 'xPos', format: 'number', comparators: NUMBER, hint: 'Chunk x coordinate', example: 'xPos >= -10' },
  { name: 'yPos', format: 'number', comparators: NUMBER, hint: 'Lowest section y of the chunk', example: 'yPos = -4' },
  { name: 'zPos', format: 'number', comparators: NUMBER, hint: 'Chunk z coordinate', example: 'zPos < 10' },
  {
    name: 'Timestamp',
    format: 'number',
    comparators: NUMBER,
    hint: 'Last save time from the region header: unix seconds or "YYYY-MM-DD hh:mm:ss"',
    example: 'Timestamp < "2024-01-01"',
  },
  {
    name: 'Compression',
    format: 'text',
    comparators: EQUALITY,
    hint: 'Compression type: GZIP, ZLIB, UNCOMPRESSED, LZ4 (or 1-4)',
    example: 'Compression = ZLIB',
  },
  {
    name: 'LastUpdate',
    format: 'number',
    comparators: NUMBER,
    hint: 'World tick of the last chunk update, or a duration like "2 days"',
    example: 'LastUpdate < "1 day"',
  },
  {
    name: 'Palette',
    format: 'text',
    comparators: ['contains', '!contains', 'intersects', '!intersects', '=', '!='],
    hint: "Block names, comma separated; 'custom:block' in single quotes",
    example: 'Palette contains "diamond_ore,emerald_ore"',
  },
  {
    name: 'Biome',
    format: 'text',
    comparators: TEXT,
    hint: "Biome names or ids, comma separated; 'custom:biome' in single quotes",
    example: 'Biome contains "plains"',
  },
  {
    name: 'Status',
    format: 'text',
    comparators: EQUALITY,
    hint: 'Generation status, e.g. full, empty, features',
    example: 'Status = full',
  },
  {
    name: 'LightPopulated',
    format: 'number',
    comparators: EQUALITY,
    hint: '1 or 0 (pre-1.14 light flag)',
    example: 'LightPopulated = 1',
  },
  {
    name: 'Entities',
    format: 'text',
    comparators: TEXT,
    hint: 'Entity ids, comma separated',
    example: 'Entities contains "villager"',
  },
  {
    name: 'Structures',
    format: 'text',
    comparators: TEXT,
    hint: 'Structure names, comma separated',
    example: 'Structures contains "village_plains"',
  },
  {
    name: '#Entities',
    format: 'number',
    comparators: NUMBER,
    hint: 'Number of entities (0 or more)',
    example: '#Entities > 50',
  },
  {
    name: '#ProtoEntities',
    format: 'number',
    comparators: NUMBER,
    hint: 'Number of entities still stored in a proto chunk (0 or more)',
    example: '#ProtoEntities > 0',
  },
  {
    name: '#TileEntities',
    format: 'number',
    comparators: NUMBER,
    hint: 'Number of block entities (0 or more)',
    example: '#TileEntities > 100',
  },
  {
    name: 'Circle',
    format: 'text',
    comparators: ['contains', '!contains'],
    hint: 'Chunk circles "<x>;<z>;<radius>", comma separated',
    example: 'Circle contains "0;0;20"',
  },
  {
    name: 'Border',
    format: 'number',
    comparators: NUMBER,
    hint: 'Missing neighbour chunks, 0 to 4 (chunks at the edge of generated terrain)',
    example: 'Border > 0',
  },
  {
    name: 'Size',
    format: 'number',
    comparators: NUMBER,
    hint: 'World Map: compressed size of the chunk in bytes',
    example: 'Size > 100000',
  },
];

/** MCA Selector filters World Map cannot run: they need files or scripts outside the world. */
const UNAVAILABLE: Record<string, string> = {
  PlayerLocation: 'it reads player data files',
  PlayerSpawn: 'it reads player data files',
  Selection: 'it reads a selection file; use the selection instead',
  Script: 'scripts are not supported',
  Custom: 'scripts are not supported',
};

const typeByName = new Map(FILTER_TYPES.map((type) => [type.name, type]));

class ParseError extends Error {
  readonly position: number;
  constructor(message: string, position: number) {
    super(message);
    this.position = position;
  }
}

/** Java's Character.isWhitespace for the characters a query can contain. */
const isWhitespace = (c: string): boolean => {
  const code = c.charCodeAt(0);
  return (
    (code >= 0x09 && code <= 0x0d) ||
    (code >= 0x1c && code <= 0x20) ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x2006) ||
    (code >= 0x2008 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x205f ||
    code === 0x3000
  );
};

/** FilterParser.isValidCharacter: the characters of an unquoted value. */
const isValueCharacter = (c: string): boolean => /[a-zA-Z0-9,\-+:]/.test(c);

/** MCA Selector's StringPointer. */
class Pointer {
  index = 0;
  readonly value: string;
  constructor(value: string) {
    this.value = value;
  }

  hasNext(): boolean {
    return this.index < this.value.length;
  }

  current(): string {
    if (!this.hasNext()) throw new ParseError('unexpected end of query', this.index);
    return this.value[this.index];
  }

  skipWhitespace(): void {
    while (this.hasNext() && isWhitespace(this.value[this.index])) this.index++;
  }

  simpleString(valid: (c: string) => boolean = (c) => !isWhitespace(c)): string {
    const start = this.index;
    while (this.hasNext() && valid(this.value[this.index])) this.index++;
    return this.value.slice(start, this.index);
  }

  quotedString(): string {
    this.index++;
    let out = '';
    let escape = false;
    while (this.hasNext()) {
      const c = this.value[this.index++];
      if (escape) {
        if (c !== '\\' && c !== '"') throw new ParseError(`invalid escape of '${c}'`, this.index);
        escape = false;
        out += c;
      } else if (c === '\\') escape = true;
      else if (c === '"') return out;
      else out += c;
    }
    throw new ParseError('missing end quote', this.index);
  }

  expect(c: string): void {
    this.skipWhitespace();
    if (this.hasNext() && this.value[this.index] === c) {
      this.index++;
      return;
    }
    const got = this.hasNext() ? `'${this.value[this.index]}'` : 'EOF';
    throw new ParseError(`expected '${c}' but got ${got}`, this.index);
  }
}

/** Java's Integer.parseInt / Long.parseLong / Byte.parseByte range check on a decimal string. */
const parseJavaInteger = (text: string, min: number, max: number): number | null => {
  if (!/^[+-]?\d+$/.test(text)) return null;
  const value = Number(text);
  return value >= min && value <= max ? value : null;
};

const INT_MIN = -2147483648;
const INT_MAX = 2147483647;
const parseInt32 = (text: string): number | null => parseJavaInteger(text, INT_MIN, INT_MAX);

/** Java's String.split(delimiter): trailing empty fields are dropped (an empty input stays one field). */
const javaSplit = (text: string, delimiter: string): string[] => {
  if (text === '') return [''];
  const parts = text.split(delimiter);
  while (parts.length > 0 && parts[parts.length - 1] === '') parts.pop();
  return parts;
};

const DURATION_UNITS: [RegExp, number][] = [
  [/(\d+)[^a-zA-Z0-9_]*(?:years?|y)/, 31536000],
  [/(\d+)[^a-zA-Z0-9_]*months?/, 2592000],
  [/(\d+)[^a-zA-Z0-9_]*(?:days?|d)/, 86400],
  [/(\d+)[^a-zA-Z0-9_]*(?:hours?|h)/, 3600],
  [/(\d+)[^a-zA-Z0-9_]*(?:minutes?|mins?)/, 60],
  [/(\d+)[^a-zA-Z0-9_]*(?:seconds?|secs?|s)/, 1],
];

/** MCA Selector's TextHelper.parseDuration: seconds of a text like "1 day 5 hours" (each unit once), or
 * null. The sum wraps like Java's int arithmetic. */
export const parseDuration = (text: string): number | null => {
  let duration = 0;
  let remains = text;
  let found = false;
  for (const [pattern, seconds] of DURATION_UNITS) {
    const match = pattern.exec(text);
    if (!match) continue;
    found = true;
    duration = (duration + Number(BigInt.asIntN(32, BigInt(match[1]) * BigInt(seconds)))) | 0;
    remains = remains.replace(match[0], '');
  }
  return found && remains.trim() === '' ? duration : null;
};

/** MCA Selector's TextHelper.parseTimestamp ("yyyy-MM-dd[ [HH][:mm][:ss]]", local time): unix seconds
 * (wrapped to a Java int) or null. Day numbers past the month's end are clamped like Java's SMART resolver. */
export const parseTimestamp = (text: string): number | null => {
  const m = /^(\d{4})-(\d{2})-(\d{2})(?: (\d{2})?(?::(\d{2}))?(?::(\d{2}))?)?$/.exec(text.trim());
  if (!m) return null;
  const [year, month, day] = [Number(m[1]), Number(m[2]), Number(m[3])];
  const [hour, minute, second] = [Number(m[4] ?? 0), Number(m[5] ?? 0), Number(m[6] ?? 0)];
  if (month < 1 || month > 12 || day < 1 || day > 31 || hour > 23 || minute > 59 || second > 59) return null;
  const date = new Date(2000, 0, 1, hour, minute, second);
  date.setFullYear(year, month - 1, 1);
  const monthDays = new Date(date.getFullYear(), date.getMonth() + 1, 0).getDate();
  date.setDate(Math.min(day, monthDays));
  return Math.floor(date.getTime() / 1000) | 0;
};

const quote = (raw: string): string => `"${raw.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;

const isQuotedName = (name: string, minLength: number): boolean =>
  name.length >= minLength && name.startsWith("'") && name.endsWith("'");

/** Biome names: name, minecraft:name, numeric id, 'custom' (BiomeFilter.setFilterValue). */
const validBiomes = (raw: string): boolean => {
  const names = javaSplit(raw.replace(/ /g, ''), ',');
  if (raw === '' || names.length === 0) return false;
  return names.every((name) => {
    if (isQuotedName(name, 3) || /^[0-9]+$/.test(name)) return true;
    return /^minecraft:[a-z0-9_]+$/.test(name.startsWith('minecraft:') ? name : `minecraft:${name}`);
  });
};

/** Entity ids: name, minecraft:name, 'custom' (EntityFilter.setFilterValue). */
const validEntities = (raw: string): boolean => {
  const names = javaSplit(raw.replace(/ /g, ''), ',');
  if (raw === '' || names.length === 0) return false;
  return names.every((name) => {
    const m = /^([a-z_]*):?([a-z_]*)$/.exec(name);
    const id = m && m[2] === '' ? `minecraft:${m[1]}` : name;
    return isQuotedName(id, 2) || /^(minecraft:)?[a-z0-9_]+$/.test(id);
  });
};

/**
 * Checks a filter value like the filter's setFilterValue and returns its query form (FilterOverlay's Rust
 * to_query: numbers re-printed, Compression/Status canonical, every other text quoted), or null when invalid.
 */
const normalizeValue = (type: string, raw: string): string | null => {
  const integer = (min: number, max: number): string | null => {
    const value = parseJavaInteger(raw.replace(/ /g, ''), min, max);
    return value === null ? null : String(value);
  };
  switch (type) {
    case 'DataVersion':
    case '#Entities':
    case '#ProtoEntities':
    case '#TileEntities':
    case 'Size':
      return integer(0, INT_MAX);
    case 'xPos':
    case 'yPos':
    case 'zPos':
      return integer(INT_MIN, INT_MAX);
    case 'Border':
      return integer(0, 4);
    case 'LightPopulated':
      return integer(0, 1);
    case 'InhabitedTime':
    case 'LastUpdate':
      return parseJavaInteger(raw.replace(/ /g, ''), -(2 ** 63), 2 ** 63 - 1) !== null || parseDuration(raw) !== null
        ? quote(raw)
        : null;
    case 'Timestamp':
      return parseInt32(raw.replace(/ /g, '')) !== null || parseTimestamp(raw) !== null ? quote(raw) : null;
    case 'Compression': {
      const found = COMPRESSIONS.find(
        ([name, byte]) => name.toLowerCase() === raw.toLowerCase() || String(byte) === raw,
      );
      // `_` is not an unquoted value character: *_EXT names are quoted so the query parses again
      if (!found) return null;
      return found[0].includes('_') ? quote(found[0]) : found[0];
    }
    case 'Status': {
      // always quoted: status names like structure_starts contain `_`
      if (raw.startsWith("'") && raw.endsWith("'")) return quote(raw);
      const name = raw.startsWith('minecraft:') ? raw.slice(10) : raw;
      return STATUSES.includes(name) ? quote(`minecraft:${name}`) : null;
    }
    case 'Palette': {
      const names = javaSplit(raw, ',').map((s) => s.trim());
      const any = names.some((s) => isQuotedName(s, 2) || /^(minecraft:)?[a-z0-9_]+$/.test(s));
      return any ? quote(raw) : null;
    }
    case 'Biome':
      return validBiomes(raw) ? quote(raw) : null;
    case 'Entities':
      return validEntities(raw) ? quote(raw) : null;
    case 'Structures': {
      const names = javaSplit(raw, ',').map((s) => s.trim());
      const valid =
        names.length > 0 && names.every((s) => isQuotedName(s, 2) || /^(minecraft:)?[A-Za-z0-9_]+$/.test(s));
      return valid ? quote(raw) : null;
    }
    case 'Circle': {
      const valid = javaSplit(raw, ',').every((circle) => {
        const parts = javaSplit(circle, ';');
        if (parts.length !== 3) return false;
        const [x, z, r] = parts.map(parseInt32);
        return x !== null && z !== null && r !== null && r >= 0;
      });
      return valid ? quote(raw) : null;
    }
    default:
      return null;
  }
};

/** Whether a value is valid for a filter type (for the builder's inline errors). */
export const isValidValue = (type: string, value: string): boolean => normalizeValue(type, value) !== null;

const parseFilter = (ptr: Pointer, operator: Operator): FilterNode => {
  const typeName = ptr.simpleString();
  const unavailable = UNAVAILABLE[typeName];
  if (unavailable)
    throw new ParseError(`filter type "${typeName}" is not available in World Map: ${unavailable}`, ptr.index);
  const type = typeByName.get(typeName);
  if (!type) throw new ParseError(`invalid filter type "${typeName}"`, ptr.index);
  ptr.skipWhitespace();
  const comparator = ptr.simpleString() as Comparator;
  if (!COMPARATORS.includes(comparator)) throw new ParseError('invalid comparator', ptr.index);
  if (!type.comparators.includes(comparator)) {
    throw new ParseError(`comparator ${comparator} not allowed for filter type ${typeName}`, ptr.index);
  }
  ptr.skipWhitespace();
  const value = ptr.hasNext() && ptr.current() === '"' ? ptr.quotedString() : ptr.simpleString(isValueCharacter);
  if (normalizeValue(typeName, value) === null) throw new ParseError('invalid value', ptr.index);
  return { kind: 'filter', operator, type: typeName, comparator, value };
};

/** Most groups open at once (the backend's limit, guarding its recursion). */
const MAX_DEPTH = 100;

const parseGroup = (
  ptr: Pointer,
  operator: Operator,
  negated: boolean,
  depth: number,
): FilterNode & { kind: 'group' } => {
  const group: FilterNode & { kind: 'group' } = { kind: 'group', operator, negated, children: [] };
  ptr.skipWhitespace();
  let first = true;
  while (ptr.hasNext() && ptr.current() !== ')') {
    let childOperator: Operator = 'AND';
    if (first) first = false;
    else {
      const op = ptr.simpleString();
      if (op !== 'AND' && op !== 'OR') throw new ParseError(`invalid operator ${op}`, ptr.index);
      childOperator = op;
    }
    ptr.skipWhitespace();
    let childNegated = false;
    if (ptr.current() === '!') {
      childNegated = true;
      ptr.index++;
      ptr.skipWhitespace();
    }
    if (ptr.current() === '(') {
      ptr.index++;
      if (depth + 1 > MAX_DEPTH) throw new ParseError('groups nested too deeply', ptr.index);
      group.children.push(parseGroup(ptr, childOperator, childNegated, depth + 1));
      ptr.skipWhitespace();
      ptr.expect(')');
      ptr.skipWhitespace();
      continue;
    }
    if (childNegated) throw new ParseError('only groups can be negated', ptr.index);
    group.children.push(parseFilter(ptr, childOperator));
    ptr.skipWhitespace();
  }
  ptr.skipWhitespace();
  return group;
};

/** Parses an MCA Selector query; `position` is the character index where parsing failed. */
export const parseQuery = (
  text: string,
): { ok: true; root: FilterNode } | { ok: false; error: string; position: number } => {
  const ptr = new Pointer(text);
  try {
    const root = parseGroup(ptr, 'AND', false, 0);
    if (ptr.hasNext()) throw new ParseError(`unexpected '${ptr.current()}'`, ptr.index);
    return { ok: true, root };
  } catch (error) {
    if (error instanceof ParseError) return { ok: false, error: error.message, position: error.position };
    throw error;
  }
};

const nodeQuery = (node: FilterNode, depth: number): string => {
  if (node.kind === 'filter')
    return `${node.type} ${node.comparator} ${normalizeValue(node.type, node.value) ?? quote(node.value)}`;
  const inner = node.children
    .map((child, i) => (i === 0 ? '' : ` ${child.operator} `) + nodeQuery(child, depth + 1))
    .join('');
  if (node.negated) return `!(${inner})`;
  return depth === 0 ? inner : `(${inner})`;
};

/** The normalized query (same as the backend's Filter::to_query): canonical values, single spaces,
 * groups in parentheses, empty groups kept as `()`. Invalid builder values are emitted quoted. */
export const toQuery = (root: FilterNode): string => nodeQuery(root, 0);

/** MCA Selector "Change NBT fields" fields World Map applies (changer/fields). */
export const CHANGE_FIELDS: { name: string; hint: string; example: string; values?: string[] }[] = [
  { name: 'Status', hint: 'Sets the generation status', example: 'full', values: STATUSES },
  {
    name: 'FixStatus',
    hint: 'Sets the status of chunks with full terrain but a lower status to "full"',
    example: 'true',
    values: ['true'],
  },
  { name: 'InhabitedTime', hint: 'Ticks players spent in the chunk (number)', example: '0' },
  { name: 'LastUpdate', hint: 'World tick of the last update (number)', example: '0' },
  { name: 'Timestamp', hint: 'Region header save time, unix seconds', example: '0' },
  { name: 'DataVersion', hint: 'Chunk data version (number, does not convert the data)', example: '3953' },
  {
    name: 'Compression',
    hint: 'Recompresses the chunk',
    example: 'ZLIB',
    values: ['GZIP', 'ZLIB', 'UNCOMPRESSED', 'LZ4'],
  },
  { name: 'Biome', hint: "Sets every biome of the chunk: name, id, or 'custom:biome'", example: 'plains' },
  {
    name: 'ReplaceBlocks',
    hint: 'from=to pairs, comma separated; to may be a block state {Name:...} and ;{tile entity}',
    example: 'stone=air, minecraft:dirt=minecraft:grass_block',
  },
  { name: 'DeleteEntities', hint: 'Deletes every entity', example: 'true', values: ['true'] },
  { name: 'DeleteSections', hint: 'Section y ranges to delete ("true" = all): -4:3, 10, :0, 5:', example: '-4:3,10' },
  { name: 'FixHeightmaps', hint: 'Recalculates the heightmaps', example: 'true', values: ['true'] },
  {
    name: 'DeleteStructureReference',
    hint: 'Structure names whose starts and references are removed, comma separated',
    example: 'village_plains',
  },
  {
    name: 'PreventRetrogen',
    hint: 'Marks old chunks so the game does not regenerate them',
    example: 'true',
    values: ['true'],
  },
  { name: 'ForceBlend', hint: 'Makes the game blend the chunk with new terrain', example: 'true', values: ['true'] },
  {
    name: 'RemoveLightingInfo',
    hint: 'Removes light data so the game relights the chunk',
    example: 'true',
    values: ['true'],
  },
  { name: 'LightPopulated', hint: '1 or 0 (pre-1.14 light flag)', example: '1', values: ['1', '0'] },
];

/** MCA Selector change syntax (ChangeParser): `Name = value, Name = "quoted value"`. Values with characters
 * other than letters, digits, `+`, `-` are quoted; empty values are skipped. */
export const changesText = (fields: { name: string; value: string }[]): string =>
  fields
    .filter((field) => field.value !== '')
    .map((field) => `${field.name} = ${/^[a-zA-Z0-9+-]+$/.test(field.value) ? field.value : quote(field.value)}`)
    .join(', ');

/** MCA Selector overlays World Map renders, plus World Map's `Size`. */
export const OVERLAY_TYPES: { name: string; parameter: boolean; hint: string; unit: string }[] = [
  { name: 'InhabitedTime', parameter: false, hint: 'Time players spent in the chunk', unit: 'ticks' },
  { name: 'Timestamp', parameter: false, hint: 'Last save time from the region header', unit: 'unix seconds' },
  { name: 'LastUpdate', parameter: false, hint: 'World tick of the last chunk update', unit: 'ticks' },
  { name: '#Entities', parameter: false, hint: 'Number of entities', unit: 'entities' },
  { name: '#TileEntities', parameter: false, hint: 'Number of block entities', unit: 'block entities' },
  { name: 'DataVersion', parameter: false, hint: 'Chunk data version', unit: 'version' },
  { name: 'AverageHeight', parameter: false, hint: 'Average terrain height', unit: 'blocks' },
  {
    name: '#Blocks',
    parameter: true,
    hint: 'Number of the given blocks (block names, comma separated)',
    unit: 'blocks',
  },
  { name: 'Size', parameter: false, hint: 'Compressed size of the chunk', unit: 'bytes' },
];
