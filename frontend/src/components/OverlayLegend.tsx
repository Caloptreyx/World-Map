import Group from '@/elements/layout/Group.tsx';
import Text from '@/elements/typography/Text.tsx';
import { formatOverlayValue, LEGEND_GRADIENT } from '../lib/overlay.ts';
import { useExtTranslations } from '../translations.ts';

interface Props {
  type: string;
  parameter: string;
  min: number;
  max: number;
}

/** The overlay's colour scale with the values at both ends, next to the heatmap legend. */
export default function OverlayLegend({ type, parameter, min, max }: Props) {
  const { t } = useExtTranslations();
  return (
    <div className='wm-glass' style={{ padding: '6px 10px', width: 220 }}>
      <Text className='wm-label'>
        {t('overlay.legend', { type })}
        {parameter ? ` · ${parameter}` : ''}
      </Text>
      <div style={{ marginTop: 5, height: 5, borderRadius: 3, background: LEGEND_GRADIENT }} />
      <Group justify='space-between' mt={3} wrap='nowrap'>
        <span className='wm-mono' style={{ fontSize: 11 }}>
          {formatOverlayValue(type, min)}
        </span>
        <span className='wm-mono' style={{ fontSize: 11 }}>
          {formatOverlayValue(type, max)}
        </span>
      </Group>
    </div>
  );
}
