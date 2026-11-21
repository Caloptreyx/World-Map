import { type PointerEvent as ReactPointerEvent, useEffect, useMemo, useRef, useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import { type Entity, getTile, type Heatmap, type Place, type RegionFile } from '../api.ts';
import { entityIcon, entityLayer, entityName, mobKind } from '../lib/entities.ts';
import {
  CHUNK_BLOCKS,
  type ChunkRect,
  type Cluster,
  chunkRect,
  cluster,
  heatColour,
  MAX_TILE_REQUESTS,
  REGION_BLOCKS,
  type Size,
  toScreen,
  toWorld,
  type View,
  visibleRegions,
  zoomAt,
} from '../lib/geometry.ts';

export interface Layers {
  players: boolean;
  markers: boolean;
  heatmap: boolean;
  mobs: boolean;
  entities: boolean;
}

export interface TileStatus {
  /** Regions on screen still being rendered. */
  loading: number;
  /** Regions on screen not requested because too many are visible. */
  skipped: number;
  error: string | null;
}

type Tile = { state: 'loading' } | { state: 'ready'; bitmap: ImageBitmap } | { state: 'error'; error: string };

/** Tile requests at the same time (the backend renders a few regions at once anyway). */
const PARALLEL_TILES = 4;
/** Tiles kept in memory beyond the ones on screen. */
const KEPT_TILES = 300;
const CLUSTER_CELL = 28;
const ICON_SIZE = 20;
const DRAG_THRESHOLD = 4;

const icons = new Map<string, HTMLImageElement | 'failed'>();

interface Props {
  server: string;
  place: Place;
  regions: RegionFile[];
  view: View;
  size: Size;
  onView: (view: View) => void;
  layers: Layers;
  heat: Heatmap | null;
  entities: Entity[];
  selecting: boolean;
  selection: ChunkRect[];
  onSelection: (rects: ChunkRect[]) => void;
  picking: boolean;
  onPick: (x: number, z: number) => void;
  onCursor: (point: [number, number] | null) => void;
  onMenu: (x: number, z: number, clientX: number, clientY: number) => void;
  onTiles: (status: TileStatus) => void;
}

interface Drag {
  pointer: number;
  startX: number;
  startY: number;
  view: View;
  mode: 'pan' | 'select';
  moved: boolean;
  additive: boolean;
}

export default function MapCanvas(props: Props) {
  const { server, place, regions, view, size, layers, heat, entities, selection } = props;
  const canvas = useRef<HTMLCanvasElement>(null);
  const tiles = useRef(new Map<string, Tile>());
  const inFlight = useRef(0);
  const aborter = useRef(new AbortController());
  const drag = useRef<Drag | null>(null);
  const latest = useRef(props);
  latest.current = props;
  const [version, setVersion] = useState(0);
  const [preview, setPreview] = useState<ChunkRect | null>(null);
  const [hovered, setHovered] = useState<Cluster<Entity> | null>(null);

  const byPosition = useMemo(() => new Map(regions.map((region) => [`${region.x},${region.z}`, region])), [regions]);
  const tileKey = (region: RegionFile) =>
    `${place.world}|${place.dimension}|${region.x}|${region.z}|${region.modified}|${region.size}`;

  // a new world or dimension: drop everything in flight
  useEffect(() => {
    aborter.current.abort();
    aborter.current = new AbortController();
    inFlight.current = 0;
    for (const tile of tiles.current.values()) {
      if (tile.state === 'ready') tile.bitmap.close();
    }
    tiles.current.clear();
    setVersion((v) => v + 1);
  }, [server, place.world, place.dimension]);

  useEffect(() => () => aborter.current.abort(), []);

  // request missing tiles on screen, nearest first
  useEffect(() => {
    if (size.width === 0) return;
    const visible = visibleRegions(view, size)
      .map(([x, z]) => byPosition.get(`${x},${z}`))
      .filter((region): region is RegionFile => !!region);
    const tooMany = visible.length > MAX_TILE_REQUESTS;
    let loading = 0;
    let skipped = 0;
    let error: string | null = null;
    for (const region of visible) {
      const key = tileKey(region);
      const tile = tiles.current.get(key);
      if (tile?.state === 'ready') continue;
      if (tile?.state === 'error') {
        error = tile.error;
        continue;
      }
      if (tile?.state === 'loading') {
        loading++;
        continue;
      }
      if (tooMany) {
        skipped++;
        continue;
      }
      loading++;
      if (inFlight.current >= PARALLEL_TILES) continue;
      inFlight.current++;
      tiles.current.set(key, { state: 'loading' });
      const signal = aborter.current.signal;
      getTile(server, place, region, signal)
        .then((blob) => createImageBitmap(blob))
        .then((bitmap) => {
          if (signal.aborted) {
            bitmap.close();
            return;
          }
          tiles.current.set(key, { state: 'ready', bitmap });
        })
        .catch((err) => {
          if (signal.aborted) return;
          tiles.current.set(key, { state: 'error', error: httpErrorToHuman(err) });
        })
        .finally(() => {
          if (signal.aborted) return;
          inFlight.current--;
          setVersion((v) => v + 1);
        });
    }
    props.onTiles({ loading, skipped, error });

    // forget tiles far away once there are many
    if (tiles.current.size > KEPT_TILES + visible.length) {
      const keep = new Set(visible.map(tileKey));
      for (const [key, tile] of tiles.current) {
        if (keep.has(key) || tile.state === 'loading') continue;
        if (tile.state === 'ready') tile.bitmap.close();
        tiles.current.delete(key);
      }
    }
  }, [view, size, byPosition, version]);

  const clusters = useMemo(() => {
    const shown = entities.filter((entity) => layers[entityLayer(entity.id)]);
    return cluster(shown, view, size, CLUSTER_CELL);
  }, [entities, layers.mobs, layers.entities, view, size]);

  // draw
  useEffect(() => {
    const element = canvas.current;
    if (!element || size.width === 0) return;
    const ratio = window.devicePixelRatio || 1;
    if (element.width !== Math.round(size.width * ratio) || element.height !== Math.round(size.height * ratio)) {
      element.width = Math.round(size.width * ratio);
      element.height = Math.round(size.height * ratio);
    }
    const ctx = element.getContext('2d');
    if (!ctx) return;
    const frame = requestAnimationFrame(() => {
      ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
      ctx.clearRect(0, 0, size.width, size.height);
      ctx.imageSmoothingEnabled = view.zoom < 1;

      const regionSize = REGION_BLOCKS * view.zoom;
      for (const [x, z] of visibleRegions(view, size)) {
        const region = byPosition.get(`${x},${z}`);
        if (!region) continue;
        const [sx, sy] = toScreen(view, size, x * REGION_BLOCKS, z * REGION_BLOCKS);
        const tile = tiles.current.get(tileKey(region));
        if (tile?.state === 'ready') {
          ctx.drawImage(tile.bitmap, sx, sy, regionSize, regionSize);
        } else {
          ctx.fillStyle = tile?.state === 'error' ? 'rgba(239, 68, 68, 0.12)' : 'rgba(255, 255, 255, 0.04)';
          ctx.fillRect(sx + 1, sy + 1, regionSize - 2, regionSize - 2);
        }
      }

      const chunkSize = CHUNK_BLOCKS * view.zoom;
      if (layers.heatmap && heat && heat.max > 0) {
        ctx.save();
        ctx.filter = `blur(${Math.max(2, Math.min(24, chunkSize / 2))}px)`;
        for (const [cx, cz, count] of heat.cells) {
          const [sx, sy] = toScreen(view, size, cx * CHUNK_BLOCKS, cz * CHUNK_BLOCKS);
          if (sx < -chunkSize * 2 || sy < -chunkSize * 2 || sx > size.width || sy > size.height) continue;
          ctx.fillStyle = heatColour(count, heat.max);
          ctx.fillRect(sx, sy, chunkSize, chunkSize);
        }
        ctx.restore();
      }

      if (props.selecting && chunkSize >= 6) {
        ctx.strokeStyle = 'rgba(0, 0, 0, 0.25)';
        ctx.lineWidth = 1;
        ctx.beginPath();
        const [originX, originY] = toScreen(view, size, 0, 0);
        const startX = ((originX % chunkSize) + chunkSize) % chunkSize;
        const startY = ((originY % chunkSize) + chunkSize) % chunkSize;
        for (let x = startX; x < size.width; x += chunkSize) {
          ctx.moveTo(Math.round(x) + 0.5, 0);
          ctx.lineTo(Math.round(x) + 0.5, size.height);
        }
        for (let y = startY; y < size.height; y += chunkSize) {
          ctx.moveTo(0, Math.round(y) + 0.5);
          ctx.lineTo(size.width, Math.round(y) + 0.5);
        }
        ctx.stroke();
      }

      const drawRect = (rect: ChunkRect, fill: string, stroke: string) => {
        const [sx, sy] = toScreen(view, size, rect.x1 * CHUNK_BLOCKS, rect.z1 * CHUNK_BLOCKS);
        const w = (rect.x2 - rect.x1 + 1) * chunkSize;
        const h = (rect.z2 - rect.z1 + 1) * chunkSize;
        ctx.fillStyle = fill;
        ctx.fillRect(sx, sy, w, h);
        ctx.strokeStyle = stroke;
        ctx.lineWidth = 1.5;
        ctx.strokeRect(sx, sy, w, h);
      };
      for (const rect of selection) drawRect(rect, 'rgba(239, 68, 68, 0.25)', 'rgba(239, 68, 68, 0.9)');
      if (preview) drawRect(preview, 'rgba(59, 130, 246, 0.2)', 'rgba(59, 130, 246, 0.9)');

      for (const group of clusters) {
        const first = group.members[0];
        const url = entityIcon(first.id);
        const icon = url ? icons.get(url) : 'failed';
        const half = ICON_SIZE / 2;
        if (url && icon === undefined) {
          const image = new Image();
          icons.set(url, image);
          image.onload = () => setVersion((v) => v + 1);
          image.onerror = () => {
            icons.set(url, 'failed');
            setVersion((v) => v + 1);
          };
          image.src = url;
        }
        if (icon instanceof HTMLImageElement && icon.complete && icon.naturalWidth > 0) {
          ctx.drawImage(icon, group.sx - half, group.sy - half, ICON_SIZE, ICON_SIZE);
        } else {
          ctx.beginPath();
          ctx.arc(group.sx, group.sy, 5, 0, Math.PI * 2);
          ctx.fillStyle =
            entityLayer(first.id) === 'entities' ? '#a1a1aa' : mobKind(first.id) === 'hostile' ? '#ef4444' : '#22c55e';
          ctx.fill();
          ctx.strokeStyle = 'rgba(0, 0, 0, 0.6)';
          ctx.lineWidth = 1;
          ctx.stroke();
        }
        if (group.members.length > 1) {
          const label = String(group.members.length);
          ctx.font = '600 10px sans-serif';
          const width = Math.max(14, ctx.measureText(label).width + 6);
          ctx.fillStyle = 'rgba(17, 24, 39, 0.9)';
          ctx.beginPath();
          ctx.roundRect(group.sx + half - 6, group.sy - half - 6, width, 14, 7);
          ctx.fill();
          ctx.fillStyle = '#fff';
          ctx.textBaseline = 'middle';
          ctx.fillText(label, group.sx + half - 3, group.sy - half + 1);
        }
      }
    });
    return () => cancelAnimationFrame(frame);
  }, [view, size, version, byPosition, layers.heatmap, heat, selection, preview, clusters, props.selecting]);

  // the wheel zooms around the cursor (a passive React listener could not stop the page from scrolling)
  useEffect(() => {
    const element = canvas.current;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const rect = element.getBoundingClientRect();
      const { view: current, size: currentSize, onView } = latest.current;
      onView(
        zoomAt(
          current,
          currentSize,
          event.clientX - rect.left,
          event.clientY - rect.top,
          1.15 ** (-event.deltaY / 100),
        ),
      );
    };
    element.addEventListener('wheel', onWheel, { passive: false });
    return () => element.removeEventListener('wheel', onWheel);
  }, []);

  const local = (event: ReactPointerEvent | React.MouseEvent): [number, number] => {
    const rect = canvas.current!.getBoundingClientRect();
    return [event.clientX - rect.left, event.clientY - rect.top];
  };

  const onPointerDown = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    // the middle button always pans, so the map can move while selecting chunks
    if (event.button !== 0 && event.button !== 1) return;
    const [sx, sy] = local(event);
    event.currentTarget.setPointerCapture(event.pointerId);
    drag.current = {
      pointer: event.pointerId,
      startX: sx,
      startY: sy,
      view,
      mode: props.selecting && event.button === 0 ? 'select' : 'pan',
      moved: false,
      additive: event.shiftKey,
    };
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const [sx, sy] = local(event);
    const [wx, wz] = toWorld(view, size, sx, sy);
    props.onCursor([wx, wz]);
    const current = drag.current;
    if (!current || current.pointer !== event.pointerId) {
      const near = clusters.find((group) => Math.hypot(group.sx - sx, group.sy - sy) <= ICON_SIZE / 2 + 2);
      setHovered(near ?? null);
      return;
    }
    if (Math.hypot(sx - current.startX, sy - current.startY) > DRAG_THRESHOLD) current.moved = true;
    if (!current.moved) return;
    if (current.mode === 'pan') {
      props.onView({
        ...current.view,
        x: current.view.x - (sx - current.startX) / current.view.zoom,
        z: current.view.z - (sy - current.startY) / current.view.zoom,
      });
    } else {
      const [x1, z1] = toWorld(view, size, current.startX, current.startY);
      setPreview(chunkRect(x1, z1, wx, wz));
    }
  };

  const onPointerUp = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const current = drag.current;
    if (!current || current.pointer !== event.pointerId) return;
    drag.current = null;
    const [sx, sy] = local(event);
    const [wx, wz] = toWorld(view, size, sx, sy);
    if (current.mode === 'select') {
      const [x1, z1] = current.moved ? toWorld(view, size, current.startX, current.startY) : [wx, wz];
      const rect = chunkRect(x1, z1, wx, wz);
      setPreview(null);
      props.onSelection(current.additive ? [...selection, rect] : [rect]);
    } else if (!current.moved && props.picking) {
      props.onPick(wx, wz);
    }
  };

  return (
    <>
      <canvas
        ref={canvas}
        style={{
          position: 'absolute',
          inset: 0,
          width: '100%',
          height: '100%',
          cursor: props.selecting || props.picking ? 'crosshair' : drag.current ? 'grabbing' : 'grab',
          touchAction: 'none',
        }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={() => {
          drag.current = null;
          setPreview(null);
        }}
        onPointerLeave={() => {
          props.onCursor(null);
          setHovered(null);
        }}
        onDoubleClick={(event) => {
          const [sx, sy] = local(event);
          props.onView(zoomAt(view, size, sx, sy, 2));
        }}
        onContextMenu={(event) => {
          event.preventDefault();
          const [sx, sy] = local(event);
          const [wx, wz] = toWorld(view, size, sx, sy);
          props.onMenu(wx, wz, event.clientX, event.clientY);
        }}
      />
      {hovered && (
        <div
          style={{
            position: 'absolute',
            left: hovered.sx + 14,
            top: hovered.sy + 14,
            pointerEvents: 'none',
            background: 'rgba(17, 24, 39, 0.92)',
            color: '#fff',
            borderRadius: 6,
            padding: '4px 8px',
            fontSize: 12,
            maxWidth: 260,
            zIndex: 3,
          }}
        >
          {summarize(hovered.members).map(([label, count]) => (
            <div key={label}>
              {count > 1 ? `${count} × ` : ''}
              {label}
            </div>
          ))}
        </div>
      )}
    </>
  );
}

/** Entity counts by name (custom names shown as such), the most common first, at most 8 lines. */
function summarize(members: Entity[]): [string, number][] {
  const counts = new Map<string, number>();
  for (const entity of members) {
    const label = entity.name ? `${entity.name} (${entityName(entity.id)})` : entityName(entity.id);
    counts.set(label, (counts.get(label) ?? 0) + 1);
  }
  return [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 8);
}
