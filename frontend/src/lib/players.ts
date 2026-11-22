// Which players the map draws: online or offline, as the viewer's layers and the server's
// setting allow.
import type { Online, Player, PlayerVisibility } from '../api.ts';
import type { Layers } from '../components/MapCanvas.tsx';

/** Whether the player is online: as the player list says, or named by the teleport check (which
 * may also ask the console). */
export const isOnline = (player: Player, online: Online | null): boolean =>
  player.online ||
  (!!player.name && !!online?.players.some((name) => name.toLowerCase() === player.name!.toLowerCase()));

/** The layers the viewer can turn on: a kind of player the server hides stays off. */
export const effectiveLayers = (layers: Layers, visibility: PlayerVisibility): Layers => ({
  ...layers,
  onlinePlayers: layers.onlinePlayers && visibility.online,
  offlinePlayers: layers.offlinePlayers && visibility.offline,
});

/** The players the layers show. */
export const shownPlayers = (players: Player[], layers: Layers, online: Online | null): Player[] =>
  players.filter((player) => (isOnline(player, online) ? layers.onlinePlayers : layers.offlinePlayers));

export type NoPlayers = 'hidden' | 'layers' | 'none' | 'online' | 'offline';

/** Why the player list is empty: the server hides everyone, both layers are off, or nobody of
 * the kinds shown is there. `drawn` are the layers after the server's setting. */
export function whyNoPlayers(visibility: PlayerVisibility, drawn: Layers): NoPlayers {
  if (!visibility.online && !visibility.offline) return 'hidden';
  if (drawn.onlinePlayers && drawn.offlinePlayers) return 'none';
  if (drawn.onlinePlayers) return 'online';
  if (drawn.offlinePlayers) return 'offline';
  return 'layers';
}

/** The saved layers over the defaults; 0.1.1 saved one `players` layer, which now sets both kinds. */
export function restoreLayers(defaults: Layers, saved: unknown): Layers {
  const layers = { ...defaults };
  if (!saved || typeof saved !== 'object') return layers;
  const record = saved as Record<string, unknown>;
  if (typeof record.players === 'boolean') {
    layers.onlinePlayers = record.players;
    layers.offlinePlayers = record.players;
  }
  for (const key of Object.keys(defaults) as (keyof Layers)[]) {
    const value = record[key];
    if (typeof value === 'boolean') layers[key] = value;
  }
  return layers;
}
