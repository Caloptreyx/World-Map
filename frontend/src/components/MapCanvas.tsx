import { type PointerEvent as ReactPointerEvent, useEffect, useMemo, useRef, useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import { type Entity, getTile, type Heatmap, type Place, type RegionFile } from '../api.ts';
import { entityIcon, entityLayer, entityName, mobKind } from '../lib/entities.ts';
import {
  CHUNK_BLOCKS,
  type ChunkRect,
  type Cluster,
  chunkOf,
  chunkRect,
  cluster,
  clusterCell,
  heatColour,
  MAX_TILE_REQUESTS,
  pinchView,
  REGION_BLOCKS,
  type Size,
  toggleChunk,
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
const ICON_SIZE = 20;
const DRAG_THRESHOLD = 4;
/** Finger movement still counted as a tap. */
const TOUCH_SLOP = 10;
const LONG_PRESS_MS = 450;

/** Radius of the disc behind an entity group: grows slowly with its size. */
const groupRadius = (count: number) => (count > 1 ? 13 + Math.min(7, Math.log2(count) * 2) : ICON_SIZE / 2);

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
  /** A tap on a touch screen (outside selection and picking): the touch stand-in for the cursor and menu. */
  onTap: (x: number, z: number, clientX: number, clientY: number) => void;
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
  touch: boolean;
}

export default function MapCanvas(props: Props) {
  const { server, place, regions, view, size, layers, heat, entities, selection } = props;
  const canvas = useRef<HTMLCanvasElement>(null);
  const tiles = useRef(new Map<string, Tile>());
  const inFlight = useRef(0);
  const aborter = useRef(new AbortController());
  const drag = useRef<Drag | null>(null);
  const pointers = useRef(new Map<number, [number, number]>());
  const pinch = useRef<{ view: View; from: [[number, number], [number, number]] } | null>(null);
  const press = useRef<number | null>(null);
  const latest = useRef(props);
  latest.current = props;
  const [version, setVersion] = useState(0);
  const [preview, setPreview] = useState<ChunkRect | null>(null);
  const [hovered, setHovered] = useState<Cluster<Entity> | null>(null);
  const [pulse, setPulse] = useState(0);

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
    return cluster(shown, view, size, clusterCell(view.zoom));
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
    let pulseTimer: number | undefined;
    const frame = requestAnimationFrame(() => {
      ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
      ctx.clearRect(0, 0, size.width, size.height);
      ctx.imageSmoothingEnabled = view.zoom < 1;

      const regionSize = REGION_BLOCKS * view.zoom;
      let pending = false;
      for (const [x, z] of visibleRegions(view, size)) {
        const region = byPosition.get(`${x},${z}`);
        if (!region) continue;
        const [sx, sy] = toScreen(view, size, x * REGION_BLOCKS, z * REGION_BLOCKS);
        const tile = tiles.current.get(tileKey(region));
        if (tile?.state === 'ready') {
          ctx.drawImage(tile.bitmap, sx, sy, regionSize, regionSize);
        } else if (tile?.state === 'error') {
          ctx.fillStyle = 'rgba(239, 68, 68, 0.1)';
          ctx.fillRect(sx + 1, sy + 1, regionSize - 2, regionSize - 2);
          ctx.strokeStyle = 'rgba(239, 68, 68, 0.5)';
          ctx.setLineDash([6, 6]);
          ctx.strokeRect(sx + 1.5, sy + 1.5, regionSize - 3, regionSize - 3);
          ctx.setLineDash([]);
          if (regionSize >= 48) {
            ctx.fillStyle = 'rgba(239, 68, 68, 0.85)';
            ctx.font = '600 18px sans-serif';
            ctx.textAlign = 'center';
            ctx.textBaseline = 'middle';
            ctx.fillText('!', sx + regionSize / 2, sy + regionSize / 2);
            ctx.textAlign = 'start';
          }
        } else {
          // a soft pulse while the region renders (redrawn through `pulse`)
          pending = true;
          const glow = 0.05 + 0.035 * Math.sin(Date.now() / 260);
          ctx.fillStyle = `rgba(255, 255, 255, ${glow.toFixed(3)})`;
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
        const count = group.members.length;
        const id = mostCommon(group.members);
        const url = entityIcon(id);
        const icon = url ? icons.get(url) : 'failed';
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
        const radius = groupRadius(count);
        if (count > 1) {
          ctx.beginPath();
          ctx.arc(group.sx, group.sy, radius, 0, Math.PI * 2);
          ctx.fillStyle = 'rgba(17, 20, 26, 0.72)';
          ctx.fill();
          ctx.strokeStyle = 'rgba(255, 255, 255, 0.28)';
          ctx.lineWidth = 1;
          ctx.stroke();
        }
        const size = count > 1 ? 16 : ICON_SIZE;
        if (icon instanceof HTMLImageElement && icon.complete && icon.naturalWidth > 0) {
          ctx.drawImage(icon, group.sx - size / 2, group.sy - size / 2, size, size);
        } else {
          ctx.beginPath();
          ctx.arc(group.sx, group.sy, 5, 0, Math.PI * 2);
          ctx.fillStyle =
            entityLayer(id) === 'entities' ? '#a1a1aa' : mobKind(id) === 'hostile' ? '#ef4444' : '#22c55e';
          ctx.fill();
          ctx.strokeStyle = 'rgba(0, 0, 0, 0.6)';
          ctx.lineWidth = 1;
          ctx.stroke();
        }
        if (count > 1) {
          const label = count > 999 ? '999+' : String(count);
          ctx.font = '600 10px sans-serif';
          const width = Math.max(16, ctx.measureText(label).width + 8);
          const left = group.sx + radius * 0.7 - width / 2;
          const top = group.sy + radius * 0.7 - 8;
          ctx.fillStyle = '#f8fafc';
          ctx.beginPath();
          ctx.roundRect(left, top, width, 16, 8);
          ctx.fill();
          ctx.fillStyle = '#111418';
          ctx.textAlign = 'center';
          ctx.textBaseline = 'middle';
          ctx.fillText(label, left + width / 2, top + 8.5);
          ctx.textAlign = 'start';
        }
      }
      if (pending) pulseTimer = window.setTimeout(() => setPulse((p) => p + 1), 120);
    });
    return () => {
      cancelAnimationFrame(frame);
      clearTimeout(pulseTimer);
    };
  }, [view, size, version, pulse, byPosition, layers.heatmap, heat, selection, preview, clusters, props.selecting]);

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

  const cancelPress = () => {
    if (press.current !== null) {
      clearTimeout(press.current);
      press.current = null;
    }
  };

  const onPointerDown = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const touch = event.pointerType !== 'mouse';
    // the middle button always pans, so the map can move while selecting chunks
    if (!touch && event.button !== 0 && event.button !== 1) return;
    const [sx, sy] = local(event);
    event.currentTarget.setPointerCapture(event.pointerId);
    pointers.current.set(event.pointerId, [sx, sy]);
    if (pointers.current.size === 2) {
      // a second finger: pinch instead of whatever the first one started
      cancelPress();
      drag.current = null;
      setPreview(null);
      const [a, b] = [...pointers.current.values()];
      pinch.current = { view, from: [a, b] };
      return;
    }
    if (pointers.current.size > 2) return;
    drag.current = {
      pointer: event.pointerId,
      startX: sx,
      startY: sy,
      view,
      mode: props.selecting && !touch && event.button === 0 ? 'select' : 'pan',
      moved: false,
      additive: event.shiftKey || touch,
      touch,
    };
    if (touch && props.selecting) {
      // touch: a long press starts a rectangle, a plain drag still pans
      press.current = window.setTimeout(() => {
        press.current = null;
        const current = drag.current;
        if (!current || current.moved) return;
        current.mode = 'select';
        const [x, z] = toWorld(current.view, latest.current.size, current.startX, current.startY);
        setPreview(chunkRect(x, z, x, z));
        navigator.vibrate?.(12);
      }, LONG_PRESS_MS);
    }
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const [sx, sy] = local(event);
    if (pointers.current.has(event.pointerId)) pointers.current.set(event.pointerId, [sx, sy]);
    if (pinch.current) {
      if (pointers.current.size === 2) {
        const [a, b] = [...pointers.current.values()];
        props.onView(pinchView(pinch.current.view, size, pinch.current.from, [a, b]));
      }
      return;
    }
    const [wx, wz] = toWorld(view, size, sx, sy);
    const current = drag.current;
    if (event.pointerType === 'mouse') {
      props.onCursor([wx, wz]);
      if (!current) {
        const reach = groupRadius(1) + 4;
        setHovered(clusters.find((group) => Math.hypot(group.sx - sx, group.sy - sy) <= reach) ?? null);
      }
    }
    if (!current || current.pointer !== event.pointerId) return;
    if (Math.hypot(sx - current.startX, sy - current.startY) > (current.touch ? TOUCH_SLOP : DRAG_THRESHOLD)) {
      current.moved = true;
      if (current.mode === 'pan') cancelPress();
    }
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
    pointers.current.delete(event.pointerId);
    cancelPress();
    if (pinch.current) {
      // the fingers left after a pinch do nothing until all are lifted
      if (pointers.current.size === 0) pinch.current = null;
      return;
    }
    const current = drag.current;
    if (!current || current.pointer !== event.pointerId) return;
    drag.current = null;
    const [sx, sy] = local(event);
    const [wx, wz] = toWorld(view, size, sx, sy);
    if (current.mode === 'select') {
      setPreview(null);
      if (current.moved || current.touch) {
        const [x1, z1] = toWorld(view, size, current.startX, current.startY);
        const rect = chunkRect(x1, z1, wx, wz);
        props.onSelection(current.additive ? [...selection, rect] : [rect]);
      } else {
        props.onSelection(toggleChunk(selection, chunkOf(wx), chunkOf(wz)));
      }
    } else if (!current.moved) {
      if (props.picking) props.onPick(wx, wz);
      else if (props.selecting) props.onSelection(toggleChunk(selection, chunkOf(wx), chunkOf(wz)));
      else if (current.touch) props.onTap(wx, wz, event.clientX, event.clientY);
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
        onPointerCancel={(event) => {
          pointers.current.delete(event.pointerId);
          if (pointers.current.size === 0) pinch.current = null;
          cancelPress();
          drag.current = null;
          setPreview(null);
        }}
        onPointerLeave={(event) => {
          // touch pointers leave after every tap: the tapped position stays in the readout
          if (event.pointerType !== 'mouse') return;
          props.onCursor(null);
          setHovered(null);
        }}
        onDoubleClick={(event) => {
          const [sx, sy] = local(event);
          props.onView(zoomAt(view, size, sx, sy, 2));
        }}
        onContextMenu={(event) => {
          event.preventDefault();
          // touch screens send this for a long press; taps open the readout there instead
          if ((event.nativeEvent as PointerEvent).pointerType === 'touch') return;
          const [sx, sy] = local(event);
          const [wx, wz] = toWorld(view, size, sx, sy);
          props.onMenu(wx, wz, event.clientX, event.clientY);
        }}
      />
      {hovered && (
        <div className='wm-glass wm-tooltip' style={{ left: hovered.sx + 16, top: hovered.sy + 16 }}>
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

/** The entity id a group shows: the most frequent one. */
function mostCommon(members: Entity[]): string {
  const counts = new Map<string, number>();
  let best = members[0].id;
  for (const entity of members) {
    const count = (counts.get(entity.id) ?? 0) + 1;
    counts.set(entity.id, count);
    if (count > (counts.get(best) ?? 0)) best = entity.id;
  }
  return best;
}
