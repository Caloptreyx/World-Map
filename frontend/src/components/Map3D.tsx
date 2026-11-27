import { faArrowRotateLeft, faLocationDot } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { useMediaQuery } from '@mantine/hooks';
import { type PointerEvent as ReactPointerEvent, useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { httpErrorToHuman } from '@/api/axios.ts';
import SegmentedControl from '@/elements/layout/SegmentedControl.tsx';
import Tooltip from '@/elements/overlays/Tooltip.tsx';
import Text from '@/elements/typography/Text.tsx';
import type { Cluster, Size, View } from '../lib/geometry.ts';
import { type Entity, getBlock, getSurface, type Online, type Place, type Player, type RegionFile } from '../api.ts';
import {
  type Camera3D,
  clampCamera,
  compassDegrees,
  DEFAULT_DISTANCE,
  DEFAULT_PITCH,
  DEFAULT_YAW,
  flyLook,
  flyMove,
  flyTo,
  from2D,
  type Mode,
  moveMap,
  orbit,
  panMap,
  to2D,
  toFly,
  toMap,
  zoomMap,
} from '../lib/camera3d.ts';
import { entityIcon, entityLayer, mobKind } from '../lib/entities.ts';
import { isOnline } from '../lib/players.ts';
import { type Atmosphere, Terrain3D, type TerrainStatus } from '../lib/terrain3d.ts';
import { useExtTranslations } from '../translations.ts';
import BlockCard, { type BlockCardState } from './BlockCard.tsx';
import { groupRadius, ICON_SIZE, type Layers, mostCommon, summarize } from './MapCanvas.tsx';
import { avatarUrl, playerLabel } from './PlayerCard.tsx';

/** A request from the page to move the camera (locate a player, jump to coordinates, go home). */
export interface GotoRequest {
  x: number;
  /** Null when unknown: the camera settles on the terrain once it has loaded there. */
  y: number | null;
  z: number;
  /** A new number for every request, so the same spot can be asked for twice. */
  nonce: number;
}

/** Why the 3D view cannot be shown: no WebGL2, the engine failed to start, or the GPU context was lost. */
export type Unavailable = { kind: 'unsupported' } | { kind: 'failed'; error: string } | { kind: 'lost' };

interface Props {
  server: string;
  place: Place;
  regions: RegionFile[];
  atmosphere: Atmosphere;
  size: Size;
  /** The 2D view when the 3D view opens: its centre becomes the camera target. Read once. */
  initialView: View;
  /** A guess of the ground height until the terrain under the camera has loaded. */
  groundY: number;
  /** Where Reset goes (spawn or the origin); y is null when unknown. */
  home: [number, number | null, number];
  goto: GotoRequest | null;
  /** The camera moved: the ground point at the screen centre as a 2D view (debounced). */
  onView: (view: View) => void;
  /** The layers as drawn. */
  layers: Layers;
  /** Players in this world and dimension. */
  players: Player[];
  onlineNow: Online | null;
  selectedUuid: string | null;
  onSelectPlayer: (uuid: string | null) => void;
  /** Null when the spawn marker is off or does not belong to this dimension. */
  spawn: [number, number, number] | null;
  entities: Entity[];
  /** Heatmap/overlay colours per region `rx,rz`: 32×32 RGBA, alpha = strength. */
  tints: Map<string, Uint8Array> | null;
  /** The selected player may be teleported (permission, server running, a known dimension key). */
  canTeleport: boolean;
  selected: Player | null;
  onTeleport: (player: Player, x: number, y: number, z: number) => void;
  /** Picking a spot for a teleport: the next clicked block is the target. */
  picking: boolean;
  onPick: (x: number, y: number, z: number) => void;
  onCopy: (text: string) => void;
  onCursor: (point: [number, number] | null) => void;
  onStatus: (status: TerrainStatus) => void;
  onUnavailable: (reason: Unavailable) => void;
  compact: boolean;
  /** Where the compass, Map | Fly and Reset go (below the world overview, or the pills on phones). */
  controlsTarget: HTMLElement | null;
  /** Where the control hint goes (the notices stack); null hides it. */
  hintTarget: HTMLElement | null;
}

interface Drag {
  pointer: number;
  startX: number;
  startY: number;
  lastX: number;
  lastY: number;
  kind: 'pan' | 'orbit' | 'look';
  moved: boolean;
  touch: boolean;
}

/** Two fingers: their midpoint, distance and angle at the last move. */
interface Gesture {
  mid: [number, number];
  span: number;
  angle: number;
}

interface Flight {
  from: Camera3D;
  to: Camera3D;
  start: number;
  duration: number;
}

const DRAG_THRESHOLD = 4;
/** Finger movement still counted as a tap. */
const TOUCH_SLOP = 10;
/** Radians per dragged pixel. */
const ORBIT_SPEED = 0.006;
const LOOK_SPEED = 0.004;
/** Q/E, radians per second. */
const TURN_SPEED = 1.8;
/** Map-mode WASD, in camera distances per second: the same screen speed at every zoom. */
const PAN_SPEED = 0.9;
/** Fly speed in blocks per second: the default and the wheel's range. */
const FLY_SPEED = 24;
const MIN_FLY_SPEED = 2;
const MAX_FLY_SPEED = 600;
/** What Shift multiplies speeds by. */
const FAST = 3;
const FLY_MS = 650;
const SETTLE_MS = 300;
/** Farther camera moves jump: a flight over thousands of blocks would only queue regions it passes. */
const INSTANT_JUMP = 3000;
/** Screen cell for grouping mobs and entities. */
const CLUSTER_CELL = 44;
/** Markers this far off screen are still drawn (half a marker sticks in). */
const MARKER_MARGIN = 40;
const VIEW_SYNC_MS = 250;
/** Keys that move the camera while held. */
const MOVE_KEYS: Record<string, true> = {
  KeyW: true,
  KeyA: true,
  KeyS: true,
  KeyD: true,
  KeyQ: true,
  KeyE: true,
  Space: true,
  KeyC: true,
  AltLeft: true,
  AltRight: true,
};

/** Entity icons that failed to load, drawn as dots from then on. */
const failedIcons = new Set<string>();

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));
const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
/** The signed smallest difference between two angles. */
const angleDelta = (from: number, to: number) => {
  const turn = Math.PI * 2;
  return ((((to - from) % turn) + turn * 1.5) % turn) - Math.PI;
};
const ease = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2);

const between = (from: Camera3D, to: Camera3D, t: number): Camera3D => ({
  mode: to.mode,
  x: lerp(from.x, to.x, t),
  y: lerp(from.y, to.y, t),
  z: lerp(from.z, to.z, t),
  yaw: from.yaw + angleDelta(from.yaw, to.yaw) * t,
  pitch: lerp(from.pitch, to.pitch, t),
  distance: lerp(from.distance, to.distance, t),
});

/** The HTTP status of a failed request, if it got an answer. */
function statusOf(error: unknown): number | null {
  if (!error || typeof error !== 'object' || !('response' in error)) return null;
  const response = error.response;
  if (!response || typeof response !== 'object' || !('status' in response)) return null;
  return typeof response.status === 'number' ? response.status : null;
}

const sameStatus = (a: TerrainStatus, b: TerrainStatus) =>
  a.loading === b.loading &&
  a.meshing === b.meshing &&
  a.regions === b.regions &&
  a.capped === b.capped &&
  a.error === b.error;

/** The WebGL2 relief view: owns the canvas, the engine, the camera and every pointer and key gesture,
 * and draws the markers over the terrain. */
export default function Map3D(props: Props) {
  const { t } = useExtTranslations();
  const { size, compact } = props;
  const canvas = useRef<HTMLCanvasElement>(null);
  const engine = useRef<Terrain3D | null>(null);
  const latest = useRef(props);
  latest.current = props;
  const text = useRef(t);
  text.current = t;
  const [initialCamera] = useState(() =>
    clampCamera(from2D(props.initialView, Math.max(1, props.size.height), props.groundY)),
  );
  const cam = useRef<Camera3D>(initialCamera);
  const flight = useRef<Flight | null>(null);
  /** The map target should drop (or rise) onto the terrain once its height is known. */
  const settle = useRef(true);
  const keys = useRef(new Set<string>());
  const shift = useRef(false);
  const loop = useRef<number | null>(null);
  const lastTick = useRef(0);
  const drag = useRef<Drag | null>(null);
  const pointers = useRef(new Map<number, [number, number]>());
  const gesture = useRef<Gesture | null>(null);
  const syncTimer = useRef<number | undefined>(undefined);
  const hoverPoint = useRef<[number, number] | null>(null);
  const hoverFrame = useRef<number | null>(null);
  const lastStatus = useRef<TerrainStatus | null>(null);
  const cardToken = useRef(0);
  const seenGoto = useRef(props.goto?.nonce);
  const clusters = useRef<Cluster<Entity>[]>([]);
  const flySpeed = useRef(FLY_SPEED);
  const [mode, setMode] = useState<Mode>(initialCamera.mode);
  const [speed, setSpeed] = useState(FLY_SPEED);
  const [card, setCard] = useState<BlockCardState | null>(null);
  const [hovered, setHovered] = useState<Cluster<Entity> | null>(null);
  const hoveredRef = useRef<Cluster<Entity> | null>(null);
  const [, setFrame] = useState(0);
  const [, setIconsFailed] = useState(0);
  const noHover = useMediaQuery('(hover: none)');

  const byPosition = useMemo(
    () => new Map(props.regions.map((region) => [`${region.x},${region.z}`, region])),
    [props.regions],
  );
  const regionsRef = useRef(byPosition);
  regionsRef.current = byPosition;

  const showHovered = (group: Cluster<Entity> | null) => {
    if (hoveredRef.current === group) return;
    hoveredRef.current = group;
    setHovered(group);
  };

  const scheduleSync = () => {
    clearTimeout(syncTimer.current);
    syncTimer.current = window.setTimeout(() => {
      latest.current.onView(to2D(cam.current, Math.max(1, latest.current.size.height)));
    }, VIEW_SYNC_MS);
  };

  /** Shows a camera: clamped, handed to the engine, synced back to the page after a pause. */
  const push = (next: Camera3D) => {
    cam.current = clampCamera(next);
    engine.current?.setCamera(cam.current);
    // a group's tooltip sits where the group was drawn; it would float off once the view moves
    showHovered(null);
    scheduleSync();
  };

  /** A camera from the user's own input: it ends any flight. */
  const input = (next: Camera3D) => {
    flight.current = null;
    push(next);
  };

  const tick = (now: number) => {
    loop.current = null;
    const dt = Math.min(0.1, (now - lastTick.current) / 1000);
    lastTick.current = now;
    let active = false;
    const current = flight.current;
    if (current) {
      const progress = Math.min(1, (now - current.start) / current.duration);
      push(between(current.from, current.to, ease(progress)));
      if (progress < 1) active = true;
      else flight.current = null;
    }
    if (keys.current.size > 0 && moveByKeys(dt)) active = true;
    if (active) loop.current = requestAnimationFrame(tick);
  };

  const ensureLoop = () => {
    if (loop.current !== null) return;
    lastTick.current = performance.now();
    loop.current = requestAnimationFrame(tick);
  };

  /** Moves for the held keys; false when none of them moves the camera in this mode. */
  const moveByKeys = (dt: number): boolean => {
    const held = keys.current;
    const axis = (plus: boolean, minus: boolean) => (plus ? 1 : 0) - (minus ? 1 : 0);
    const forward = axis(held.has('KeyW'), held.has('KeyS'));
    const right = axis(held.has('KeyD'), held.has('KeyA'));
    const turn = axis(held.has('KeyE'), held.has('KeyQ'));
    const up = axis(held.has('Space'), held.has('KeyC') || held.has('AltLeft') || held.has('AltRight'));
    const current = cam.current;
    const fast = shift.current ? FAST : 1;
    if (current.mode === 'map') {
      if (forward === 0 && right === 0 && turn === 0) return false;
      const step = current.distance * PAN_SPEED * fast * dt;
      let next = forward !== 0 || right !== 0 ? moveMap(current, forward * step, right * step) : current;
      if (turn !== 0) next = orbit(next, turn * TURN_SPEED * dt, 0);
      settle.current = true;
      input(next);
      return true;
    }
    if (forward === 0 && right === 0 && turn === 0 && up === 0) return false;
    const step = flySpeed.current * fast * dt;
    let next = flyMove(current, forward * step, right * step, up * step);
    if (turn !== 0) next = flyLook(next, turn * TURN_SPEED * dt, 0);
    input(next);
    return true;
  };

  /** Flies the camera to `to`, or jumps there with reduced motion or over long distances. */
  const fly = (to: Camera3D, duration: number) => {
    const from = cam.current;
    const instant = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    if (instant || Math.hypot(to.x - from.x, to.z - from.z) > INSTANT_JUMP) {
      flight.current = null;
      push(to);
      return;
    }
    flight.current = { from, to: clampCamera(to), start: performance.now(), duration };
    ensureLoop();
  };

  /** Drops the map target onto the terrain once its column has loaded. */
  const trySettle = () => {
    const current = cam.current;
    const terrain = engine.current;
    if (!settle.current || !terrain || current.mode !== 'map' || flight.current || drag.current) return;
    const height = terrain.heightAt(Math.floor(current.x), Math.floor(current.z));
    if (height === null) return;
    settle.current = false;
    if (Math.abs(height + 1 - current.y) > 1) fly({ ...current, y: height + 1 }, SETTLE_MS);
  };

  /** The ground height in the middle of the screen, for turning a fly camera into a map one. */
  const groundAhead = (): number => {
    const terrain = engine.current;
    const { width, height } = latest.current.size;
    const hit = terrain?.pick(width / 2, height / 2) ?? null;
    if (hit) return hit.y + 1;
    const below = terrain?.heightAt(Math.floor(cam.current.x), Math.floor(cam.current.z)) ?? null;
    return below !== null ? below + 1 : latest.current.groundY;
  };

  const switchMode = (next: Mode) => {
    const current = cam.current;
    if (current.mode === next) return;
    flight.current = null;
    push(next === 'fly' ? toFly(current) : toMap(current, groundAhead()));
    settle.current = next === 'map';
    setMode(next);
  };

  const focusCanvas = () => canvas.current?.focus({ preventScroll: true });

  const closeCard = () => {
    cardToken.current++;
    setCard(null);
    engine.current?.setHighlight(null);
  };

  const openCard = (x: number, y: number, z: number, left: number, top: number) => {
    const token = ++cardToken.current;
    engine.current?.setHighlight({ x, y, z });
    setCard({ x, y, z, left, top, info: null, error: null });
    const { server, place } = latest.current;
    getBlock(server, place, x, z)
      .then((info) => {
        if (token !== cardToken.current) return;
        setCard((current) => current && { ...current, info });
        engine.current?.setHighlight({ x: info.x, y: info.y, z: info.z });
      })
      .catch((error: unknown) => {
        if (token !== cardToken.current) return;
        const message = statusOf(error) === 404 ? text.current('map3d.block.missing', {}) : httpErrorToHuman(error);
        setCard((current) => current && { ...current, error: message });
      });
  };

  const groupAt = (sx: number, sy: number): Cluster<Entity> | null =>
    clusters.current.find(
      (group) => Math.hypot(group.sx - sx, group.sy - sy) <= groupRadius(group.members.length) + 4,
    ) ?? null;

  /** A click or tap without dragging: inspect a mob group, pick a teleport spot, or open a block's card. */
  const click = (sx: number, sy: number) => {
    const group = groupAt(sx, sy);
    if (group) {
      showHovered(group);
      return;
    }
    const hit = engine.current?.pick(sx, sy) ?? null;
    if (!hit) {
      closeCard();
      return;
    }
    latest.current.onCursor([hit.x, hit.z]);
    if (latest.current.picking) {
      latest.current.onPick(hit.x, hit.y + 1, hit.z);
      return;
    }
    openCard(hit.x, hit.y, hit.z, sx, sy);
  };

  const reset = () => {
    const [x, savedY, z] = latest.current.home;
    const ground = engine.current?.heightAt(Math.floor(x), Math.floor(z)) ?? null;
    const y = ground !== null ? ground + 1 : (savedY ?? latest.current.groundY);
    if (cam.current.mode === 'fly') {
      push(toMap(cam.current, groundAhead()));
      setMode('map');
    }
    settle.current = ground === null;
    fly({ mode: 'map', x, y, z, yaw: DEFAULT_YAW, pitch: DEFAULT_PITCH, distance: DEFAULT_DISTANCE }, FLY_MS);
  };

  // the engine lives as long as the view; the page falls back to 2D when it cannot run
  useEffect(() => {
    const element = canvas.current;
    if (!element) return;
    if (!Terrain3D.supported()) {
      latest.current.onUnavailable({ kind: 'unsupported' });
      return;
    }
    let terrain: Terrain3D;
    try {
      terrain = new Terrain3D(element, {
        fetchSurface: (rx, rz, signal) => {
          const region = regionsRef.current.get(`${rx},${rz}`);
          if (!region) return Promise.reject(new Error(`r.${rx}.${rz}.mca is gone`));
          const { server, place } = latest.current;
          return getSurface(server, place, region, signal).catch((error: unknown) => {
            // a cancelled request is not a region error: the engine drops it as it is
            if (signal.aborted) throw error;
            throw new Error(statusOf(error) === 422 ? text.current('map3d.unreadable', {}) : httpErrorToHuman(error));
          });
        },
        onStatus: (status) => {
          if (lastStatus.current && sameStatus(lastStatus.current, status)) return;
          lastStatus.current = { ...status };
          latest.current.onStatus(status);
        },
        onContextLost: () => latest.current.onUnavailable({ kind: 'lost' }),
        onFrame: () => {
          trySettle();
          setFrame((frame) => frame + 1);
        },
      });
    } catch (error) {
      latest.current.onUnavailable({ kind: 'failed', error: error instanceof Error ? error.message : String(error) });
      return;
    }
    engine.current = terrain;
    const { width, height } = latest.current.size;
    terrain.resize(width, height, window.devicePixelRatio || 1);
    terrain.setCamera(cam.current);
    return () => {
      // the 2D map opens where the 3D camera looked
      clearTimeout(syncTimer.current);
      latest.current.onView(to2D(cam.current, Math.max(1, latest.current.size.height)));
      if (loop.current !== null) cancelAnimationFrame(loop.current);
      if (hoverFrame.current !== null) cancelAnimationFrame(hoverFrame.current);
      loop.current = null;
      hoverFrame.current = null;
      flight.current = null;
      keys.current.clear();
      engine.current = null;
      terrain.dispose();
    };
  }, []);

  const placeKey = `${props.server}|${props.place.world}|${props.place.dimension}`;
  useEffect(() => {
    engine.current?.setPlace(placeKey, props.regions, props.atmosphere);
  }, [placeKey, props.regions, props.atmosphere]);

  // another world or dimension: the old block card and ground height mean nothing there
  useEffect(() => {
    settle.current = true;
    closeCard();
    showHovered(null);
  }, [placeKey]);

  useEffect(() => {
    if (size.width === 0 || size.height === 0) return;
    engine.current?.resize(size.width, size.height, window.devicePixelRatio || 1);
  }, [size.width, size.height]);

  useEffect(() => {
    engine.current?.setTints(props.tints);
  }, [props.tints]);

  useEffect(() => {
    const request = props.goto;
    if (!request || request.nonce === seenGoto.current) return;
    seenGoto.current = request.nonce;
    const ground = engine.current?.heightAt(Math.floor(request.x), Math.floor(request.z)) ?? null;
    const y = request.y ?? (ground !== null ? ground + 1 : cam.current.y);
    settle.current = request.y === null && ground === null;
    closeCard();
    fly(flyTo(cam.current, request.x, y, request.z), FLY_MS);
  }, [props.goto]);

  // keys move the camera while the map (or nothing in particular) has focus, never while typing
  useEffect(() => {
    const accepts = (event: KeyboardEvent) => {
      if (event.ctrlKey || event.metaKey) return false;
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest('input, textarea, select, [contenteditable="true"], [role="dialog"], [role="menu"]')) {
        return false;
      }
      // Space presses a focused button
      if (event.code === 'Space' && target?.closest('button, a')) return false;
      const root = canvas.current?.closest('.wm-map');
      return !target || target === document.body || !!root?.contains(target);
    };
    const onDown = (event: KeyboardEvent) => {
      shift.current = event.shiftKey;
      if (event.key === 'Escape') {
        closeCard();
        showHovered(null);
        return;
      }
      if (!accepts(event)) return;
      if (event.code === 'KeyF' && !event.shiftKey && !event.altKey) {
        event.preventDefault();
        if (!event.repeat) switchMode(cam.current.mode === 'map' ? 'fly' : 'map');
        return;
      }
      if (!Object.hasOwn(MOVE_KEYS, event.code)) return;
      event.preventDefault();
      flight.current = null;
      keys.current.add(event.code);
      ensureLoop();
    };
    const onUp = (event: KeyboardEvent) => {
      shift.current = event.shiftKey;
      if (keys.current.delete(event.code) && keys.current.size === 0) trySettle();
    };
    const onBlur = () => {
      keys.current.clear();
      shift.current = false;
    };
    window.addEventListener('keydown', onDown);
    window.addEventListener('keyup', onUp);
    window.addEventListener('blur', onBlur);
    return () => {
      window.removeEventListener('keydown', onDown);
      window.removeEventListener('keyup', onUp);
      window.removeEventListener('blur', onBlur);
    };
  }, []);

  // the wheel zooms toward the cursor, or sets the fly speed (a passive React listener could not stop
  // the page from scrolling)
  useEffect(() => {
    const element = canvas.current;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const delta = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? 400 : 1);
      const current = cam.current;
      if (current.mode === 'fly') {
        flySpeed.current = clamp(flySpeed.current * 1.2 ** (-delta / 100), MIN_FLY_SPEED, MAX_FLY_SPEED);
        setSpeed(Math.round(flySpeed.current));
        return;
      }
      const rect = element.getBoundingClientRect();
      const hit = engine.current?.pick(event.clientX - rect.left, event.clientY - rect.top) ?? null;
      const toward: [number, number, number] | undefined = hit ? [hit.x + 0.5, hit.y + 1, hit.z + 0.5] : undefined;
      input(zoomMap(current, 1.0015 ** delta, toward));
    };
    element.addEventListener('wheel', onWheel, { passive: false });
    return () => element.removeEventListener('wheel', onWheel);
  }, []);

  const local = (event: ReactPointerEvent): [number, number] => {
    const rect = canvas.current!.getBoundingClientRect();
    return [event.clientX - rect.left, event.clientY - rect.top];
  };

  const twoFingers = (): Gesture => {
    const [a, b] = [...pointers.current.values()];
    return {
      mid: [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2],
      span: Math.max(1, Math.hypot(b[0] - a[0], b[1] - a[1])),
      angle: Math.atan2(b[1] - a[1], b[0] - a[0]),
    };
  };

  /** Pinch zooms (or flies forward), twist turns, a vertical two-finger drag tilts. */
  const onGesture = () => {
    const previous = gesture.current;
    if (!previous || pointers.current.size !== 2) return;
    const now = twoFingers();
    gesture.current = now;
    const twist = angleDelta(previous.angle, now.angle);
    const tilt = now.mid[1] - previous.mid[1];
    const current = cam.current;
    if (current.mode === 'map') {
      const hit = engine.current?.pick(now.mid[0], now.mid[1]) ?? null;
      const toward: [number, number, number] | undefined = hit ? [hit.x + 0.5, hit.y + 1, hit.z + 0.5] : undefined;
      // fingers turning clockwise turn the world with them, so the camera turns the other way
      input(orbit(zoomMap(current, previous.span / now.span, toward), -twist, tilt * ORBIT_SPEED));
    } else {
      const forward = ((now.span - previous.span) * flySpeed.current) / 60;
      input(flyLook(flyMove(current, forward, 0, 0), -twist, -tilt * LOOK_SPEED));
    }
  };

  const onPointerDown = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const touch = event.pointerType !== 'mouse';
    if (!touch && event.button !== 0 && event.button !== 2) return;
    focusCanvas();
    event.currentTarget.setPointerCapture(event.pointerId);
    const [sx, sy] = local(event);
    pointers.current.set(event.pointerId, [sx, sy]);
    flight.current = null;
    if (pointers.current.size === 2) {
      // a second finger: a two-finger gesture instead of whatever the first one started
      drag.current = null;
      gesture.current = twoFingers();
      return;
    }
    if (pointers.current.size > 2) return;
    const turning = !touch && (event.button === 2 || event.shiftKey);
    drag.current = {
      pointer: event.pointerId,
      startX: sx,
      startY: sy,
      lastX: sx,
      lastY: sy,
      kind: cam.current.mode === 'fly' ? 'look' : turning ? 'orbit' : 'pan',
      moved: false,
      touch,
    };
  };

  const hover = (sx: number, sy: number) => {
    hoverPoint.current = [sx, sy];
    if (hoverFrame.current !== null) return;
    // picking is a ray walk: once per frame is plenty for the cursor readout
    hoverFrame.current = requestAnimationFrame(() => {
      hoverFrame.current = null;
      const point = hoverPoint.current;
      if (!point || drag.current) return;
      const hit = engine.current?.pick(point[0], point[1]) ?? null;
      latest.current.onCursor(hit ? [hit.x, hit.z] : null);
      showHovered(groupAt(point[0], point[1]));
    });
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const [sx, sy] = local(event);
    if (pointers.current.has(event.pointerId)) pointers.current.set(event.pointerId, [sx, sy]);
    if (gesture.current) {
      onGesture();
      return;
    }
    const current = drag.current;
    if (!current || current.pointer !== event.pointerId) {
      if (event.pointerType === 'mouse') hover(sx, sy);
      return;
    }
    if (!current.moved) {
      const slop = current.touch ? TOUCH_SLOP : DRAG_THRESHOLD;
      if (Math.hypot(sx - current.startX, sy - current.startY) <= slop) return;
      current.moved = true;
    }
    const dx = sx - current.lastX;
    const dy = sy - current.lastY;
    current.lastX = sx;
    current.lastY = sy;
    const camera = cam.current;
    if (current.kind === 'pan') input(panMap(camera, dx, dy, Math.max(1, size.height)));
    else if (current.kind === 'orbit') input(orbit(camera, -dx * ORBIT_SPEED, dy * ORBIT_SPEED));
    else input(flyLook(camera, dx * LOOK_SPEED, -dy * LOOK_SPEED));
  };

  const onPointerUp = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    pointers.current.delete(event.pointerId);
    if (gesture.current) {
      // the fingers left after a gesture do nothing until all are lifted
      if (pointers.current.size === 0) {
        gesture.current = null;
        settle.current = true;
        trySettle();
      }
      return;
    }
    const current = drag.current;
    if (!current || current.pointer !== event.pointerId) return;
    drag.current = null;
    if (current.moved) {
      if (current.kind === 'pan') {
        settle.current = true;
        trySettle();
      }
      return;
    }
    const [sx, sy] = local(event);
    click(sx, sy);
  };

  // markers: re-projected on every drawn frame (onFrame re-renders this component)
  const terrain = engine.current;
  const onScreen = (x: number, y: number, z: number): [number, number] | null => {
    const point = terrain?.project(x, y, z) ?? null;
    if (!point) return null;
    const [left, top] = point;
    if (left < -MARKER_MARGIN || top < -MARKER_MARGIN) return null;
    if (left > size.width + MARKER_MARGIN || top > size.height + MARKER_MARGIN) return null;
    return point;
  };

  const groups = new Map<string, Cluster<Entity>>();
  for (const entity of props.entities) {
    if (!props.layers[entityLayer(entity.id)]) continue;
    // entity positions are at their feet: the icon sits on the body instead
    const point = onScreen(entity.x, entity.y + 0.5, entity.z);
    if (!point) continue;
    const [sx, sy] = point;
    const key = `${Math.floor(sx / CLUSTER_CELL)},${Math.floor(sy / CLUSTER_CELL)}`;
    const group = groups.get(key);
    if (group) {
      const n = group.members.length;
      group.sx = (group.sx * n + sx) / (n + 1);
      group.sy = (group.sy * n + sy) / (n + 1);
      group.members.push(entity);
    } else {
      groups.set(key, { sx, sy, members: [entity] });
    }
  }
  clusters.current = [...groups.values()];

  let spawnPoint: [number, number] | null = null;
  if (props.spawn) {
    const [x, y, z] = props.spawn;
    const ground = terrain?.heightAt(Math.floor(x), Math.floor(z)) ?? null;
    spawnPoint = onScreen(x + 0.5, ground !== null ? ground + 1 : y, z + 0.5);
  }

  const teleportName = props.canTeleport && props.selected?.name ? playerLabel(props.selected) : null;
  const degrees = Math.round(compassDegrees(cam.current)) % 360;
  const showHint = !compact && !noHover && props.hintTarget;

  const controls = (
    <div className='wm-glass wm-3d-controls' role='group' aria-label={t('map3d.controls', {})}>
      <Tooltip label={t('map3d.compass', { degrees })} withinPortal unwrapped>
        <button
          type='button'
          className='wm-tool wm-compass'
          aria-label={t('map3d.compass', { degrees })}
          onClick={() => {
            fly({ ...cam.current, yaw: DEFAULT_YAW }, 400);
            focusCanvas();
          }}
        >
          <svg viewBox='0 0 32 32' width='30' height='30' style={{ transform: `rotate(${-degrees}deg)` }} aria-hidden>
            <circle cx='16' cy='16' r='14.5' fill='none' stroke='currentColor' strokeOpacity='0.25' />
            <path d='M16 4 L20 16 L12 16 Z' fill='#ef4444' />
            <path d='M16 28 L20 16 L12 16 Z' fill='currentColor' fillOpacity='0.55' />
            <text x='16' y='3.6' textAnchor='middle' fontSize='5.5' fontWeight='700' fill='currentColor'>
              N
            </text>
          </svg>
        </button>
      </Tooltip>
      <SegmentedControl
        size='xs'
        radius='xl'
        aria-label={t('map3d.cameraMode', {})}
        data={[
          { value: 'map', label: t('map3d.map', {}) },
          { value: 'fly', label: t('map3d.fly', {}) },
        ]}
        value={mode}
        onChange={(value) => {
          switchMode(value === 'fly' ? 'fly' : 'map');
          focusCanvas();
        }}
      />
      <Tooltip label={t('map3d.resetLabel', {})} withinPortal unwrapped>
        <button
          type='button'
          className='wm-3d-reset'
          onClick={() => {
            reset();
            focusCanvas();
          }}
        >
          <FontAwesomeIcon icon={faArrowRotateLeft} />
          {t('map3d.reset', {})}
        </button>
      </Tooltip>
    </div>
  );

  return (
    <>
      <canvas
        ref={canvas}
        className='wm-3d-canvas'
        tabIndex={0}
        aria-label={t('map3d.canvas', {})}
        data-mode={mode}
        data-picking={props.picking}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={(event) => {
          pointers.current.delete(event.pointerId);
          if (pointers.current.size === 0) gesture.current = null;
          drag.current = null;
        }}
        onPointerLeave={(event) => {
          if (event.pointerType !== 'mouse') return;
          hoverPoint.current = null;
          props.onCursor(null);
          showHovered(null);
        }}
        onContextMenu={(event) => event.preventDefault()}
      />

      {clusters.current.map((group) => {
        const count = group.members.length;
        const id = mostCommon(group.members);
        const url = entityIcon(id);
        const radius = groupRadius(count);
        const iconSize = count > 1 ? 16 : ICON_SIZE;
        const kind = entityLayer(id) === 'entities' ? 'other' : mobKind(id);
        return (
          <div
            key={`${group.members[0].x},${group.members[0].z},${id}`}
            className='wm-entity3d'
            data-group={count > 1}
            style={{ left: group.sx, top: group.sy, width: radius * 2, height: radius * 2 }}
          >
            {url && !failedIcons.has(url) ? (
              <img
                src={url}
                alt=''
                width={iconSize}
                height={iconSize}
                onError={() => {
                  failedIcons.add(url);
                  setIconsFailed((n) => n + 1);
                }}
              />
            ) : (
              <span className='wm-entity3d-dot' data-kind={kind} />
            )}
            {count > 1 && <span className='wm-entity3d-count'>{count > 999 ? '999+' : count}</span>}
          </div>
        );
      })}

      {spawnPoint && (
        <div className='wm-spawn3d' title={t('map.spawn', {})} style={{ left: spawnPoint[0], top: spawnPoint[1] }}>
          <FontAwesomeIcon icon={faLocationDot} />
        </div>
      )}

      {props.players.map((player) => {
        const point = onScreen(player.x, player.y, player.z);
        if (!point) return null;
        const isSelected = player.uuid === props.selectedUuid;
        const online = isOnline(player, props.onlineNow);
        return (
          <button
            key={player.uuid}
            type='button'
            className='wm-marker'
            data-selected={isSelected}
            data-online={online}
            aria-label={`${playerLabel(player)} (${online ? t('players.online', {}) : t('players.offline', {})})`}
            onClick={() => props.onSelectPlayer(isSelected ? null : player.uuid)}
            style={{ left: point[0], top: point[1], zIndex: isSelected ? 4 : 3 }}
          >
            <span style={{ position: 'relative' }}>
              <img className='wm-marker-head' src={avatarUrl(player.uuid, 48)} alt='' />
              {online && <span className='wm-marker-online' />}
            </span>
            <span className='wm-marker-pin' />
            <span className='wm-marker-name'>{playerLabel(player)}</span>
          </button>
        );
      })}

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

      {card && (
        <BlockCard
          card={card}
          style={
            compact
              ? { position: 'absolute', left: 12, right: 12, bottom: 124, zIndex: 7 }
              : {
                  position: 'absolute',
                  left: clamp(card.left + 14, 8, Math.max(8, size.width - 296)),
                  top: clamp(card.top + 14, 8, Math.max(8, size.height - 260)),
                  zIndex: 7,
                  width: 280,
                }
          }
          teleportName={teleportName}
          onCopy={(value) => props.onCopy(value)}
          onTeleport={(x, y, z) => {
            if (props.selected) props.onTeleport(props.selected, x, y, z);
            closeCard();
          }}
          onClose={() => {
            closeCard();
            focusCanvas();
          }}
        />
      )}

      {props.controlsTarget && createPortal(controls, props.controlsTarget)}
      {showHint &&
        props.hintTarget &&
        createPortal(
          <div className='wm-glass wm-3d-hint'>
            <Text size='xs'>{mode === 'fly' ? t('map3d.hintFly', {}) : t('map3d.hintMap', {})}</Text>
            {mode === 'fly' && (
              <Text className='wm-label' mt={2}>
                {t('map3d.speed', { speed })}
              </Text>
            )}
          </div>,
          props.hintTarget,
        )}
    </>
  );
}
