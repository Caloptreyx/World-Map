import Group from '@/elements/layout/Group.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import Anchor from '@/elements/typography/Anchor.tsx';
import Text from '@/elements/typography/Text.tsx';
import { useExtTranslations } from '../translations.ts';
import { FILTER_WIKI } from './FilterTab.tsx';
import { OVERLAY_WIKI } from './OverlaySettings.tsx';

const dot = (colour: string) => (
  <span style={{ width: 10, height: 10, borderRadius: 999, background: colour, display: 'inline-block' }} />
);

/** The legend and gestures, in the toolbar's help popover. */
export default function HelpContent({ heatmapDays }: { heatmapDays: number }) {
  const { t } = useExtTranslations();
  return (
    <Stack gap='sm'>
      <Text size='xs' fw={600}>
        {t('help.legend', {})}
      </Text>
      <Group gap='md'>
        <Group gap={6}>
          {dot('#ef4444')}
          <Text size='xs'>{t('help.hostile', {})}</Text>
        </Group>
        <Group gap={6}>
          {dot('#22c55e')}
          <Text size='xs'>{t('help.passive', {})}</Text>
        </Group>
        <Group gap={6}>
          {dot('#a1a1aa')}
          <Text size='xs'>{t('help.other', {})}</Text>
        </Group>
      </Group>
      <div>
        <div
          style={{
            height: 6,
            borderRadius: 3,
            background: 'linear-gradient(90deg, #285aff, #28dc78, #fadc28, #f03228)',
          }}
        />
        <Group justify='space-between' mt={4}>
          <Text className='wm-label'>{t('help.heatFew', {})}</Text>
          <Text className='wm-label'>{t('heatmap.legend', { days: heatmapDays })}</Text>
          <Text className='wm-label'>{t('help.heatMany', {})}</Text>
        </Group>
      </div>
      <Text size='xs' fw={600} mt={4}>
        {t('help.title', {})}
      </Text>
      {(['move', 'menu', 'players', 'heatmap', 'worlds', 'data'] as const).map((key) => (
        <Text key={key} size='xs' c='dimmed'>
          {t(`help.${key}`, {})}
        </Text>
      ))}
      <Text size='xs' fw={600} mt={4}>
        {t('help.view3dTitle', {})}
      </Text>
      {(['view3dSwitch', 'view3dMap', 'view3dFly', 'view3dTouch', 'view3dBlock', 'view3dTools'] as const).map((key) => (
        <Text key={key} size='xs' c='dimmed'>
          {t(`help.${key}`, {})}
        </Text>
      ))}
      <Text size='xs' fw={600} mt={4}>
        {t('help.toolsTitle', {})}
      </Text>
      {(['select', 'selectTouch', 'selectModes', 'selectCsv'] as const).map((key) => (
        <Text key={key} size='xs' c='dimmed'>
          {t(`help.${key}`, {})}
        </Text>
      ))}
      <Text size='xs' c='dimmed'>
        {t('help.filters', {})}{' '}
        <Anchor href={FILTER_WIKI} target='_blank' rel='noreferrer' size='xs'>
          {t('filter.wiki', {})}
        </Anchor>
      </Text>
      <Text size='xs' c='dimmed'>
        {t('help.overlays', {})}{' '}
        <Anchor href={OVERLAY_WIKI} target='_blank' rel='noreferrer' size='xs'>
          {t('filter.wiki', {})}
        </Anchor>
      </Text>
      {(['changes', 'imports', 'backups'] as const).map((key) => (
        <Text key={key} size='xs' c='dimmed'>
          {t(`help.${key}`, {})}
        </Text>
      ))}
    </Stack>
  );
}
