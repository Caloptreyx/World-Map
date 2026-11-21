import { type DefinedTranslations, defineTranslations, type GetPlaceholders, type PathValue } from 'shared';

const translations = defineTranslations({
  items: {},
  translations: {
    title: 'World Map',
    subtitle: 'Your world, at a glance: players, mobs and the busy areas, read straight from the server files.',
    unsupported: {
      title: 'No world map for this server',
      egg: 'The administrator has not enabled the world map for this kind of server.',
      bedrock: 'This is a Bedrock server. The world map reads Minecraft Java worlds.',
      notMinecraft: 'This does not look like a Minecraft Java server: there is no server.properties.',
      noWorld: 'No world was found yet. Start the server once so it generates its world.',
      tooOld:
        'This world was last saved by a Minecraft version older than 1.13, which the map cannot draw. Open it with a newer version first.',
    },
    overview: {
      title: 'World Overview',
      description: 'Data is streamed directly from the server files',
      badge: '2D Map',
      lastView: 'Last view',
      spawn: 'Spawn',
      jump: 'Jump to coordinates',
      jumpButton: 'Jump',
      world: 'World',
      dimension: 'Dimension',
      version: 'Saved by {version}',
    },
    dimensions: {
      overworld: 'Overworld',
      nether: 'Nether',
      end: 'End',
    },
    players: {
      title: 'Player Selection',
      description: 'Choose a player to locate their last known location',
      placeholder: 'Select a player',
      none: 'No player has joined yet.',
      online: 'Online',
      offline: 'Offline',
      lastSeen: 'Last saved {time}',
      elsewhere: 'In {world} · {dimension}',
      locate: 'Locate on the map',
      teleport: 'Teleport',
      teleportToPlayer: 'Teleport to {name}',
      teleportHere: 'Pick a spot on the map',
      teleportHint: 'Click the map where {name} should go. Esc cancels.',
      teleported: 'Sent: {command}',
      teleportOffline: 'The server is not running.',
      teleportNotOnline: '{name} is not online.',
      onlineUnknown: 'Who is online could not be checked: {error}',
      refreshOnline: 'Check who is online',
      resetHeatmap: 'Reset the heatmap',
      resetHeatmapTitle: 'Reset Heatmap',
      resetHeatmapContent: 'Delete every collected player position of this server? The heatmap starts over.',
      resetHeatmapDone: 'The heatmap was reset.',
    },
    toolbar: {
      home: 'Go to spawn',
      players: 'Players',
      markers: 'Markers',
      heatmap: 'Player heatmap',
      mobs: 'Mobs',
      entities: 'Other entities',
      select: 'Select chunks',
      fullscreen: 'Fullscreen',
      help: 'Help',
    },
    cursor: {
      title: 'Cursor',
      chunk: 'Chunk {x}, {z}',
      region: 'Region {x}, {z}',
    },
    map: {
      zoomInTiles: 'Zoom in to load more of the map',
      zoomInEntities: 'Zoom in to see mobs and entities',
      entitiesTruncated: 'Showing the first {count} entities here',
      loading: 'Rendering {count} regions…',
      tileFailed: 'Some regions could not be drawn: {error}',
      spawn: 'Spawn',
      copyCoordinates: 'Copy coordinates',
      copied: 'Copied {text}',
      teleportHere: 'Teleport {name} here',
    },
    heatmap: {
      legend: 'Player activity, last {days} days',
      empty: 'No samples yet. The heatmap fills while players are online.',
      disabled: 'The administrator turned the heatmap off.',
    },
    selection: {
      hint: 'Drag to select chunks, hold Shift to add to the selection. Esc clears it.',
      count: '{count} chunks selected',
      clear: 'Clear',
      copy: 'Copy coordinates',
      delete: 'Delete chunks',
      deleteOnline: 'Stop the server to delete chunks.',
      deleteTitle: 'Delete Chunks',
      deleteContent:
        'Delete {count} chunks in {dimension}? Their terrain, entities and points of interest are removed and generate again the next time they load. Builds in them are lost.',
      backup: 'Create a backup first: this cannot be undone from the map.',
      tooMany: 'Select at most {limit} chunks at once.',
      deleted: 'Deleted {deleted} chunks in {files} files.',
    },
    help: {
      title: 'World Map Help',
      move: 'Drag the map to move, scroll to zoom, double-click to zoom in.',
      layers: 'The buttons at the bottom right toggle players, the spawn marker, the heatmap, mobs and other entities.',
      entities: 'Mobs and entities show once you zoom in; nearby ones are grouped with a count.',
      heatmap:
        'The heatmap counts where players were each time the server saved them (every few minutes while they are online).',
      select:
        'Chunk selection: drag a rectangle, Shift adds to it, the middle mouse button still moves the map. Selected chunks can be deleted while the server is stopped.',
      data: 'Everything is read from the server files, so the map shows the world as last saved.',
      menu: 'Right-click the map to copy coordinates or teleport the selected player there.',
    },
    settings: {
      access: 'Access',
      eggs: 'Eggs',
      eggsDescription: 'Only servers of these eggs get the map. Leave empty for every Minecraft Java server.',
      heatmap: 'Heatmap',
      heatmapEnabled: 'Collect player positions for the heatmap',
      heatmapInterval: 'Minutes between samples',
      heatmapIntervalDescription: 'Minecraft saves player files every 5 minutes by default.',
      heatmapRetention: 'Days to keep samples',
      rendering: 'Rendering',
      maxRegion: 'Largest region file to read (MiB)',
      concurrency: 'Regions rendered at the same time',
      netherCeiling: 'Nether: draw from this height down',
      netherCeilingDescription: 'Starts below the bedrock roof so the caves show.',
      entityLimit: 'Most entities per view',
      maxDelete: 'Most chunks per deletion',
      tileCache: 'Tile cache size (MiB)',
      tileCacheDescription: 'Rendered regions are kept in the database; the least recently viewed are removed first.',
      save: 'Save',
      saved: 'Settings saved.',
    },
  },
});

export const useExtTranslations = translations.useTranslations.bind(translations);
export const getExtTranslations = translations.getTranslations.bind(translations);

type Messages = (typeof translations)['obj'];
type ExtKey = typeof translations extends DefinedTranslations<infer _I, infer _O, infer P> ? P : never;
type ExtValues<K extends ExtKey> = Record<GetPlaceholders<PathValue<Messages, K> & string>[number], string | number>;
/** `t` of `useExtTranslations`, for helpers outside components. */
export type ExtT = <K extends ExtKey>(key: K, values: ExtValues<K>) => string;

export default translations;
