import { axiosInstance } from '@/api/axios.ts';
import type { WireSelection } from './lib/selection.ts';

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
    /** Change NBT fields. */
    edit_chunks: boolean;
    /** Copy chunks between worlds and import uploaded region files. */
    import_chunks: boolean;
    /** Download selected chunks as a zip. */
    export_chunks: boolean;
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
    /** Most chunks one write operation (delete, change, import) may touch. */
    operation_chunks: number;
    entity_regions: number;
    extra_worlds: number;
  };
  /** Where write operations move the original files, and for how long they are kept. */
  backups: {
    retention_days: number;
    folder: string;
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
  backup_retention_days: number;
}

export const getOverview = async (server: string): Promise<Overview> => (await axiosInstance.get(base(server))).data;

/** The region files of a dimension, plus regions that only have entities/ or poi/ files. */
export interface Regions {
  regions: RegionFile[];
  others: [number, number][];
}

export const getRegions = async (server: string, place: Place): Promise<Regions> =>
  (await axiosInstance.get(`${base(server)}/regions`, { params: place })).data;

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

// Chunk tools (MCA Selector): filters, changes, jobs, overlays, export, imports, operations.

export interface FileNeeds {
  region: boolean;
  entities: boolean;
  poi: boolean;
}

export type CheckResult<T> = ({ ok: true } & T) | { ok: false; error: string; position: number };

export const checkFilter = async (
  server: string,
  query: string,
): Promise<CheckResult<{ normalized: string; needs: FileNeeds }>> =>
  (await axiosInstance.post(`${base(server)}/filter/check`, { query })).data;

export const checkChanges = async (server: string, changes: string): Promise<CheckResult<{ normalized: string }>> =>
  (await axiosInstance.post(`${base(server)}/changes/check`, { changes })).data;

export interface Offset {
  /** Chunks. */
  x: number;
  /** Sections (16 blocks). */
  y: number;
  /** Chunks. */
  z: number;
}

export type JobRequest =
  | { kind: 'filter'; world: string; dimension: string; query: string; selection?: WireSelection | null }
  | { kind: 'delete'; world: string; dimension: string; selection: WireSelection }
  | { kind: 'change'; world: string; dimension: string; selection: WireSelection; changes: string; force: boolean }
  | {
      kind: 'import';
      source: Place | { folder: string };
      target: Place;
      offset: Offset;
      overwrite: boolean;
      source_selection?: WireSelection | null;
      target_selection?: WireSelection | null;
      sections?: string | null;
    };

export type JobKind = 'filter' | 'delete' | 'change' | 'import' | 'restore';
export type OperationKind = 'delete' | 'change' | 'import' | 'restore';

export interface RegionError {
  region: string;
  error: string;
}

export interface Operation {
  id: string;
  kind: OperationKind;
  /** Unix seconds. */
  created: number;
  user: string | null;
  username: string | null;
  world: string;
  dimension: string;
  /** The backup folder, relative to the server root. */
  backup: string;
  files: number;
  summary: Record<string, unknown> | null;
  state: 'done' | 'partial' | 'restored' | 'pruned';
  restorable: boolean;
}

export interface FilterResult {
  matched: number;
  regions_read: number;
  selection: WireSelection;
  errors: RegionError[];
  /** Regions that could not be read. */
  failures: number;
  /** Stopped early (cancelled or failed): the result covers part of the dimension. */
  partial: boolean;
}

export interface WriteResult {
  chunks: number;
  skipped: number;
  files: number;
  operation: Operation | null;
  errors: RegionError[];
  /** Regions that could not be written. */
  failures: number;
  /** Stopped early (cancelled or failed): only part of the work was done. */
  partial: boolean;
}

export interface Job {
  id: string;
  kind: JobKind;
  state: 'running' | 'done' | 'failed' | 'cancelled';
  world: string;
  dimension: string;
  user: string | null;
  /** Unix seconds. */
  started: number;
  finished: number | null;
  /** Regions. */
  progress: { done: number; total: number };
  message: string | null;
  error: string | null;
  result: FilterResult | WriteResult | null;
}

export const isFilterResult = (result: Job['result']): result is FilterResult => !!result && 'matched' in result;

export const createJob = async (server: string, request: JobRequest): Promise<Job> =>
  (await axiosInstance.post(`${base(server)}/jobs`, request)).data.job;

export const getJobs = async (server: string): Promise<Job[]> =>
  (await axiosInstance.get(`${base(server)}/jobs`)).data.jobs;

export const getJob = async (server: string, id: string): Promise<Job> =>
  (await axiosInstance.get(`${base(server)}/jobs/${id}`)).data.job;

export const cancelJob = async (server: string, id: string): Promise<Job> =>
  (await axiosInstance.post(`${base(server)}/jobs/${id}/cancel`)).data.job;

export interface OverlayValues {
  /** 1024 values, index x + z * 32; null where the chunk has no value. */
  values: (number | null)[];
  min: number | null;
  max: number | null;
}

export const getOverlay = async (
  server: string,
  place: Place,
  x: number,
  z: number,
  overlay: string,
  parameter: string | null,
): Promise<OverlayValues> =>
  (
    await axiosInstance.get(`${base(server)}/overlay`, {
      params: { ...place, x, z, overlay, ...(parameter ? { parameter } : {}) },
    })
  ).data;

/** Downloads the selected chunks as a zip of region files, with the name the server gives it. */
export const exportChunks = async (
  server: string,
  place: Place,
  selection: WireSelection,
): Promise<{ blob: Blob; filename: string | null }> => {
  const response = await axiosInstance.post(
    `${base(server)}/export`,
    { ...place, selection },
    { responseType: 'blob' },
  );
  const disposition = String(response.headers['content-disposition'] ?? '');
  const match = /filename\*=UTF-8''([^;]+)|filename="?([^";]+)"?/i.exec(disposition);
  const filename = match ? (match[1] ? decodeURIComponent(match[1]) : match[2]) : null;
  return { blob: response.data, filename };
};

/** Creates an empty staging folder for uploaded region files. */
export const createImport = async (server: string): Promise<string> =>
  (await axiosInstance.post(`${base(server)}/imports`)).data.folder;

/** Extracts an uploaded zip in the staging folder; the folder holding its region/, entities/ and
 * poi/ folders (the import job's source). */
export const extractImport = async (server: string, folder: string, file: string): Promise<string> =>
  (await axiosInstance.post(`${base(server)}/imports/extract`, { folder, file })).data.folder;

export const getOperations = async (server: string): Promise<Operation[]> =>
  (await axiosInstance.get(`${base(server)}/operations`)).data.operations;

export const restoreOperation = async (server: string, id: string): Promise<Job> =>
  (await axiosInstance.post(`${base(server)}/operations/${id}/restore`)).data.job;

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
