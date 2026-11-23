import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { describe, test } from 'node:test';
import {
  CHANGE_FIELDS,
  FILTER_TYPES,
  OVERLAY_TYPES,
  changesText,
  isValidValue,
  parseDuration,
  parseQuery,
  parseTimestamp,
  toQuery,
  type FilterNode,
} from '../frontend/src/lib/filter.ts';

const normalized = (text: string): string => {
  const result = parseQuery(text);
  assert.ok(result.ok, `${text}: ${result.ok ? '' : `${result.error} at ${result.position}`}`);
  return toQuery(result.root);
};

const failure = (text: string): { error: string; position: number } => {
  const result = parseQuery(text);
  assert.ok(!result.ok, `${text} should not parse`);
  return { error: result.error, position: result.position };
};

describe('query parser', () => {
  test('builds the tree with operators and negated groups', () => {
    const result = parseQuery('xPos >= 0 AND !( Status = full OR zPos < -3 )');
    assert.ok(result.ok);
    const expected: FilterNode = {
      kind: 'group',
      operator: 'AND',
      negated: false,
      children: [
        { kind: 'filter', operator: 'AND', type: 'xPos', comparator: '>=', value: '0' },
        {
          kind: 'group',
          operator: 'AND',
          negated: true,
          children: [
            { kind: 'filter', operator: 'AND', type: 'Status', comparator: '=', value: 'full' },
            { kind: 'filter', operator: 'OR', type: 'zPos', comparator: '<', value: '-3' },
          ],
        },
      ],
    };
    assert.deepEqual(result.root, expected);
  });

  test('normalizes like the backend', () => {
    assert.equal(normalized('  xPos   >=  +5 AND zPos < -0'), 'xPos >= 5 AND zPos < 0');
    assert.equal(
      normalized('Status = "carvers" OR Status = minecraft:full'),
      'Status = "minecraft:carvers" OR Status = "minecraft:full"',
    );
    assert.equal(normalized('Compression = "zlib"'), 'Compression = ZLIB');
    assert.equal(normalized('Compression != -127'), 'Compression != "GZIP_EXT"');
    assert.equal(normalized('InhabitedTime < "1 minute"'), 'InhabitedTime < "1 minute"');
    assert.equal(normalized('InhabitedTime > 0'), 'InhabitedTime > "0"');
    assert.equal(normalized('Timestamp > 2026-10-01'), 'Timestamp > "2026-10-01"');
    assert.equal(normalized('Palette contains "a\\"b\\\\c,stone"'), 'Palette contains "a\\"b\\\\c,stone"');
    assert.equal(normalized('Status = "\'custom\'"'), 'Status = "\'custom\'"');
    assert.equal(normalized('DataVersion = "1 000"'), 'DataVersion = 1000');
    assert.equal(normalized('((xPos = 1)) OR !(zPos = 2)'), '((xPos = 1)) OR !(zPos = 2)');
    assert.equal(normalized('() AND xPos = 1'), '() AND xPos = 1');
    assert.equal(normalized('!()'), '!()');
    assert.equal(normalized(''), '');
    assert.equal(normalized('Circle contains "1;2;3,-4;5;0"'), 'Circle contains "1;2;3,-4;5;0"');
  });

  test('reports errors with positions', () => {
    assert.deepEqual(failure('xPos>5'), { error: 'invalid filter type "xPos>5"', position: 6 });
    assert.deepEqual(failure('foo = 1'), { error: 'invalid filter type "foo"', position: 3 });
    assert.deepEqual(failure('xPos == 1'), { error: 'invalid comparator', position: 7 });
    assert.deepEqual(failure('xPos contains 1'), {
      error: 'comparator contains not allowed for filter type xPos',
      position: 13,
    });
    assert.deepEqual(failure('xPos = abc'), { error: 'invalid value', position: 10 });
    assert.deepEqual(failure('xPos = 1 and zPos = 1'), { error: 'invalid operator and', position: 12 });
    assert.deepEqual(failure('!xPos = 1'), { error: 'only groups can be negated', position: 1 });
    assert.deepEqual(failure('(xPos = 1'), { error: "expected ')' but got EOF", position: 9 });
    assert.deepEqual(failure('Palette contains "stone'), { error: 'missing end quote', position: 23 });
    assert.deepEqual(failure('Palette contains "a\\nb"'), { error: "invalid escape of 'n'", position: 21 });
    assert.equal(failure('xPos = 1)').position, 8);
    assert.deepEqual(failure('xPos = 1 AND'), { error: 'unexpected end of query', position: 12 });
    assert.deepEqual(failure('!'), { error: 'unexpected end of query', position: 1 });
    assert.deepEqual(failure('xPos = '), { error: 'invalid value', position: 7 });
    assert.deepEqual(failure('xPos = 1)'), { error: "unexpected ')'", position: 8 });
    assert.match(failure('PlayerLocation contains "x"').error, /not available in World Map/);
    assert.deepEqual(failure('Group = 1'), { error: 'invalid filter type "Group"', position: 5 });
    assert.equal(
      normalized(`${'('.repeat(100)}xPos = 1${')'.repeat(100)}`),
      `${'('.repeat(100)}xPos = 1${')'.repeat(100)}`,
    );
    assert.deepEqual(failure(`${'( '.repeat(101)}xPos = 1${')'.repeat(101)}`), {
      error: 'groups nested too deeply',
      position: 201,
    });
  });

  test('validates values per filter', () => {
    const valid: [string, string][] = [
      ['DataVersion', '3953'],
      ['xPos', '-2147483648'],
      ['Border', '4'],
      ['LightPopulated', '1'],
      ['InhabitedTime', '1 hour 30 mins'],
      ['LastUpdate', '2d'],
      ['Timestamp', '2024-02-30 12'],
      ['Compression', 'LZ4'],
      ['Status', 'minecraft:features'],
      ['Palette', "stone,'mod:block'"],
      ['Biome', "plains,1,'mod:x'"],
      ['Entities', 'villager,minecraft:cow'],
      ['Structures', 'Village,minecraft:shipwreck'],
      ['Circle', '0;0;10,'],
      ['Size', '1000'],
    ];
    for (const [type, value] of valid) assert.ok(isValidValue(type, value), `${type} ${value}`);
    const invalid: [string, string][] = [
      ['DataVersion', '-1'],
      ['xPos', '2147483648'],
      ['Border', '5'],
      ['LightPopulated', '2'],
      ['InhabitedTime', '5 lightyears'],
      ['Timestamp', '2024-13-01'],
      ['Compression', 'brotli'],
      ['Status', 'done'],
      ['Biome', ''],
      ['Circle', '0;0;-1'],
      ['Circle', '0;0'],
      ['#Entities', '-3'],
    ];
    for (const [type, value] of invalid) assert.ok(!isValidValue(type, value), `${type} ${value}`);
  });

  test('durations and timestamps like TextHelper', () => {
    assert.equal(parseDuration('5 minutes'), 300);
    assert.equal(
      parseDuration('1 year 2 months 3 days 4h 5min 6s'),
      31536000 + 2 * 2592000 + 3 * 86400 + 4 * 3600 + 5 * 60 + 6,
    );
    assert.equal(parseDuration('5'), null);
    assert.equal(parseDuration('5 minutes and more'), null);
    const local = new Date(2026, 9, 1).getTime() / 1000;
    assert.equal(parseTimestamp('2026-10-01'), local);
    assert.equal(parseTimestamp(' 2026-10-01 01:02:03 '), local + 3723);
    assert.equal(parseTimestamp('2026-02-31'), new Date(2026, 1, 28).getTime() / 1000);
    assert.equal(parseTimestamp('26-10-01'), null);
  });

  test('every query MCA Selector ran for the fixtures parses', () => {
    const file = new URL('./fixtures/mcaselector/queries-expected.json', import.meta.url);
    if (!existsSync(file)) return;
    for (const query of Object.keys(JSON.parse(readFileSync(file, 'utf8')))) {
      const result = parseQuery(query);
      assert.ok(result.ok, query);
      const again = parseQuery(toQuery(result.root));
      assert.ok(again.ok, query);
      assert.equal(toQuery(again.root), toQuery(result.root), query);
    }
  });

  test('matches the backend parser cases', (t) => {
    const file = new URL('./fixtures/queries.json', import.meta.url);
    if (!existsSync(file)) {
      t.skip('no tests/fixtures/queries.json');
      return;
    }
    const cases: { query: string; ok: boolean; normalized?: string; error_position?: number }[] = JSON.parse(
      readFileSync(file, 'utf8'),
    );
    // only the backend knows MCA Selector's block, biome, entity and structure registries
    const registry = /\b(Palette|Biome|Entities|Structures)\b/;
    for (const c of cases) {
      const result = parseQuery(c.query);
      if (c.ok) {
        assert.ok(result.ok, `${c.query}: ${result.ok ? '' : result.error}`);
        if (c.normalized !== undefined) assert.equal(toQuery(result.root), c.normalized, c.query);
      } else if (!result.ok) {
        if (c.error_position !== undefined) assert.equal(result.position, c.error_position, c.query);
      } else {
        assert.match(c.query, registry, `${c.query} should not parse`);
      }
    }
  });
});

describe('builder tables', () => {
  test('filter, change and overlay types', () => {
    assert.equal(new Set(FILTER_TYPES.map((t) => t.name)).size, 20);
    for (const type of FILTER_TYPES) {
      const result = parseQuery(type.example);
      assert.ok(result.ok, type.example);
      assert.ok(result.root.kind === 'group' && result.root.children[0].kind === 'filter');
    }
    assert.deepEqual(
      CHANGE_FIELDS.map((f) => f.name),
      [
        'Status',
        'FixStatus',
        'InhabitedTime',
        'LastUpdate',
        'Timestamp',
        'DataVersion',
        'Compression',
        'Biome',
        'ReplaceBlocks',
        'DeleteEntities',
        'DeleteSections',
        'FixHeightmaps',
        'DeleteStructureReference',
        'PreventRetrogen',
        'ForceBlend',
        'RemoveLightingInfo',
        'LightPopulated',
      ],
    );
    assert.equal(
      OVERLAY_TYPES.filter((o) => o.parameter)
        .map((o) => o.name)
        .join(),
      '#Blocks',
    );
  });

  test('change text uses the ChangeParser syntax', () => {
    assert.equal(
      changesText([
        { name: 'Status', value: 'full' },
        { name: 'InhabitedTime', value: '-5' },
        { name: 'Biome', value: '' },
        { name: 'ReplaceBlocks', value: 'stone=air, minecraft:dirt="x"' },
        { name: 'DeleteSections', value: '-4:3,10' },
      ]),
      'Status = full, InhabitedTime = -5, ReplaceBlocks = "stone=air, minecraft:dirt=\\"x\\"", DeleteSections = "-4:3,10"',
    );
  });
});
