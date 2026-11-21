import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { entityIcon, entityLayer, entityName, mobKind, vanillaPath } from '../frontend/src/lib/entities.ts';

describe('entities', () => {
  test('creatures are mobs, everything else is an entity', () => {
    assert.equal(entityLayer('minecraft:zombie'), 'mobs');
    assert.equal(entityLayer('minecraft:villager'), 'mobs');
    assert.equal(entityLayer('minecraft:item'), 'entities');
    assert.equal(entityLayer('minecraft:oak_boat'), 'entities');
    assert.equal(entityLayer('minecraft:bamboo_chest_raft'), 'entities');
    assert.equal(entityLayer('minecraft:hopper_minecart'), 'entities');
    assert.equal(entityLayer('minecraft:armor_stand'), 'entities');
    // modded creatures are mobs, modded boats are entities
    assert.equal(entityLayer('alexsmobs:grizzly_bear'), 'mobs');
    assert.equal(entityLayer('mymod:redwood_boat'), 'entities');
    // object prototype names are not entities
    assert.equal(entityLayer('minecraft:constructor'), 'mobs');
  });

  test('hostile and passive mobs', () => {
    assert.equal(mobKind('minecraft:creeper'), 'hostile');
    assert.equal(mobKind('minecraft:cow'), 'passive');
    assert.equal(mobKind('mymod:creeper'), 'passive');
  });

  test('icons come from spawn eggs and items, only for vanilla ids', () => {
    assert.equal(entityIcon('minecraft:zombie'), 'https://mc.nerothe.com/img/1.26.2/minecraft_zombie_spawn_egg.png');
    assert.equal(entityIcon('minecraft:chest_minecart'), 'https://mc.nerothe.com/img/1.26.2/minecraft_chest_minecart.png');
    assert.equal(entityIcon('minecraft:experience_orb'), 'https://mc.nerothe.com/img/1.26.2/minecraft_experience_bottle.png');
    assert.equal(entityIcon('minecraft:marker'), null);
    assert.equal(entityIcon('mymod:thing'), null);
    assert.equal(entityIcon('minecraft:../../etc'), null);
  });

  test('names and paths', () => {
    assert.equal(entityName('minecraft:zombie_villager'), 'Zombie Villager');
    assert.equal(vanillaPath('pig'), 'pig');
    assert.equal(vanillaPath('mod:pig'), null);
  });
});
