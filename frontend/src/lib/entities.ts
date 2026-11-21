// Sorting entities into the map's layers and finding icons for them.

/** Rendered item and spawn egg icons by item id (see Caloptreyx/Player-Manager for the checks). */
const ICON_HOST = 'https://mc.nerothe.com/img/1.26.2';

export type EntityLayer = 'mobs' | 'entities';
export type MobKind = 'hostile' | 'passive';

/** Entities that are not living creatures: items, projectiles, vehicles, decorations, markers. */
const NOT_MOBS: Record<string, true> = {
  item: true,
  experience_orb: true,
  arrow: true,
  spectral_arrow: true,
  trident: true,
  snowball: true,
  egg: true,
  ender_pearl: true,
  eye_of_ender: true,
  fireball: true,
  small_fireball: true,
  dragon_fireball: true,
  wither_skull: true,
  shulker_bullet: true,
  llama_spit: true,
  potion: true,
  splash_potion: true,
  lingering_potion: true,
  experience_bottle: true,
  firework_rocket: true,
  fishing_bobber: true,
  falling_block: true,
  tnt: true,
  armor_stand: true,
  item_frame: true,
  glow_item_frame: true,
  painting: true,
  leash_knot: true,
  end_crystal: true,
  area_effect_cloud: true,
  marker: true,
  interaction: true,
  block_display: true,
  item_display: true,
  text_display: true,
  evoker_fangs: true,
  lightning_bolt: true,
  wind_charge: true,
  breeze_wind_charge: true,
  ominous_item_spawner: true,
};

const HOSTILE: Record<string, true> = {
  blaze: true,
  bogged: true,
  breeze: true,
  cave_spider: true,
  creaking: true,
  creeper: true,
  drowned: true,
  elder_guardian: true,
  ender_dragon: true,
  endermite: true,
  evoker: true,
  ghast: true,
  guardian: true,
  hoglin: true,
  husk: true,
  illusioner: true,
  magma_cube: true,
  phantom: true,
  piglin_brute: true,
  pillager: true,
  ravager: true,
  shulker: true,
  silverfish: true,
  skeleton: true,
  slime: true,
  spider: true,
  stray: true,
  vex: true,
  vindicator: true,
  warden: true,
  witch: true,
  wither: true,
  wither_skeleton: true,
  zoglin: true,
  zombie: true,
  zombie_villager: true,
};

/** Entities whose icon is an item of another name. */
const ITEM_FOR: Record<string, string> = {
  experience_orb: 'experience_bottle',
  fishing_bobber: 'fishing_rod',
  eye_of_ender: 'ender_eye',
  potion: 'splash_potion',
  breeze_wind_charge: 'wind_charge',
  leash_knot: 'lead',
  small_fireball: 'fire_charge',
  fireball: 'fire_charge',
};

/** Entities named like their item. */
const ITEM_NAMED: Record<string, true> = {
  arrow: true,
  spectral_arrow: true,
  trident: true,
  snowball: true,
  egg: true,
  ender_pearl: true,
  splash_potion: true,
  lingering_potion: true,
  experience_bottle: true,
  firework_rocket: true,
  wind_charge: true,
  tnt: true,
  armor_stand: true,
  item_frame: true,
  glow_item_frame: true,
  painting: true,
  end_crystal: true,
};

/** Vehicles named like their item: every boat, raft and minecart. */
const VEHICLE_SUFFIXES = ['_boat', '_raft', 'minecart'];

/** The path of a vanilla id (`minecraft:zombie` -> `zombie`); null for other namespaces. */
export const vanillaPath = (id: string): string | null => {
  const [namespace, path] = id.includes(':') ? id.split(':', 2) : ['minecraft', id];
  return namespace === 'minecraft' && /^[a-z0-9_]+$/.test(path) ? path : null;
};

const isVehicle = (path: string): boolean => VEHICLE_SUFFIXES.some((suffix) => path.endsWith(suffix));

export const entityLayer = (id: string): EntityLayer => {
  const path = vanillaPath(id) ?? id.split(':').pop() ?? id;
  return Object.hasOwn(NOT_MOBS, path) || isVehicle(path) ? 'entities' : 'mobs';
};

export const mobKind = (id: string): MobKind => (Object.hasOwn(HOSTILE, vanillaPath(id) ?? '') ? 'hostile' : 'passive');

/** An icon URL for an entity: its spawn egg for mobs, its item for vehicles and the like. */
export const entityIcon = (id: string): string | null => {
  const path = vanillaPath(id);
  if (!path) return null;
  if (entityLayer(id) === 'mobs') return `${ICON_HOST}/minecraft_${path}_spawn_egg.png`;
  if (Object.hasOwn(ITEM_FOR, path)) return `${ICON_HOST}/minecraft_${ITEM_FOR[path]}.png`;
  return Object.hasOwn(ITEM_NAMED, path) || isVehicle(path) ? `${ICON_HOST}/minecraft_${path}.png` : null;
};

/** `minecraft:zombie_villager` -> `Zombie Villager`. */
export const entityName = (id: string): string =>
  (id.split(':').pop() ?? id)
    .split('_')
    .filter(Boolean)
    .map((word) => word[0].toUpperCase() + word.slice(1))
    .join(' ');
