import {
  faCircleQuestion,
  faCompress,
  faCube,
  faDragon,
  faExpand,
  faFire,
  faHouse,
  faLocationDot,
  faUsers,
  faBorderAll,
  type IconDefinition,
} from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import ActionIcon from '@/elements/buttons/ActionIcon.tsx';
import Group from '@/elements/layout/Group.tsx';
import Paper from '@/elements/layout/Paper.tsx';
import Tooltip from '@/elements/overlays/Tooltip.tsx';
import type { Layers } from './MapCanvas.tsx';
import { useExtTranslations } from '../translations.ts';
import { CARD_STYLE } from './OverviewCard.tsx';

interface Props {
  layers: Layers;
  onLayer: (layer: keyof Layers) => void;
  selecting: boolean;
  onSelecting: () => void;
  fullscreen: boolean;
  onFullscreen: () => void;
  onHome: () => void;
  onHelp: () => void;
}

export default function Toolbar(props: Props) {
  const { t } = useExtTranslations();
  const button = (label: string, icon: IconDefinition, active: boolean | null, onClick: () => void) => (
    <Tooltip label={label} key={label}>
      <ActionIcon
        radius='xl'
        size='lg'
        variant={active ? 'filled' : 'default'}
        aria-pressed={active ?? undefined}
        onClick={onClick}
      >
        <FontAwesomeIcon icon={icon} />
      </ActionIcon>
    </Tooltip>
  );
  const layer = (name: keyof Layers, label: string, icon: IconDefinition) =>
    button(label, icon, props.layers[name], () => props.onLayer(name));

  return (
    <Paper withBorder radius='xl' p={6} style={CARD_STYLE}>
      <Group gap={6} wrap='nowrap'>
        {button(t('toolbar.home', {}), faHouse, null, props.onHome)}
        {layer('players', t('toolbar.players', {}), faUsers)}
        {layer('markers', t('toolbar.markers', {}), faLocationDot)}
        {layer('heatmap', t('toolbar.heatmap', {}), faFire)}
        {layer('mobs', t('toolbar.mobs', {}), faDragon)}
        {layer('entities', t('toolbar.entities', {}), faCube)}
        {button(t('toolbar.select', {}), faBorderAll, props.selecting, props.onSelecting)}
        {button(t('toolbar.fullscreen', {}), props.fullscreen ? faCompress : faExpand, null, props.onFullscreen)}
        {button(t('toolbar.help', {}), faCircleQuestion, null, props.onHelp)}
      </Group>
    </Paper>
  );
}
