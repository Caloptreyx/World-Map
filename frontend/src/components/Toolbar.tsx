import {
  faBorderAll,
  faCircleQuestion,
  faCompress,
  faCube,
  faDragon,
  faExpand,
  faFire,
  faHouse,
  faLayerGroup,
  faLocationDot,
  faPalette,
  faToolbox,
  faUserCheck,
  faUserClock,
  type IconDefinition,
} from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { forwardRef, type ReactNode } from 'react';
import Switch from '@/elements/input/Switch.tsx';
import Divider from '@/elements/layout/Divider.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import Popover from '@/elements/overlays/Popover.tsx';
import Tooltip from '@/elements/overlays/Tooltip.tsx';
import Text from '@/elements/typography/Text.tsx';
import type { PlayerVisibility } from '../api.ts';
import type { Layers } from './MapCanvas.tsx';
import { useExtTranslations } from '../translations.ts';
import HelpContent from './HelpContent.tsx';
import OverlaySettings, { type OverlayChoice } from './OverlaySettings.tsx';

interface Props {
  compact: boolean;
  /** The layers as drawn: a kind of player the server hides is off. */
  layers: Layers;
  onLayer: (layer: keyof Layers) => void;
  /** Which players the server's setting lets the map show. */
  visibility: PlayerVisibility;
  selecting: boolean;
  onSelecting: () => void;
  fullscreen: boolean;
  onFullscreen: () => void;
  onHome: () => void;
  heatmapDays: number;
  toolsOpen: boolean;
  onTools: () => void;
  overlay: OverlayChoice;
  onOverlay: (overlay: OverlayChoice) => void;
}

interface ToolProps {
  label: string;
  icon: IconDefinition;
  pressed?: boolean;
  /** Shown but unavailable; the tooltip says why. */
  unavailable?: boolean;
  onClick?: () => void;
}

/** A 40 px round map tool; `pressed` marks layers and modes that are on. */
const Tool = forwardRef<HTMLButtonElement, ToolProps>(({ label, icon, pressed, unavailable, onClick }, ref) => (
  <button
    ref={ref}
    type='button'
    className='wm-tool'
    aria-label={label}
    aria-pressed={pressed}
    aria-disabled={unavailable || undefined}
    onClick={unavailable ? undefined : onClick}
  >
    <FontAwesomeIcon icon={icon} />
  </button>
));

const tip = (label: string, child: ReactNode) => (
  <Tooltip label={label} key={label} withinPortal unwrapped>
    {child}
  </Tooltip>
);

export default function Toolbar(props: Props) {
  const { t } = useExtTranslations();
  // the reason a layer cannot be turned on, else null
  const hidden = t('toolbar.hiddenByServer', {});
  const layers: [keyof Layers, string, IconDefinition, string | null][] = [
    ['onlinePlayers', t('toolbar.onlinePlayers', {}), faUserCheck, props.visibility.online ? null : hidden],
    ['offlinePlayers', t('toolbar.offlinePlayers', {}), faUserClock, props.visibility.offline ? null : hidden],
    ['markers', t('toolbar.markers', {}), faLocationDot, null],
    ['heatmap', t('toolbar.heatmap', {}), faFire, null],
    ['mobs', t('toolbar.mobs', {}), faDragon, null],
    ['entities', t('toolbar.entities', {}), faCube, null],
  ];
  const overlayOn = props.overlay.enabled;
  const activeLayers = layers.filter(([name]) => props.layers[name]).length + (overlayOn ? 1 : 0);

  const help = (
    <Popover position='top-end' width={340} withinPortal shadow='md'>
      <Popover.Target>
        <Tool label={t('toolbar.help', {})} icon={faCircleQuestion} />
      </Popover.Target>
      <Popover.Dropdown className='wm-popover-scroll'>
        <HelpContent heatmapDays={props.heatmapDays} />
      </Popover.Dropdown>
    </Popover>
  );
  const overlaySettings = <OverlaySettings value={props.overlay} onChange={props.onOverlay} />;

  return (
    <div className='wm-glass wm-toolbar' role='toolbar' aria-label={t('title', {})}>
      {tip(t('toolbar.home', {}), <Tool label={t('toolbar.home', {})} icon={faHouse} onClick={props.onHome} />)}
      <span className='wm-toolbar-divider' />
      {props.compact ? (
        <Popover position='top' width={280} withinPortal shadow='md'>
          <Popover.Target>
            <Tool
              label={`${t('toolbar.layers', {})} (${activeLayers})`}
              icon={faLayerGroup}
              pressed={activeLayers > 0}
            />
          </Popover.Target>
          <Popover.Dropdown className='wm-popover-scroll'>
            <Stack gap='sm'>
              <Text size='xs' fw={600}>
                {t('toolbar.layers', {})}
              </Text>
              {layers.map(([name, label, , unavailable]) => (
                <Switch
                  key={name}
                  size='md'
                  label={label}
                  description={unavailable ?? undefined}
                  disabled={!!unavailable}
                  checked={props.layers[name]}
                  onChange={() => props.onLayer(name)}
                />
              ))}
              <Divider />
              <Text size='xs' fw={600}>
                {t('toolbar.overlay', {})}
              </Text>
              {overlaySettings}
            </Stack>
          </Popover.Dropdown>
        </Popover>
      ) : (
        <>
          {layers.map(([name, label, icon, unavailable]) =>
            tip(
              unavailable ? `${label}: ${unavailable}` : label,
              <Tool
                label={label}
                icon={icon}
                pressed={props.layers[name]}
                unavailable={!!unavailable}
                onClick={() => props.onLayer(name)}
              />,
            ),
          )}
          <Popover position='top-end' width={300} withinPortal shadow='md'>
            <Popover.Target>
              <Tool label={t('toolbar.overlay', {})} icon={faPalette} pressed={overlayOn} />
            </Popover.Target>
            <Popover.Dropdown>
              <Stack gap='xs'>
                <Text size='xs' fw={600}>
                  {t('toolbar.overlay', {})}
                </Text>
                {overlaySettings}
              </Stack>
            </Popover.Dropdown>
          </Popover>
        </>
      )}
      <span className='wm-toolbar-divider' />
      {tip(
        t('toolbar.select', {}),
        <Tool
          label={t('toolbar.select', {})}
          icon={faBorderAll}
          pressed={props.selecting}
          onClick={props.onSelecting}
        />,
      )}
      {tip(
        t('toolbar.chunkTools', {}),
        <Tool label={t('toolbar.chunkTools', {})} icon={faToolbox} pressed={props.toolsOpen} onClick={props.onTools} />,
      )}
      {tip(
        t('toolbar.fullscreen', {}),
        <Tool
          label={t('toolbar.fullscreen', {})}
          icon={props.fullscreen ? faCompress : faExpand}
          pressed={props.fullscreen}
          onClick={props.onFullscreen}
        />,
      )}
      {help}
    </div>
  );
}
