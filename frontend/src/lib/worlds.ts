// Picking the world and dimension to show, and telling why there is none.
import type { Dimension, Overview, Place, World } from '../api.ts';

export const VANILLA_DIMENSIONS = ['minecraft:overworld', 'minecraft:the_nether', 'minecraft:the_end'];

/** The place to open: the saved one while it exists (the saved world's first dimension when only
 * the dimension is gone), else the main world's first dimension (the first world's without one). */
export function initialPlace(worlds: World[], saved: Partial<Place> | undefined): Place | null {
  const world = worlds.find((candidate) => candidate.name === saved?.world) ?? worlds[0];
  if (!world) return null;
  const dimension = world.dimensions.find((candidate) => candidate.id === saved?.dimension) ?? world.dimensions[0];
  return { world: world.name, dimension: dimension.id };
}

/** The world and dimension of a place, when they still exist. */
export function findPlace(worlds: World[], place: Place): { world: World; dimension: Dimension } | null {
  const world = worlds.find((candidate) => candidate.name === place.world);
  const dimension = world?.dimensions.find((candidate) => candidate.id === place.dimension);
  return world && dimension ? { world, dimension } : null;
}

type WorldsOverview = Pick<Overview, 'worlds' | 'main_folder' | 'level_name'>;

/** Why no map can be drawn at all: no world yet, or `level-name` names a folder that is not
 * there (a fresh server with the default name has simply not generated its world yet). */
export function emptyReason(overview: WorldsOverview): 'no_world' | 'level_missing' | null {
  if (overview.worlds.length > 0) return null;
  return overview.main_folder === null && overview.level_name !== 'world' ? 'level_missing' : 'no_world';
}

/** What is wrong with the main world while other worlds are shown: its folder is missing, or it
 * holds no saved terrain yet. */
export function mainProblem(overview: WorldsOverview): 'missing' | 'empty' | null {
  if (overview.worlds.length === 0 || overview.worlds.some((world) => world.main)) return null;
  return overview.main_folder === null ? 'missing' : 'empty';
}

/** Whether the world's spawn marker belongs on this dimension: the world's first one (its
 * Overworld when it has one). */
export function showsSpawn(world: World, dimension: string): boolean {
  return !!world.level.spawn && world.dimensions[0]?.id === dimension;
}
