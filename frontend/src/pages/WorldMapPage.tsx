import { faCopy, faLocationArrow, faLocationDot } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { useDebouncedValue } from '@mantine/hooks';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import Button from '@/elements/buttons/Button.tsx';
import ServerContentContainer from '@/elements/containers/ServerContentContainer.tsx';
import Alert from '@/elements/feedback/Alert.tsx';
import Spinner from '@/elements/feedback/Spinner.tsx';
import Group from '@/elements/layout/Group.tsx';
import Paper from '@/elements/layout/Paper.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import ConfirmationModal from '@/elements/modals/ConfirmationModal.tsx';
import { Modal } from '@/elements/modals/Modal.tsx';
import Text from '@/elements/typography/Text.tsx';
import { useToast } from '@/providers/ToastProvider.tsx';
import { useServerStore } from '@/stores/server.ts';
import {
  deleteChunks,
  getEntities,
  getHeatmap,
  getOnline,
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
import MapCanvas, { type Layers, type TileStatus } from '../components/MapCanvas.tsx';
import OverviewCard, { CARD_STYLE, dimensionLabel } from '../components/OverviewCard.tsx';
import PlayerCard, { avatarUrl, playerLabel } from '../components/PlayerCard.tsx';
import Toolbar from '../components/Toolbar.tsx';
import {
  type ChunkRect,
  chunkOf,
  regionOf,
  selectedChunks,
  selectionText,
  type Size,
  toScreen,
  type View,
  visibleBounds,
  visibleRegions,
} from '../lib/geometry.ts';
import { useExtTranslations } from '../translations.ts';

const DEFAULT_LAYERS: Layers = { players: true, markers: true, heatmap: false, mobs: true, entities: false };

interface Saved {
  place: Place;
  view: View;
  layers: Layers;
}

const storageKey = (server: string) => `${PACKAGE}:view:${server}`;

function loadSaved(server: string): Partial<Saved> {
  try {
    return JSON.parse(localStorage.getItem(storageKey(server)) ?? '{}');
  } catch {
    return {};
  }
}

/** The place to open: the last one if it still exists, else the main world's first dimension. */
function initialPlace(overview: Overview, saved: Partial<Saved>): Place | null {
  const world = overview.worlds.find((candidate) => candidate.name === saved.place?.world);
  if (world?.dimensions.some((dimension) => dimension.id === saved.place?.dimension)) return saved.place!;
  const main = overview.worlds[0];
  return main ? { world: main.name, dimension: main.dimensions[0].id } : null;
}

function useSize(ref: React.RefObject<HTMLDivElement | null>): Size {
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

export default function WorldMapPage() {
  const { t } = useExtTranslations();
  const server = useServerStore((state) => state.server.uuid);
  const overview = useQuery({ queryKey: [PACKAGE, server, 'overview'], queryFn: () => getOverview(server) });

  return (
    <ServerContentContainer title={t('title', {})} subtitle={t('subtitle', {})}>
      {overview.isLoading ? (
        <Spinner.Centered />
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
  const reason = overview.unsupported
    ? t(
        overview.unsupported === 'egg'
          ? 'unsupported.egg'
          : overview.unsupported === 'bedrock'
            ? 'unsupported.bedrock'
            : 'unsupported.notMinecraft',
        {},
      )
    : overview.too_old
      ? t('unsupported.tooOld', {})
      : overview.worlds.length === 0
        ? t('unsupported.noWorld', {})
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

function WorldMap({ server, overview }: { server: string; overview: Overview }) {
  const { t } = useExtTranslations();
  const { addToast } = useToast();
  const queryClient = useQueryClient();
  const power = useServerStore((state) => state.state);
  const running = power === 'running';
  const saved = useMemo(() => loadSaved(server), [server]);
  const spawn = overview.level.spawn;

  const [place, setPlace] = useState<Place>(() => initialPlace(overview, saved)!);
  const [view, setView] = useState<View>(
    () => saved.view ?? { x: spawn ? spawn[0] : 0, z: spawn ? spawn[2] : 0, zoom: 1 },
  );
  const [layers, setLayers] = useState<Layers>(() => ({ ...DEFAULT_LAYERS, ...saved.layers }));
  const [selecting, setSelecting] = useState(false);
  const [selection, setSelection] = useState<ChunkRect[]>([]);
  const [cursor, setCursor] = useState<[number, number] | null>(null);
  const [selectedUuid, setSelectedUuid] = useState<string | null>(null);
  const [picking, setPicking] = useState<Player | null>(null);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [tileStatus, setTileStatus] = useState<TileStatus>({ loading: 0, skipped: 0, error: null });
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [confirmReset, setConfirmReset] = useState(false);
  const [help, setHelp] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  const container = useRef<HTMLDivElement>(null);
  const size = useSize(container);

  useEffect(() => {
    localStorage.setItem(storageKey(server), JSON.stringify({ place, view, layers } satisfies Saved));
  }, [server, place, view, layers]);

  useEffect(() => {
    const onChange = () => setFullscreen(document.fullscreenElement === container.current);
    document.addEventListener('fullscreenchange', onChange);
    return () => document.removeEventListener('fullscreenchange', onChange);
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      setMenu(null);
      setPicking(null);
      setSelection([]);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const regions = useQuery({
    queryKey: [PACKAGE, server, 'regions', place.world, place.dimension],
    queryFn: () => getRegions(server, place),
    refetchInterval: 60_000,
  });
  const players = useQuery({
    queryKey: [PACKAGE, server, 'players'],
    queryFn: () => getPlayers(server),
    refetchInterval: 60_000,
  });
  const online = useQuery({
    queryKey: [PACKAGE, server, 'online'],
    queryFn: () => getOnline(server),
    enabled: overview.capabilities.teleport && running,
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

  const allPlayers = players.data ?? [];
  const selected = allPlayers.find((player) => player.uuid === selectedUuid) ?? null;
  const here = allPlayers.filter((player) => player.world === place.world && player.dimension === place.dimension);
  const mainOverworld = place.world === overview.worlds[0]?.name && place.dimension === 'minecraft:overworld';
  const chunkCount = useMemo(() => selectedChunks(selection), [selection]);

  const goTo = (target: Place, x: number, z: number, zoom?: number) => {
    setPlace({ world: target.world, dimension: target.dimension });
    setView((current) => ({ x, z, zoom: zoom ?? current.zoom }));
  };
  const changePlace = (target: Place) => {
    setPlace(target);
    setSelection([]);
  };
  const locate = (player: Player) => goTo(player, player.x, player.z, Math.max(view.zoom, 2));

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

  const toggleFullscreen = () => {
    if (document.fullscreenElement) document.exitFullscreen();
    else container.current?.requestFullscreen();
  };

  const runDelete = async () => {
    try {
      const result = await deleteChunks(server, place, selection);
      addToast(t('selection.deleted', { deleted: result.deleted, files: result.files }), 'success');
      setSelection([]);
      setConfirmDelete(false);
      queryClient.invalidateQueries({ queryKey: [PACKAGE, server, 'regions'] });
      queryClient.invalidateQueries({ queryKey: [PACKAGE, server, 'entities'] });
    } catch (error) {
      addToast(httpErrorToHuman(error), 'error');
    }
  };

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
  const tooManyChunks = chunkCount > overview.limits.delete_chunks;
  const notices = [
    tileStatus.loading > 0 ? t('map.loading', { count: tileStatus.loading }) : null,
    tileStatus.skipped > 0 ? t('map.zoomInTiles', {}) : null,
    tileStatus.error ? t('map.tileFailed', { error: tileStatus.error }) : null,
    (layers.mobs || layers.entities) && !entityArea ? t('map.zoomInEntities', {}) : null,
    entities.data?.truncated ? t('map.entitiesTruncated', { count: entities.data.entities.length }) : null,
  ].filter((notice): notice is string => !!notice);

  return (
    <div
      ref={container}
      style={{
        position: 'relative',
        height: fullscreen ? '100vh' : 'calc(100vh - 220px)',
        minHeight: 480,
        borderRadius: fullscreen ? 0 : 'var(--mantine-radius-md)',
        overflow: 'hidden',
        background: '#101113',
        userSelect: 'none',
      }}
    >
      <MapCanvas
        server={server}
        place={place}
        regions={regions.data ?? []}
        view={view}
        size={size}
        onView={setView}
        layers={layers}
        heat={layers.heatmap ? (heat.data ?? null) : null}
        entities={entityArea ? (entities.data?.entities ?? []) : []}
        selecting={selecting}
        selection={selection}
        onSelection={setSelection}
        picking={!!picking}
        onPick={(x, z) => {
          if (picking) teleportHere(picking, x, z);
          setPicking(null);
        }}
        onCursor={setCursor}
        onMenu={onMenu}
        onTiles={setTileStatus}
      />

      {layers.markers && mainOverworld && spawn && (
        <div
          title={t('map.spawn', {})}
          style={{
            position: 'absolute',
            left: marker(spawn[0], spawn[2])[0] - 9,
            top: marker(spawn[0], spawn[2])[1] - 22,
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

      {layers.players &&
        here.map((player) => {
          const [left, top] = marker(player.x, player.z);
          if (left < -40 || top < -40 || left > size.width + 40 || top > size.height + 40) return null;
          const isSelected = player.uuid === selectedUuid;
          return (
            <button
              key={player.uuid}
              type='button'
              title={`${playerLabel(player)} · X ${player.x.toFixed(1)} Z ${player.z.toFixed(1)}`}
              onClick={() => setSelectedUuid(player.uuid)}
              style={{
                position: 'absolute',
                left: left - 14,
                top: top - 38,
                width: 28,
                padding: 0,
                border: 'none',
                background: 'none',
                cursor: 'pointer',
                zIndex: isSelected ? 4 : 3,
              }}
            >
              <img
                src={avatarUrl(player.uuid, 48)}
                alt={playerLabel(player)}
                width={28}
                height={28}
                style={{
                  display: 'block',
                  borderRadius: 4,
                  border: `2px solid ${isSelected ? '#3b82f6' : '#fff'}`,
                  boxShadow: '0 1px 4px rgba(0,0,0,0.6)',
                  imageRendering: 'pixelated',
                }}
              />
              <div
                style={{
                  width: 0,
                  height: 0,
                  margin: '0 auto',
                  borderLeft: '5px solid transparent',
                  borderRight: '5px solid transparent',
                  borderTop: `8px solid ${isSelected ? '#3b82f6' : '#fff'}`,
                }}
              />
            </button>
          );
        })}

      <div style={{ position: 'absolute', top: 12, left: 12, zIndex: 5 }}>
        <OverviewCard
          overview={overview}
          place={place}
          onPlace={changePlace}
          onJump={(x, z) => setView((current) => ({ ...current, x, z }))}
        />
      </div>

      <div style={{ position: 'absolute', top: 12, right: 12, zIndex: 5 }}>
        <PlayerCard
          players={allPlayers}
          selected={selected}
          onSelect={setSelectedUuid}
          online={online.data?.online ?? null}
          onlineError={online.error ? httpErrorToHuman(online.error) : null}
          running={running}
          canTeleport={overview.capabilities.teleport}
          canResetHeatmap={overview.capabilities.reset_heatmap}
          onLocate={locate}
          onTeleportTo={(player, name) => runTeleport(player, { type: 'player', name })}
          onPick={(player) => {
            setPicking(player);
            setSelecting(false);
          }}
          onRefreshOnline={() => online.refetch()}
          onResetHeatmap={() => setConfirmReset(true)}
        />
      </div>

      <div style={{ position: 'absolute', bottom: 12, left: 12, zIndex: 5 }}>
        <Stack gap={6} align='flex-start'>
          {layers.heatmap && (
            <Paper withBorder radius='md' px='sm' py={6} style={CARD_STYLE}>
              <Text size='xs'>
                {!overview.heatmap.enabled
                  ? t('heatmap.disabled', {})
                  : heat.data && heat.data.max === 0
                    ? t('heatmap.empty', {})
                    : t('heatmap.legend', { days: heat.data?.days ?? overview.heatmap.retention_days })}
              </Text>
              <div
                style={{
                  marginTop: 4,
                  height: 6,
                  width: 180,
                  borderRadius: 3,
                  background: 'linear-gradient(90deg, #285aff, #28dc78, #fadc28, #f03228)',
                }}
              />
            </Paper>
          )}
          {notices.map((notice) => (
            <Paper key={notice} withBorder radius='md' px='sm' py={4} style={CARD_STYLE}>
              <Text size='xs'>{notice}</Text>
            </Paper>
          ))}
          <Paper withBorder radius='md' px='sm' py={6} style={{ ...CARD_STYLE, minWidth: 180 }}>
            <Text size='xs' fw={600}>
              {t('cursor.title', {})}
            </Text>
            {cursor ? (
              <>
                <Text size='xs' ff='monospace'>
                  X {Math.floor(cursor[0])} · Z {Math.floor(cursor[1])}
                </Text>
                <Text size='xs' c='dimmed' ff='monospace'>
                  {t('cursor.chunk', { x: chunkOf(cursor[0]), z: chunkOf(cursor[1]) })} ·{' '}
                  {t('cursor.region', { x: regionOf(cursor[0]), z: regionOf(cursor[1]) })}
                </Text>
              </>
            ) : (
              <Text size='xs' c='dimmed' ff='monospace'>
                –
              </Text>
            )}
          </Paper>
        </Stack>
      </div>

      <div style={{ position: 'absolute', bottom: 12, right: 12, zIndex: 5 }}>
        <Toolbar
          layers={layers}
          onLayer={(layer) => setLayers((current) => ({ ...current, [layer]: !current[layer] }))}
          selecting={selecting}
          onSelecting={() => {
            setSelecting((current) => !current);
            setPicking(null);
          }}
          fullscreen={fullscreen}
          onFullscreen={toggleFullscreen}
          onHome={() =>
            goTo(
              { world: overview.worlds[0].name, dimension: overview.worlds[0].dimensions[0].id },
              spawn ? spawn[0] : 0,
              spawn ? spawn[2] : 0,
            )
          }
          onHelp={() => setHelp(true)}
        />
      </div>

      {(selecting || picking) && (
        <div
          style={{
            position: 'absolute',
            bottom: 72,
            left: '50%',
            transform: 'translateX(-50%)',
            width: 'max-content',
            maxWidth: '95%',
            zIndex: 5,
          }}
        >
          <Paper withBorder radius='md' px='sm' py={8} style={CARD_STYLE}>
            {picking ? (
              <Text size='sm'>{t('players.teleportHint', { name: playerLabel(picking) })}</Text>
            ) : selection.length === 0 ? (
              <Text size='sm'>{t('selection.hint', {})}</Text>
            ) : (
              <Group gap='sm' wrap='nowrap'>
                <Text size='sm' fw={500}>
                  {t('selection.count', { count: chunkCount })}
                </Text>
                <Button
                  size='xs'
                  variant='default'
                  leftSection={<FontAwesomeIcon icon={faCopy} />}
                  onClick={() => copy(selectionText(selection))}
                >
                  {t('selection.copy', {})}
                </Button>
                {overview.capabilities.delete_chunks && (
                  <Button
                    size='xs'
                    color='red'
                    disabled={power !== 'offline' || tooManyChunks}
                    onClick={() => setConfirmDelete(true)}
                  >
                    {t('selection.delete', {})}
                  </Button>
                )}
                <Button size='xs' variant='subtle' onClick={() => setSelection([])}>
                  {t('selection.clear', {})}
                </Button>
                {overview.capabilities.delete_chunks && (power !== 'offline' || tooManyChunks) && (
                  <Text size='xs' c='dimmed'>
                    {power !== 'offline'
                      ? t('selection.deleteOnline', {})
                      : t('selection.tooMany', { limit: overview.limits.delete_chunks })}
                  </Text>
                )}
              </Group>
            )}
          </Paper>
        </div>
      )}

      {menu && (
        <>
          <div style={{ position: 'absolute', inset: 0, zIndex: 6 }} onPointerDown={() => setMenu(null)} />
          <Paper
            withBorder
            radius='md'
            p={4}
            style={{ ...CARD_STYLE, position: 'absolute', left: menu.left, top: menu.top, zIndex: 7 }}
          >
            <Stack gap={2}>
              <Button
                size='xs'
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
              {overview.capabilities.teleport && selected?.name && running && (
                <Button
                  size='xs'
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
          </Paper>
        </>
      )}

      <ConfirmationModal
        opened={confirmDelete}
        onClose={() => setConfirmDelete(false)}
        title={t('selection.deleteTitle', {})}
        confirm={t('selection.delete', {})}
        onConfirmed={runDelete}
      >
        <Stack gap='sm'>
          <Text size='sm'>
            {t('selection.deleteContent', { count: chunkCount, dimension: dimensionLabel(t, place.dimension) })}
          </Text>
          <Alert color='yellow'>{t('selection.backup', {})}</Alert>
        </Stack>
      </ConfirmationModal>

      <ConfirmationModal
        opened={confirmReset}
        onClose={() => setConfirmReset(false)}
        title={t('players.resetHeatmapTitle', {})}
        confirm={t('players.resetHeatmap', {})}
        onConfirmed={runReset}
      >
        <Text size='sm'>{t('players.resetHeatmapContent', {})}</Text>
      </ConfirmationModal>

      <Modal opened={help} onClose={() => setHelp(false)} title={t('help.title', {})}>
        <Stack gap='xs'>
          {(['move', 'layers', 'entities', 'heatmap', 'select', 'menu', 'data'] as const).map((key) => (
            <Text key={key} size='sm'>
              {t(`help.${key}`, {})}
            </Text>
          ))}
        </Stack>
      </Modal>
    </div>
  );
}
