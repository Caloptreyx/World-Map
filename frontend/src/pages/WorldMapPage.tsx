import {
  faCopy,
  faEarthEurope,
  faGear,
  faLocationArrow,
  faLocationDot,
  faToolbox,
  faTrash,
  faUsers,
} from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { useDebouncedValue, useMediaQuery } from '@mantine/hooks';
import { useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { type RefObject, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import ActionIcon from '@/elements/buttons/ActionIcon.tsx';
import Button from '@/elements/buttons/Button.tsx';
import ServerContentContainer from '@/elements/containers/ServerContentContainer.tsx';
import Badge from '@/elements/data-display/Badge.tsx';
import Alert from '@/elements/feedback/Alert.tsx';
import Group from '@/elements/layout/Group.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import ConfirmationModal from '@/elements/modals/ConfirmationModal.tsx';
import Drawer from '@/elements/overlays/Drawer.tsx';
import Tooltip from '@/elements/overlays/Tooltip.tsx';
import Text from '@/elements/typography/Text.tsx';
import { useToast } from '@/providers/ToastProvider.tsx';
import { useServerStore } from '@/stores/server.ts';
import {
  getEntities,
  getHeatmap,
  getOnline,
  getOverlay,
  getOverview,
  getPlayers,
  getRegions,
  type Overview,
  PACKAGE,
  type Place,
  type Player,
  resetHeatmap,
  teleport,
  type TeleportTarget,
} from '../api.ts';
import ChunkTools from '../components/ChunkTools.tsx';
import FloatingPanel from '../components/FloatingPanel.tsx';
import { jobSummary } from '../components/JobProgress.tsx';
import MapCanvas, { type Layers, type OverlayCells, type TileStatus } from '../components/MapCanvas.tsx';
import OverlayLegend from '../components/OverlayLegend.tsx';
import { DEFAULT_OVERLAY, type OverlayChoice, overlayBound, overlayReady } from '../components/OverlaySettings.tsx';
import OverviewContent, { placeLabel } from '../components/OverviewCard.tsx';
import PlayersContent, { avatarUrl, playerLabel } from '../components/PlayerCard.tsx';
import PlayerVisibilityModal from '../components/PlayerVisibilityModal.tsx';
import Toolbar from '../components/Toolbar.tsx';
import { type Brush, type Granularity, SelectionControls } from '../components/toolsCommon.tsx';
import { useChunkJobs } from '../components/useChunkJob.ts';
import WorldsModal from '../components/WorldsModal.tsx';
import {
  CHUNK_BLOCKS,
  chunkOf,
  MAX_TILE_REQUESTS,
  regionOf,
  type Size,
  toScreen,
  type View,
  visibleBounds,
  visibleRegions,
} from '../lib/geometry.ts';
import { effectiveLayers, isOnline, restoreLayers, shownPlayers, whyNoPlayers } from '../lib/players.ts';
import { count, emptySelection, isEmpty, regionKey, type Selection } from '../lib/selection.ts';
import { emptyReason, findPlace, initialPlace, showsSpawn } from '../lib/worlds.ts';
import { useExtTranslations } from '../translations.ts';

const DEFAULT_LAYERS: Layers = {
  onlinePlayers: true,
  offlinePlayers: true,
  markers: true,
  heatmap: false,
  mobs: true,
  entities: false,
};
/** Below this map width the panels become sheets and the toolbar folds its layers into a menu. */
const COMPACT_WIDTH = 900;
/** Below this many screen pixels per chunk, clicks and drags select whole regions. */
const AUTO_REGION_PIXELS = 4;

interface Saved {
  place: Place;
  view: View;
  layers: Layers;
  overlay: OverlayChoice;
}

const storageKey = (server: string) => `${PACKAGE}:view:${server}`;

function loadSaved(server: string): Partial<Saved> {
  try {
    return JSON.parse(localStorage.getItem(storageKey(server)) ?? '{}');
  } catch {
    return {};
  }
}

function useSize(ref: RefObject<HTMLDivElement | null>): Size {
  const [size, setSize] = useState<Size>({ width: 0, height: 0 });
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) =>
      setSize({ width: entry.contentRect.width, height: entry.contentRect.height }),
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref.current]);
  return size;
}

/** The height that fills the window below the element's top edge (the map takes the rest of the
 * screen instead of a fixed share of it). */
function useFillHeight(ref: RefObject<HTMLDivElement | null>): number {
  const [height, setHeight] = useState(560);
  useEffect(() => {
    const measure = () => {
      const top = ref.current?.getBoundingClientRect().top ?? 0;
      setHeight(Math.max(360, Math.round(window.innerHeight - top - 16)));
    };
    measure();
    window.addEventListener('resize', measure);
    return () => window.removeEventListener('resize', measure);
  }, [ref.current]);
  return height;
}

export default function WorldMapPage() {
  const { t } = useExtTranslations();
  const server = useServerStore((state) => state.server.uuid);
  const phone = useMediaQuery('(max-width: 48em)');
  const overview = useQuery({ queryKey: [PACKAGE, server, 'overview'], queryFn: () => getOverview(server) });

  return (
    <ServerContentContainer title={t('title', {})} subtitle={t('subtitle', {})} hideTitleComponent={phone}>
      {overview.isLoading ? (
        <div className='wm-skeleton' style={{ height: 'min(70dvh, 640px)' }} aria-busy='true' />
      ) : overview.error ? (
        <Alert color='red'>{httpErrorToHuman(overview.error)}</Alert>
      ) : overview.data ? (
        <MapOrReason server={server} overview={overview.data} />
      ) : null}
    </ServerContentContainer>
  );
}

/** The map, or why it cannot be shown. */
function MapOrReason({ server, overview }: { server: string; overview: Overview }) {
  const { t } = useExtTranslations();
  const empty = emptyReason(overview);
  const reason = overview.unsupported
    ? t(
        overview.unsupported === 'egg'
          ? 'unsupported.egg'
          : overview.unsupported === 'bedrock'
            ? 'unsupported.bedrock'
            : 'unsupported.notMinecraft',
        {},
      )
    : empty === 'level_missing'
      ? t('unsupported.levelMissing', { name: overview.level_name })
      : empty === 'no_world'
        ? t('unsupported.noWorld', {})
        : overview.too_old
          ? t('unsupported.tooOld', {})
          : null;
  if (!reason) return <WorldMap key={server} server={server} overview={overview} />;
  return (
    <Alert color='yellow' title={t('unsupported.title', {})}>
      {reason}
    </Alert>
  );
}

interface MenuState {
  x: number;
  z: number;
  left: number;
  top: number;
}

type Sheet = 'world' | 'players' | null;

function WorldMap({ server, overview }: { server: string; overview: Overview }) {
  const { t } = useExtTranslations();
  const { addToast } = useToast();
  const queryClient = useQueryClient();
  const power = useServerStore((state) => state.state);
  const running = power === 'running';
  const saved = useMemo(() => loadSaved(server), [server]);

  const [place, setPlace] = useState<Place>(() => initialPlace(overview.worlds, saved.place)!);
  const [view, setView] = useState<View>(() => {
    if (saved.view && saved.place?.world === place.world) return saved.view;
    const spawn = overview.worlds.find((world) => world.name === place.world)?.level.spawn;
    return { x: spawn ? spawn[0] : 0, z: spawn ? spawn[2] : 0, zoom: 1 };
  });
  // the viewer's choice; `drawn` below also applies the server's player setting
  const [layers, setLayers] = useState<Layers>(() => restoreLayers(DEFAULT_LAYERS, saved.layers));
  const [selecting, setSelecting] = useState(false);
  const [selection, setSelection] = useState<Selection>(emptySelection);
  const [granularity, setGranularity] = useState<Granularity>('chunk');
  const [brush, setBrush] = useState<Brush>('add');
  const [toolsOpen, setToolsOpen] = useState(false);
  const [overlay, setOverlay] = useState<OverlayChoice>(() => ({ ...DEFAULT_OVERLAY, ...saved.overlay }));
  const [cursor, setCursor] = useState<[number, number] | null>(null);
  const [selectedUuid, setSelectedUuid] = useState<string | null>(null);
  const [picking, setPicking] = useState<Player | null>(null);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [sheet, setSheet] = useState<Sheet>(null);
  const [tileStatus, setTileStatus] = useState<TileStatus>({ loading: 0, skipped: 0, error: null });
  const [confirmReset, setConfirmReset] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  const [foldersOpen, setFoldersOpen] = useState(false);
  const [visibilityOpen, setVisibilityOpen] = useState(false);
  const container = useRef<HTMLDivElement>(null);
  const size = useSize(container);
  const fillHeight = useFillHeight(container);
  const compact = size.width > 0 && size.width < COMPACT_WIDTH;

  useEffect(() => {
    localStorage.setItem(storageKey(server), JSON.stringify({ place, view, layers, overlay } satisfies Saved));
  }, [server, place, view, layers, overlay]);

  // the world or dimension went away (folders or level-name changed): back to one that exists
  useEffect(() => {
    if (findPlace(overview.worlds, place)) return;
    const next = initialPlace(overview.worlds, place);
    if (next) setPlace(next);
  }, [overview.worlds, place]);

  // leaving the browser's fullscreen (Esc) leaves the map's too
  useEffect(() => {
    const onChange = () => {
      if (!document.fullscreenElement) setFullscreen(false);
    };
    document.addEventListener('fullscreenchange', onChange);
    return () => document.removeEventListener('fullscreenchange', onChange);
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      setMenu(null);
      setPicking(null);
      // Esc in a field or a dialog closes that, it does not throw the selection away
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest('input, textarea, select, [role="dialog"]')) return;
      setSelection(emptySelection());
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const regions = useQuery({
    queryKey: [PACKAGE, server, 'regions', place.world, place.dimension],
    queryFn: () => getRegions(server, place),
    refetchInterval: 60_000,
  });
  const regionFiles = regions.data?.regions;
  // regions with any file: what inverted selections, counts and overlays cover
  const existing = useMemo<[number, number][]>(
    () => [
      ...(regions.data?.regions ?? []).map((region): [number, number] => [region.x, region.z]),
      ...(regions.data?.others ?? []),
    ],
    [regions.data],
  );
  const existingKeys = useMemo(() => new Set(existing.map(([x, z]) => regionKey(x, z))), [existing]);
  const jobs = useChunkJobs(server);
  const visibility = overview.player_visibility;
  const players = useQuery({
    // refetched when the server starts or stops: who is online changes with it
    queryKey: [PACKAGE, server, 'players', running],
    queryFn: () => getPlayers(server),
    refetchInterval: 30_000,
    placeholderData: (previous) => previous,
  });
  const online = useQuery({
    queryKey: [PACKAGE, server, 'online'],
    queryFn: () => getOnline(server),
    enabled: overview.capabilities.teleport && running && visibility.online,
    staleTime: 30_000,
    retry: false,
  });
  const heat = useQuery({
    queryKey: [PACKAGE, server, 'heatmap', place.world, place.dimension],
    queryFn: () => getHeatmap(server, place, overview.heatmap.retention_days),
    enabled: layers.heatmap,
    refetchInterval: 5 * 60_000,
  });

  // entities only for a few regions at a time, once the view settles
  const [settled] = useDebouncedValue(view, 400);
  const entityArea = useMemo(() => {
    if ((!layers.mobs && !layers.entities) || size.width === 0) return null;
    if (visibleRegions(settled, size).length > overview.limits.entity_regions) return null;
    const bounds = visibleBounds(settled, size);
    return {
      x1: Math.floor(bounds.minX / 16) * 16,
      z1: Math.floor(bounds.minZ / 16) * 16,
      x2: Math.ceil(bounds.maxX / 16) * 16,
      z2: Math.ceil(bounds.maxZ / 16) * 16,
    };
  }, [settled, size, layers.mobs, layers.entities]);
  const entities = useQuery({
    queryKey: [PACKAGE, server, 'entities', place.world, place.dimension, entityArea],
    queryFn: () => getEntities(server, place, entityArea!),
    enabled: !!entityArea,
    staleTime: 30_000,
    placeholderData: (previous) => previous,
  });

  const drawn = effectiveLayers(layers, visibility);
  const allPlayers = players.data?.players ?? [];
  const presence = players.data?.presence ?? null;
  const onlineNow = online.data?.online ?? null;
  const listed = shownPlayers(allPlayers, drawn, onlineNow);
  const selected = listed.find((player) => player.uuid === selectedUuid) ?? null;
  const here = listed.filter((player) => player.world === place.world && player.dimension === place.dimension);
  const current = findPlace(overview.worlds, place);
  const spawn = current?.world.level.spawn ?? null;
  const spawnHere = !!current && showsSpawn(current.world, place.dimension);
  // the server must load this world under a known key to teleport into it
  const canTeleportHere = !!current?.dimension.key;
  const chunkCount = useMemo(() => count(selection, existing), [selection, existing]);
  const autoRegion = CHUNK_BLOCKS * view.zoom < AUTO_REGION_PIXELS;
  const regionMode = autoRegion || granularity === 'region';

  // overlay values per region on screen, like tiles: only while few regions are visible
  const overlayOn = overlayReady(overlay);
  const overlayRegions = useMemo(() => {
    if (!overlayOn || size.width === 0) return [];
    const visible = visibleRegions(settled, size).filter(([x, z]) => existingKeys.has(regionKey(x, z)));
    return visible.length > MAX_TILE_REQUESTS ? null : visible;
  }, [overlayOn, settled, size, existingKeys]);
  const modified = useMemo(
    () =>
      new Map(
        (regionFiles ?? []).map((region) => [regionKey(region.x, region.z), `${region.modified}-${region.size}`]),
      ),
    [regionFiles],
  );
  const overlayResults = useQueries({
    queries: (overlayRegions ?? []).map(([x, z]) => ({
      queryKey: [
        PACKAGE,
        server,
        'overlay',
        place.world,
        place.dimension,
        overlay.type,
        overlay.parameter,
        x,
        z,
        modified.get(regionKey(x, z)) ?? '',
      ],
      queryFn: () => getOverlay(server, place, x, z, overlay.type, overlay.parameter.trim() || null),
      staleTime: 5 * 60_000,
      retry: false,
    })),
  });
  const overlayStamp = overlayResults.map((result) => `${result.dataUpdatedAt}`).join(',');
  const overlayLoading = overlayResults.filter((result) => result.isLoading).length;
  const overlayError = overlayResults.find((result) => result.error)?.error ?? null;
  const minBound = overlayBound(overlay, overlay.min);
  const maxBound = overlayBound(overlay, overlay.max);
  const overlayCells = useMemo<OverlayCells | null>(() => {
    if (!overlayOn || !overlayRegions) return null;
    const cells = new Map<string, (number | null)[]>();
    let low = Number.POSITIVE_INFINITY;
    let high = Number.NEGATIVE_INFINITY;
    overlayRegions.forEach(([x, z], index) => {
      const data = overlayResults[index]?.data;
      if (!data) return;
      cells.set(regionKey(x, z), data.values);
      if (data.min !== null) low = Math.min(low, data.min);
      if (data.max !== null) high = Math.max(high, data.max);
    });
    const min = typeof minBound === 'number' ? minBound : low;
    const max = typeof maxBound === 'number' ? maxBound : high;
    if (!Number.isFinite(min) || !Number.isFinite(max)) return cells.size > 0 ? { cells, min: 0, max: 0 } : null;
    return { cells, min, max };
  }, [overlayOn, overlayRegions, overlayStamp, minBound, maxBound]);
  const onlineCount = allPlayers.filter((player) => isOnline(player, onlineNow)).length;
  const offlineCount = allPlayers.length - onlineCount;
  const noPlayers = whyNoPlayers(visibility, drawn);

  const goTo = (target: Place, x: number, z: number, zoom?: number) => {
    setPlace({ world: target.world, dimension: target.dimension });
    setView((current) => ({ x, z, zoom: zoom ?? current.zoom }));
  };
  const changePlace = (target: Place) => {
    // another world: its own spawn, the same coordinates mean nothing there
    if (target.world !== place.world) {
      const targetSpawn = overview.worlds.find((world) => world.name === target.world)?.level.spawn;
      setView((now) => ({ x: targetSpawn ? targetSpawn[0] : 0, z: targetSpawn ? targetSpawn[2] : 0, zoom: now.zoom }));
    }
    setPlace(target);
    setSelection(emptySelection());
  };
  const locate = (player: Player) => {
    goTo(player, player.x, player.z, Math.max(view.zoom, 2));
    setSelectedUuid(player.uuid);
    setSheet(null);
  };

  const runTeleport = async (player: Player, target: TeleportTarget) => {
    try {
      const command = await teleport(server, player.name!, target);
      addToast(t('players.teleported', { command }), 'success');
      queryClient.invalidateQueries({ queryKey: [PACKAGE, server, 'players'] });
    } catch (error) {
      addToast(httpErrorToHuman(error), 'error');
    }
  };
  const teleportHere = (player: Player, x: number, z: number) =>
    runTeleport(player, { type: 'position', world: place.world, dimension: place.dimension, x, z });

  const copy = async (text: string) => {
    await navigator.clipboard.writeText(text);
    addToast(t('map.copied', { text }), 'success');
  };

  const onMenu = useCallback((x: number, z: number, clientX: number, clientY: number) => {
    const rect = container.current!.getBoundingClientRect();
    setMenu({ x, z, left: clientX - rect.left, top: clientY - rect.top });
  }, []);
  const onTap = useCallback(
    (x: number, z: number, clientX: number, clientY: number) => {
      setCursor([x, z]);
      onMenu(x, z, clientX, clientY);
    },
    [onMenu],
  );

  const toggleFullscreen = () => {
    if (fullscreen) {
      setFullscreen(false);
      if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    } else {
      setFullscreen(true);
      // iOS Safari has no element fullscreen: the map still covers the page
      document.documentElement.requestFullscreen?.().catch(() => {});
    }
  };

  const toggleTools = () => {
    setToolsOpen((open) => !open);
    setSheet(null);
  };
  const selectionControls = (
    <SelectionControls
      granularity={granularity}
      autoRegion={autoRegion}
      onGranularity={setGranularity}
      brush={brush}
      onBrush={setBrush}
    />
  );

  const runReset = async () => {
    try {
      await resetHeatmap(server);
      addToast(t('players.resetHeatmapDone', {}), 'success');
      setConfirmReset(false);
      queryClient.invalidateQueries({ queryKey: [PACKAGE, server, 'heatmap'] });
    } catch (error) {
      addToast(httpErrorToHuman(error), 'error');
    }
  };

  const marker = (x: number, z: number) => toScreen(view, size, x, z);
  const notices = [
    tileStatus.loading > 0 ? t('map.loading', { count: tileStatus.loading }) : null,
    tileStatus.skipped > 0 ? t('map.zoomInTiles', {}) : null,
    tileStatus.error ? t('map.tileFailed', { error: tileStatus.error }) : null,
    (layers.mobs || layers.entities) && !entityArea ? t('map.zoomInEntities', {}) : null,
    entities.data?.truncated ? t('map.entitiesTruncated', { count: entities.data.entities.length }) : null,
    layers.heatmap && heat.data && heat.data.max === 0
      ? t(overview.heatmap.enabled ? 'heatmap.empty' : 'heatmap.disabled', {})
      : null,
    overlayOn && overlayRegions === null ? t('overlay.zoomIn', {}) : null,
    overlayOn && overlayLoading > 0 ? t('overlay.loading', { count: overlayLoading }) : null,
    overlayError ? t('overlay.failed', { error: httpErrorToHuman(overlayError) }) : null,
    jobs.job && (!toolsOpen || compact) && jobs.job.state === 'running' ? jobSummary(t, jobs.job) : null,
  ].filter((notice): notice is string => !!notice);

  const overviewContent = (
    <OverviewContent
      overview={overview}
      place={place}
      regions={regionFiles?.length ?? null}
      onPlace={(target) => {
        changePlace(target);
        setSheet(null);
      }}
      onJump={(x, z) => {
        setView((current) => ({ ...current, x, z }));
        setSheet(null);
      }}
      onFolders={
        overview.capabilities.manage_worlds
          ? () => {
              setFoldersOpen(true);
              setSheet(null);
            }
          : undefined
      }
    />
  );
  const playersContent = (
    <PlayersContent
      players={listed}
      empty={
        noPlayers === 'hidden'
          ? t('players.emptyHidden', {})
          : noPlayers === 'layers'
            ? t('players.emptyLayers', {})
            : noPlayers === 'online'
              ? t('players.emptyOnline', {})
              : noPlayers === 'offline'
                ? t('players.emptyOffline', {})
                : t('players.none', {})
      }
      selected={selected}
      onSelect={setSelectedUuid}
      online={onlineNow}
      onlineError={online.error ? httpErrorToHuman(online.error) : null}
      presence={presence}
      onlineHidden={!visibility.online}
      running={running}
      canTeleport={overview.capabilities.teleport}
      canPickSpot={canTeleportHere}
      onLocate={locate}
      onTeleportTo={(player, name) => runTeleport(player, { type: 'player', name })}
      onPick={(player) => {
        setPicking(player);
        setSelecting(false);
        setSheet(null);
      }}
      onRefreshOnline={() => {
        online.refetch();
        players.refetch();
      }}
    />
  );
  const playersCount = (drawn.onlinePlayers || drawn.offlinePlayers) && (
    <Group gap={6}>
      {drawn.onlinePlayers && (
        <Badge size='sm' variant='light' color={onlineCount > 0 ? 'green' : 'gray'}>
          {t('players.countOnline', { count: onlineCount })}
        </Badge>
      )}
      {drawn.offlinePlayers && (
        <Badge size='sm' variant='light' color='gray'>
          {t('players.countOffline', { count: offlineCount })}
        </Badge>
      )}
    </Group>
  );
  const visibilityButton = overview.capabilities.manage_players && (
    <Tooltip label={t('players.visibility', {})} withinPortal>
      <ActionIcon
        variant='subtle'
        size={28}
        aria-label={t('players.visibility', {})}
        onClick={() => {
          setVisibilityOpen(true);
          setSheet(null);
        }}
      >
        <FontAwesomeIcon icon={faGear} size='xs' />
      </ActionIcon>
    </Tooltip>
  );
  const resetButton = overview.capabilities.reset_heatmap && (
    <Tooltip label={t('players.resetHeatmap', {})} withinPortal>
      <ActionIcon
        variant='subtle'
        color='red'
        size={28}
        aria-label={t('players.resetHeatmap', {})}
        onClick={() => setConfirmReset(true)}
      >
        <FontAwesomeIcon icon={faTrash} size='xs' />
      </ActionIcon>
    </Tooltip>
  );
  const playersActions = (visibilityButton || resetButton) && (
    <Group gap={2} wrap='nowrap'>
      {visibilityButton}
      {resetButton}
    </Group>
  );
  const bottomOffset = compact ? 64 : 12;

  return (
    <div
      ref={container}
      className='wm-map'
      data-fullscreen={fullscreen}
      style={fullscreen ? { position: 'fixed', inset: 0, zIndex: 150, height: '100dvh' } : { height: fillHeight }}
    >
      <MapCanvas
        server={server}
        place={place}
        regions={regionFiles ?? []}
        existing={existing}
        view={view}
        size={size}
        onView={setView}
        layers={layers}
        heat={layers.heatmap ? (heat.data ?? null) : null}
        overlay={overlayCells}
        entities={entityArea ? (entities.data?.entities ?? []) : []}
        selecting={selecting}
        selection={selection}
        onSelection={setSelection}
        regionMode={regionMode}
        brush={brush}
        picking={!!picking}
        onPick={(x, z) => {
          if (picking) teleportHere(picking, x, z);
          setPicking(null);
        }}
        onCursor={setCursor}
        onMenu={onMenu}
        onTap={onTap}
        onTiles={setTileStatus}
      />

      {regionFiles && regionFiles.length === 0 && (
        <div className='wm-empty'>
          <div className='wm-glass'>
            <Text size='sm' fw={600}>
              {t('map.emptyTitle', {})}
            </Text>
            <Text className='wm-label' mt={4}>
              {t('map.empty', {})}
            </Text>
          </div>
        </div>
      )}

      {layers.markers && spawnHere && spawn && (
        <div
          title={t('map.spawn', {})}
          style={{
            position: 'absolute',
            left: marker(spawn[0], spawn[2])[0],
            top: marker(spawn[0], spawn[2])[1],
            transform: 'translate(-50%, -100%)',
            color: '#facc15',
            fontSize: 22,
            pointerEvents: 'none',
            filter: 'drop-shadow(0 1px 2px rgba(0,0,0,0.8))',
            zIndex: 2,
          }}
        >
          <FontAwesomeIcon icon={faLocationDot} />
        </div>
      )}

      {here.map((player) => {
        const [left, top] = marker(player.x, player.z);
        if (left < -40 || top < -40 || left > size.width + 40 || top > size.height + 40) return null;
        const isSelected = player.uuid === selectedUuid;
        return (
          <button
            key={player.uuid}
            type='button'
            className='wm-marker'
            data-selected={isSelected}
            data-online={isOnline(player, onlineNow)}
            aria-label={`${playerLabel(player)} (${isOnline(player, onlineNow) ? t('players.online', {}) : t('players.offline', {})})`}
            onClick={() => setSelectedUuid(isSelected ? null : player.uuid)}
            style={{ left, top, zIndex: isSelected ? 4 : 3 }}
          >
            <span style={{ position: 'relative' }}>
              <img className='wm-marker-head' src={avatarUrl(player.uuid, 48)} alt='' />
              {isOnline(player, onlineNow) && <span className='wm-marker-online' />}
            </span>
            <span className='wm-marker-pin' />
            <span className='wm-marker-name'>{playerLabel(player)}</span>
          </button>
        );
      })}

      {compact ? (
        <Group gap={8} style={{ position: 'absolute', top: 12, left: 12, right: 12, zIndex: 5 }}>
          <button
            type='button'
            className='wm-glass wm-pill'
            aria-expanded={sheet === 'world'}
            onClick={() => setSheet(sheet === 'world' ? null : 'world')}
          >
            <FontAwesomeIcon icon={faEarthEurope} />
            {placeLabel(t, overview, place)}
          </button>
          <button
            type='button'
            className='wm-glass wm-pill'
            aria-expanded={sheet === 'players'}
            onClick={() => setSheet(sheet === 'players' ? null : 'players')}
          >
            <FontAwesomeIcon icon={faUsers} />
            {selected ? playerLabel(selected) : t('players.short', {})}
            {drawn.onlinePlayers && onlineCount > 0 && (
              <Badge size='xs' color='green' variant='filled' circle>
                {onlineCount}
              </Badge>
            )}
          </button>
        </Group>
      ) : (
        <>
          <div style={{ position: 'absolute', top: 12, left: 12, zIndex: 5 }}>
            <FloatingPanel
              icon={<FontAwesomeIcon icon={faEarthEurope} />}
              title={t('overview.title', {})}
              collapseLabel={t('overview.collapse', {})}
              actions={
                <Badge size='sm' variant='light'>
                  {t('overview.badge', {})}
                </Badge>
              }
            >
              {overviewContent}
            </FloatingPanel>
          </div>
          {/* the chunk tools take the right column while they are open */}
          <div style={{ position: 'absolute', top: 12, right: 12, zIndex: 5, display: toolsOpen ? 'none' : undefined }}>
            <FloatingPanel
              icon={<FontAwesomeIcon icon={faUsers} />}
              title={t('players.title', {})}
              collapseLabel={t('players.collapse', {})}
              actions={playersActions}
            >
              <Stack gap='sm'>
                {playersCount}
                {playersContent}
              </Stack>
            </FloatingPanel>
          </div>
        </>
      )}

      <Drawer
        opened={compact && sheet !== null}
        onClose={() => setSheet(null)}
        position='bottom'
        radius='lg'
        styles={{ content: { height: 'auto', maxHeight: '85dvh', flex: '0 0 100%', width: '100%' } }}
        title={
          <Group gap={8} wrap='nowrap'>
            <FontAwesomeIcon icon={sheet === 'players' ? faUsers : faEarthEurope} />
            <Text fw={600}>{sheet === 'players' ? t('players.title', {}) : t('overview.title', {})}</Text>
          </Group>
        }
        overlayProps={{ backgroundOpacity: 0.25, blur: 2 }}
      >
        <Stack gap='sm' pb='md'>
          {sheet === 'players' && (
            <Group justify='space-between'>
              {playersCount}
              {playersActions}
            </Group>
          )}
          {sheet === 'players' ? playersContent : overviewContent}
        </Stack>
      </Drawer>

      <div
        style={{
          position: 'absolute',
          bottom: bottomOffset,
          left: 12,
          zIndex: 5,
          maxWidth: compact ? 'calc(100% - 24px)' : 360,
        }}
      >
        <Stack gap={6} align='flex-start'>
          {notices.map((notice) => (
            <div key={notice} className='wm-glass' style={{ padding: '4px 10px' }}>
              <Text size='xs'>{notice}</Text>
            </div>
          ))}
          {overlayCells && overlayCells.cells.size > 0 && (
            <OverlayLegend
              type={overlay.type}
              parameter={overlay.parameter}
              min={overlayCells.min}
              max={overlayCells.max}
            />
          )}
          {layers.heatmap && heat.data && heat.data.max > 0 && (
            <div className='wm-glass' style={{ padding: '6px 10px', width: 190 }}>
              <Text className='wm-label'>{t('heatmap.legend', { days: heat.data.days })}</Text>
              <div
                style={{
                  marginTop: 5,
                  height: 5,
                  borderRadius: 3,
                  background: 'linear-gradient(90deg, #285aff, #28dc78, #fadc28, #f03228)',
                }}
              />
            </div>
          )}
          <div className='wm-glass' style={{ padding: '6px 10px', minWidth: 172 }}>
            <Text className='wm-label'>{compact ? t('cursor.tapTitle', {}) : t('cursor.title', {})}</Text>
            {cursor ? (
              <>
                <div className='wm-mono'>
                  X {Math.floor(cursor[0])} · Z {Math.floor(cursor[1])}
                </div>
                <div className='wm-mono' style={{ color: 'var(--mantine-color-dimmed)', fontSize: 11 }}>
                  {t('cursor.chunk', { x: chunkOf(cursor[0]), z: chunkOf(cursor[1]) })} ·{' '}
                  {t('cursor.region', { x: regionOf(cursor[0]), z: regionOf(cursor[1]) })}
                </div>
              </>
            ) : (
              <Text size='xs' c='dimmed'>
                {compact ? t('cursor.tapHint', {}) : '-'}
              </Text>
            )}
          </div>
        </Stack>
      </div>

      <div
        style={
          compact
            ? { position: 'absolute', bottom: 12, left: '50%', transform: 'translateX(-50%)', zIndex: 5 }
            : { position: 'absolute', bottom: 12, right: 12, zIndex: 5 }
        }
      >
        <Toolbar
          compact={compact}
          layers={drawn}
          onLayer={(layer) => setLayers((current) => ({ ...current, [layer]: !current[layer] }))}
          visibility={visibility}
          selecting={selecting}
          onSelecting={() => {
            setSelecting((current) => !current);
            setPicking(null);
          }}
          fullscreen={fullscreen}
          onFullscreen={toggleFullscreen}
          onHome={() => {
            // the shown world's spawn, in its first dimension
            const world = current?.world ?? overview.worlds[0];
            const home = world.level.spawn;
            goTo({ world: world.name, dimension: world.dimensions[0].id }, home ? home[0] : 0, home ? home[2] : 0);
          }}
          heatmapDays={overview.heatmap.retention_days}
          toolsOpen={toolsOpen}
          onTools={toggleTools}
          overlay={overlay}
          onOverlay={setOverlay}
        />
      </div>

      {(picking || (selecting && (compact || !toolsOpen))) && (
        <div
          style={
            compact
              ? { position: 'absolute', top: 64, left: 12, right: 12, zIndex: 5 }
              : {
                  position: 'absolute',
                  bottom: 68,
                  left: '50%',
                  transform: 'translateX(-50%)',
                  width: 'max-content',
                  maxWidth: '95%',
                  zIndex: 5,
                }
          }
        >
          <div className='wm-glass' style={{ padding: '8px 12px' }}>
            {picking ? (
              <Text size='sm'>{t('players.teleportHint', { name: playerLabel(picking) })}</Text>
            ) : (
              <Stack gap={6}>
                <Text size='sm' fw={isEmpty(selection) ? undefined : 600}>
                  {isEmpty(selection)
                    ? compact
                      ? t('selection.hintTouch', {})
                      : t('selection.hint', {})
                    : t('selection.count', { count: chunkCount })}
                </Text>
                <Group gap={6} wrap='wrap'>
                  {selectionControls}
                  {!isEmpty(selection) && (
                    <Button size='xs' variant='subtle' onClick={() => setSelection(emptySelection())}>
                      {t('selection.clear', {})}
                    </Button>
                  )}
                  <Button
                    size='xs'
                    variant='default'
                    leftSection={<FontAwesomeIcon icon={faToolbox} />}
                    onClick={toggleTools}
                  >
                    {t('tools.open', {})}
                  </Button>
                </Group>
              </Stack>
            )}
          </div>
        </div>
      )}

      <ChunkTools
        compact={compact}
        opened={toolsOpen}
        onClose={() => setToolsOpen(false)}
        controls={selectionControls}
        server={server}
        overview={overview}
        place={place}
        selection={selection}
        onSelection={setSelection}
        existing={existing}
        offline={power === 'offline'}
        jobs={jobs}
      />

      {menu && (
        <>
          <div style={{ position: 'absolute', inset: 0, zIndex: 6 }} onPointerDown={() => setMenu(null)} />
          <div
            className='wm-glass'
            style={
              compact
                ? { position: 'absolute', left: 12, right: 12, bottom: 124, zIndex: 7, padding: 4 }
                : {
                    position: 'absolute',
                    left: Math.max(8, Math.min(menu.left, size.width - 300)),
                    top: Math.max(8, Math.min(menu.top, size.height - 110)),
                    zIndex: 7,
                    padding: 4,
                    width: 'max-content',
                  }
            }
          >
            <Stack gap={2}>
              <Button
                size='sm'
                variant='subtle'
                justify='flex-start'
                leftSection={<FontAwesomeIcon icon={faCopy} />}
                onClick={() => {
                  copy(`${Math.floor(menu.x)} ${Math.floor(menu.z)}`);
                  setMenu(null);
                }}
              >
                {t('map.copyCoordinates', {})} ({Math.floor(menu.x)}, {Math.floor(menu.z)})
              </Button>
              {overview.capabilities.teleport && selected?.name && running && canTeleportHere && (
                <Button
                  size='sm'
                  variant='subtle'
                  justify='flex-start'
                  leftSection={<FontAwesomeIcon icon={faLocationArrow} />}
                  onClick={() => {
                    teleportHere(selected, menu.x, menu.z);
                    setMenu(null);
                  }}
                >
                  {t('map.teleportHere', { name: playerLabel(selected) })}
                </Button>
              )}
            </Stack>
          </div>
        </>
      )}

      <ConfirmationModal
        opened={confirmReset}
        onClose={() => setConfirmReset(false)}
        title={t('players.resetHeatmapTitle', {})}
        confirm={t('players.resetHeatmap', {})}
        onConfirmed={runReset}
      >
        <Text size='sm'>{t('players.resetHeatmapContent', {})}</Text>
      </ConfirmationModal>

      {overview.capabilities.manage_worlds && (
        <WorldsModal
          key={`folders-${foldersOpen}`}
          server={server}
          overview={overview}
          opened={foldersOpen}
          onClose={() => setFoldersOpen(false)}
        />
      )}
      {overview.capabilities.manage_players && (
        <PlayerVisibilityModal
          key={`visibility-${visibilityOpen}`}
          server={server}
          visibility={visibility}
          opened={visibilityOpen}
          onClose={() => setVisibilityOpen(false)}
        />
      )}
    </div>
  );
}
