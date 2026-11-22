import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import {
  effectiveLayers,
  isOnline,
  restoreLayers,
  shownPlayers,
  whyNoPlayers,
} from '../frontend/src/lib/players.ts';

const defaults = {
  onlinePlayers: true,
  offlinePlayers: true,
  markers: true,
  heatmap: false,
  mobs: true,
  entities: false,
};

const player = (name: string | null, online: boolean) => ({
  uuid: `${name ?? 'unnamed'}-uuid`,
  name,
  world: 'world',
  dimension: 'minecraft:overworld',
  x: 0,
  y: 64,
  z: 0,
  yaw: 0,
  last_seen: 0,
  online,
});

const steve = player('Steve', true);
const alex = player('Alex', false);
const unnamed = player(null, false);
const everyone = [steve, alex, unnamed];
const both = { online: true, offline: true };

describe('online and offline players', () => {
  test('the player list or the teleport check makes a player online', () => {
    assert.equal(isOnline(steve, null), true);
    assert.equal(isOnline(alex, null), false);
    // the console's list may name a player the ping left out, in any case
    const online = { count: 2, max: 20, players: ['Steve', 'alex'], complete: true, source: 'console' as const };
    assert.equal(isOnline(alex, online), true);
    assert.equal(isOnline(unnamed, online), false);
  });

  test('each toggle shows its kind of player', () => {
    const names = (layers: typeof defaults) => shownPlayers(everyone, layers, null).map((p) => p.name);
    assert.deepEqual(names(defaults), ['Steve', 'Alex', null]);
    assert.deepEqual(names({ ...defaults, offlinePlayers: false }), ['Steve']);
    assert.deepEqual(names({ ...defaults, onlinePlayers: false }), ['Alex', null]);
    assert.deepEqual(names({ ...defaults, onlinePlayers: false, offlinePlayers: false }), []);
  });

  test('a kind the server hides stays off whatever the viewer chose', () => {
    const drawn = effectiveLayers(defaults, { online: false, offline: true });
    assert.equal(drawn.onlinePlayers, false);
    assert.equal(drawn.offlinePlayers, true);
    assert.equal(drawn.mobs, true);
    // the viewer's choice comes back when the server shows the kind again
    assert.deepEqual(effectiveLayers(defaults, both), defaults);
  });

  test('an empty list says why', () => {
    assert.equal(whyNoPlayers({ online: false, offline: false }, defaults), 'hidden');
    assert.equal(whyNoPlayers(both, { ...defaults, onlinePlayers: false, offlinePlayers: false }), 'layers');
    assert.equal(whyNoPlayers(both, defaults), 'none');
    assert.equal(whyNoPlayers(both, { ...defaults, offlinePlayers: false }), 'online');
    const offlineOnly = effectiveLayers(defaults, { online: false, offline: true });
    assert.equal(whyNoPlayers({ online: false, offline: true }, offlineOnly), 'offline');
  });

  test('saved view state from 0.1.1 keeps the players layer choice', () => {
    assert.deepEqual(restoreLayers(defaults, undefined), defaults);
    assert.deepEqual(restoreLayers(defaults, 'garbage'), defaults);
    const old = { players: false, markers: true, heatmap: true, mobs: false, entities: false };
    assert.deepEqual(restoreLayers(defaults, old), {
      onlinePlayers: false,
      offlinePlayers: false,
      markers: true,
      heatmap: true,
      mobs: false,
      entities: false,
    });
    const current = { ...defaults, offlinePlayers: false, heatmap: 'yes' };
    assert.deepEqual(restoreLayers(defaults, current), { ...defaults, offlinePlayers: false });
  });
});
