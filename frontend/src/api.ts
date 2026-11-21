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
}

export interface World {
  name: string;
  dimensions: Dimension[];
  players: string | null;
}

export interface Overview {
  unsupported: 'egg' | 'bedrock' | 'not_minecraft' | null;
  too_old: boolean;
  level_name: string;
  level: {
    data_version: number | null;
    version: string | null;
    spawn: [number, number, number] | null;
  };
  worlds: World[];
  power: Power;
  capabilities: {
    delete_chunks: boolean;
    teleport: boolean;
    reset_heatmap: boolean;
  };
  heatmap: {
    enabled: boolean;
    interval_minutes: number;
    retention_days: number;
  };
  limits: {
    delete_chunks: number;
    entity_regions: number;
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
  last_seen: number;
}

export interface Online {
  count: number;
  max: number;
  players: string[];
  complete: boolean;
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

export const getPlayers = async (server: string): Promise<Player[]> =>
  (await axiosInstance.get(`${base(server)}/players`)).data.players;

export const getOnline = async (server: string): Promise<{ running: boolean; online: Online | null }> =>
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

export const teleport = async (server: string, player: string, target: TeleportTarget): Promise<string> =>
  (await axiosInstance.post(`${base(server)}/teleport`, { player, target })).data.command;

export const getSettings = async (): Promise<Settings> =>
  (await axiosInstance.get(`${ADMIN_BASE}/settings`)).data.settings;

export const updateSettings = async (settings: Settings): Promise<Settings> =>
  (await axiosInstance.put(`${ADMIN_BASE}/settings`, settings)).data.settings;
