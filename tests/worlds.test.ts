import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { emptyReason, findPlace, initialPlace, mainProblem, showsSpawn } from '../frontend/src/lib/worlds.ts';

const dimension = (id: string, folder: string) => ({ id, folder, key: id, ceiling: false });
const level = (spawn: [number, number, number] | null) => ({ data_version: 4000, version: '26.3', spawn });

const survival = {
  name: 'survival',
  main: true,
  extra: false,
  players: 'survival/playerdata',
  level: level([10, 64, -20]),
  dimensions: [
    dimension('minecraft:overworld', 'survival'),
    dimension('minecraft:the_nether', 'survival_nether/DIM-1'),
    dimension('mypack:mining', 'survival/dimensions/mypack/mining'),
  ],
};
const lobby = {
  name: 'worlds/lobby',
  main: false,
  extra: true,
  players: null,
  level: level(null),
  dimensions: [dimension('minecraft:the_end', 'worlds/lobby/DIM1')],
};
const worlds = [survival, lobby];

describe('worlds', () => {
  test('opens the saved place while it exists, else the main world', () => {
    assert.deepEqual(initialPlace(worlds, { world: 'worlds/lobby', dimension: 'minecraft:the_end' }), {
      world: 'worlds/lobby',
      dimension: 'minecraft:the_end',
    });
    assert.deepEqual(initialPlace(worlds, { world: 'survival', dimension: 'mypack:mining' }), {
      world: 'survival',
      dimension: 'mypack:mining',
    });
    // the dimension is gone: the same world's first one
    assert.deepEqual(initialPlace(worlds, { world: 'survival', dimension: 'mypack:old' }), {
      world: 'survival',
      dimension: 'minecraft:overworld',
    });
    // the world is gone (level-name changed): the main world
    assert.deepEqual(initialPlace(worlds, { world: 'world', dimension: 'minecraft:overworld' }), {
      world: 'survival',
      dimension: 'minecraft:overworld',
    });
    assert.deepEqual(initialPlace(worlds, undefined), { world: 'survival', dimension: 'minecraft:overworld' });
    assert.equal(initialPlace([], undefined), null);
  });

  test('finds places only in their own world', () => {
    assert.equal(findPlace(worlds, { world: 'survival', dimension: 'mypack:mining' })?.dimension.folder, 'survival/dimensions/mypack/mining');
    assert.equal(findPlace(worlds, { world: 'worlds/lobby', dimension: 'minecraft:overworld' }), null);
  });

  test('tells a missing level-name from a world not generated yet', () => {
    assert.equal(emptyReason({ worlds, main_folder: 'survival', level_name: 'survival' }), null);
    assert.equal(emptyReason({ worlds: [], main_folder: null, level_name: 'world' }), 'no_world');
    assert.equal(emptyReason({ worlds: [], main_folder: 'survival', level_name: 'survival' }), 'no_world');
    assert.equal(emptyReason({ worlds: [], main_folder: null, level_name: 'survival' }), 'level_missing');

    assert.equal(mainProblem({ worlds, main_folder: 'survival', level_name: 'survival' }), null);
    assert.equal(mainProblem({ worlds: [lobby], main_folder: null, level_name: 'survival' }), 'missing');
    assert.equal(mainProblem({ worlds: [lobby], main_folder: 'survival', level_name: 'survival' }), 'empty');
    assert.equal(mainProblem({ worlds: [], main_folder: null, level_name: 'survival' }), null);
  });

  test('puts the spawn marker on the world it belongs to', () => {
    assert.equal(showsSpawn(survival, 'minecraft:overworld'), true);
    assert.equal(showsSpawn(survival, 'minecraft:the_nether'), false);
    assert.equal(showsSpawn(lobby, 'minecraft:the_end'), false);
  });
});
