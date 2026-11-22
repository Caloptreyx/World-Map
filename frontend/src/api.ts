import { axiosInstance } from '@/api/axios.ts';
import type { ChunkRect } from './lib/geometry.ts';

// The backend's payloads keep their snake_case names.

export const PACKAGE = 'dev.caloptreyx.worldmap';
export const ADMIN_BASE = `/api/admin/extensions/${PACKAGE}`;
const base = (server: string) => `/api/client/servers/${server}/world-map`;

export type Power = 'offline' | 'starting' | 'running' | 'stopping' | 'unknown';

export interface Dimension {
  id: string;
  folder: string;
  /** What `execute in` takes on the running server; null when it does not load this world. */
  key: string | null;
  /** Drawn below a roof (the Nether, datapack dimensions with a ceiling). */
  ceiling: boolean;
}

export interface LevelInfo {
  data_version: number | null;
  version: string | null;
  spawn: [number, number, number] | null;
}

export interface World {
  /** The folder below the server root, also the world's id. */
  name: string;
  /** The world `level-name` points to. */
  main: boolean;
  /** Added by hand in the world folder settings. */
  extra: boolean;
  dimensions: Dimension[];
  players: string | null;
  level: LevelInfo;
}

export interface ExtraWorld {
  path: string;
  found: boolean;
}

export interface Overview {
  unsupported: 'egg' | 'bedrock' | 'not_minecraft' | null;
  too_old: boolean;
  /** `level-name` as server.properties has it. */
  level_name: string;
  /** The folder `level-name` points to, when it exists. */
  main_folder: string | null;
  world_container: string | null;
  /** The main world first. */
  worlds: World[];
  extra_worlds: ExtraWorld[];
  power: Power;
  capabilities: {
    delete_chunks: boolean;
    teleport: boolean;
    reset_heatmap: boolean;
    manage_worlds: boolean;
    manage_players: boolean;
  };
  /** Which players the map shows on this server (a per-server setting). */
  player_visibility: PlayerVisibility;
  heatmap: {
    enabled: boolean;
    interval_minutes: number;
    retention_days: number;
  };
  limits: {
    delete_chunks: number;
    entity_regions: number;
    extra_worlds: number;
  };
}

export interface RegionFile {
  x: number;
  z: number;
  size: number;
  modified: number;
}

export interface Player {
  uuid: string;
  name: string | null;
  world: string;
  dimension: string;
  x: number;
  y: number;
  z: number;
  yaw: number;
  /** When the player file was saved, Unix seconds: the position is from then. */
  last_seen: number;
  /** Named online right now (a player the server does not name counts as offline). */
  online: boolean;
}

export interface PlayerVisibility {
  online: boolean;
  offline: boolean;
}

export type OnlineSource = 'query' | 'ping' | 'console';

/** How sure the player list is about who is online. The counts are left out while the server
 * hides online players. */
export type Presence =
  | { state: 'not_running' }
  | { state: 'unknown'; error: string }
  | {
      state: 'known' | 'partial';
      source: OnlineSource;
      count?: number;
      max?: number;
      named?: number;
    };

export interface Online {
  count: number;
  max: number;
  players: string[];
  complete: boolean;
  source: OnlineSource;
}

export interface Entity {
  id: string;
  x: number;
  y: number;
  z: number;
  name?: string;
}

export interface Heatmap {
  cells: [number, number, number][];
  max: number;
  days: number;
  enabled: boolean;
}

export interface Place {
  world: string;
  dimension: string;
}

export type TeleportTarget =
  | { type: 'player'; name: string }
  | { type: 'position'; world: string; dimension: string; x: number; y?: number; z: number };

export interface Settings {
  eggs: string[];
  heatmap_enabled: boolean;
  heatmap_interval_minutes: number;
  heatmap_retention_days: number;
  max_region_mb: number;
  render_concurrency: number;
  nether_ceiling: number;
  entity_limit: number;
  max_delete_chunks: number;
  tile_cache_mb: number;
}

export const getOverview = async (server: string): Promise<Overview> => (await axiosInstance.get(base(server))).data;

export const getRegions = async (server: string, place: Place): Promise<RegionFile[]> =>
  (await axiosInstance.get(`${base(server)}/regions`, { params: place })).data.regions;

export const getTile = async (server: string, place: Place, region: RegionFile, signal: AbortSignal): Promise<Blob> =>
  (
    await axiosInstance.get(`${base(server)}/tile`, {
      params: { ...place, x: region.x, z: region.z, v: `${region.modified}-${region.size}` },
      responseType: 'blob',
      signal,
    })
  ).data;

export const getPlayers = async (server: string): Promise<{ players: Player[]; presence: Presence | null }> =>
  (await axiosInstance.get(`${base(server)}/players`)).data;

export const getOnline = async (
  server: string,
): Promise<{ running: boolean; online: Online | null; hidden?: boolean }> =>
  (await axiosInstance.get(`${base(server)}/online`)).data;

export const getEntities = async (
  server: string,
  place: Place,
  area: { x1: number; z1: number; x2: number; z2: number },
): Promise<{ entities: Entity[]; truncated: boolean }> =>
  (await axiosInstance.get(`${base(server)}/entities`, { params: { ...place, ...area } })).data;

export const getHeatmap = async (server: string, place: Place, days: number): Promise<Heatmap> =>
  (await axiosInstance.get(`${base(server)}/heatmap`, { params: { ...place, days } })).data;

export const resetHeatmap = async (server: string): Promise<number> =>
  (await axiosInstance.delete(`${base(server)}/heatmap`)).data.deleted;

export const deleteChunks = async (
  server: string,
  place: Place,
  rects: ChunkRect[],
): Promise<{ selected: number; deleted: number; files: number }> =>
  (await axiosInstance.post(`${base(server)}/chunks/delete`, { ...place, rects })).data;

export const updateWorlds = async (server: string, paths: string[]): Promise<ExtraWorld[]> =>
  (await axiosInstance.put(`${base(server)}/worlds`, { extra_worlds: paths })).data.extra_worlds;

export const updatePlayerVisibility = async (server: string, visibility: PlayerVisibility): Promise<PlayerVisibility> =>
  (await axiosInstance.put(`${base(server)}/players/visibility`, visibility)).data.player_visibility;

export const teleport = async (server: string, player: string, target: TeleportTarget): Promise<string> =>
  (await axiosInstance.post(`${base(server)}/teleport`, { player, target })).data.command;

export const getSettings = async (): Promise<Settings> =>
  (await axiosInstance.get(`${ADMIN_BASE}/settings`)).data.settings;

export const updateSettings = async (settings: Settings): Promise<Settings> =>
  (await axiosInstance.put(`${ADMIN_BASE}/settings`, settings)).data.settings;
