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
  faUsers,
  type IconDefinition,
} from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { forwardRef, type ReactNode } from 'react';
import Switch from '@/elements/input/Switch.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import Popover from '@/elements/overlays/Popover.tsx';
import Tooltip from '@/elements/overlays/Tooltip.tsx';
import Text from '@/elements/typography/Text.tsx';
import type { Layers } from './MapCanvas.tsx';
import { useExtTranslations } from '../translations.ts';
import HelpContent from './HelpContent.tsx';

interface Props {
  compact: boolean;
  layers: Layers;
  onLayer: (layer: keyof Layers) => void;
  selecting: boolean;
  onSelecting: () => void;
  fullscreen: boolean;
  onFullscreen: () => void;
  onHome: () => void;
  heatmapDays: number;
}

interface ToolProps {
  label: string;
  icon: IconDefinition;
  pressed?: boolean;
  onClick?: () => void;
}

/** A 40 px round map tool; `pressed` marks layers and modes that are on. */
const Tool = forwardRef<HTMLButtonElement, ToolProps>(({ label, icon, pressed, onClick }, ref) => (
  <button ref={ref} type='button' className='wm-tool' aria-label={label} aria-pressed={pressed} onClick={onClick}>
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
  const layers: [keyof Layers, string, IconDefinition][] = [
    ['players', t('toolbar.players', {}), faUsers],
    ['markers', t('toolbar.markers', {}), faLocationDot],
    ['heatmap', t('toolbar.heatmap', {}), faFire],
    ['mobs', t('toolbar.mobs', {}), faDragon],
    ['entities', t('toolbar.entities', {}), faCube],
  ];
  const activeLayers = layers.filter(([name]) => props.layers[name]).length;

  const help = (
    <Popover position='top-end' width={300} withinPortal shadow='md'>
      <Popover.Target>
        <Tool label={t('toolbar.help', {})} icon={faCircleQuestion} />
      </Popover.Target>
      <Popover.Dropdown>
        <HelpContent heatmapDays={props.heatmapDays} />
      </Popover.Dropdown>
    </Popover>
  );

  return (
    <div className='wm-glass wm-toolbar' role='toolbar' aria-label={t('title', {})}>
      {tip(t('toolbar.home', {}), <Tool label={t('toolbar.home', {})} icon={faHouse} onClick={props.onHome} />)}
      <span className='wm-toolbar-divider' />
      {props.compact ? (
        <Popover position='top' width={240} withinPortal shadow='md'>
          <Popover.Target>
            <Tool
              label={`${t('toolbar.layers', {})} (${activeLayers})`}
              icon={faLayerGroup}
              pressed={activeLayers > 0}
            />
          </Popover.Target>
          <Popover.Dropdown>
            <Stack gap='sm'>
              <Text size='xs' fw={600}>
                {t('toolbar.layers', {})}
              </Text>
              {layers.map(([name, label]) => (
                <Switch
                  key={name}
                  size='md'
                  label={label}
                  checked={props.layers[name]}
                  onChange={() => props.onLayer(name)}
                />
              ))}
            </Stack>
          </Popover.Dropdown>
        </Popover>
      ) : (
        layers.map(([name, label, icon]) =>
          tip(
            label,
            <Tool label={label} icon={icon} pressed={props.layers[name]} onClick={() => props.onLayer(name)} />,
          ),
        )
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
