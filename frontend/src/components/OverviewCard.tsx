import { useState } from 'react';
import Button from '@/elements/buttons/Button.tsx';
import Badge from '@/elements/data-display/Badge.tsx';
import NumberInput from '@/elements/input/NumberInput.tsx';
import Select from '@/elements/input/Select.tsx';
import Group from '@/elements/layout/Group.tsx';
import Paper from '@/elements/layout/Paper.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import Text from '@/elements/typography/Text.tsx';
import type { Overview, Place } from '../api.ts';
import { type ExtT, useExtTranslations } from '../translations.ts';

/** A dimension's name: vanilla ones translated, datapack ones from their id. */
export function dimensionLabel(t: ExtT, id: string): string {
  switch (id) {
    case 'minecraft:overworld':
      return t('dimensions.overworld', {});
    case 'minecraft:the_nether':
      return t('dimensions.nether', {});
    case 'minecraft:the_end':
      return t('dimensions.end', {});
    default:
      return id
        .split(':')
        .pop()!
        .split(/[_/]/)
        .filter(Boolean)
        .map((word) => word[0].toUpperCase() + word.slice(1))
        .join(' ');
  }
}

export const CARD_STYLE = {
  background: 'color-mix(in srgb, var(--mantine-color-body) 88%, transparent)',
  backdropFilter: 'blur(6px)',
} as const;

interface Props {
  overview: Overview;
  place: Place;
  onPlace: (place: Place) => void;
  onJump: (x: number, z: number) => void;
}

export default function OverviewCard({ overview, place, onPlace, onJump }: Props) {
  const { t } = useExtTranslations();
  const [x, setX] = useState<number | string>('');
  const [z, setZ] = useState<number | string>('');
  const world = overview.worlds.find((candidate) => candidate.name === place.world);
  const spawn = overview.level.spawn;

  return (
    <Paper withBorder radius='md' p='sm' w={300} style={CARD_STYLE}>
      <Stack gap='xs'>
        <Group justify='space-between' wrap='nowrap'>
          <Text fw={600}>{t('overview.title', {})}</Text>
          <Badge variant='light' size='sm'>
            {t('overview.badge', {})}
          </Badge>
        </Group>
        <Text size='xs' c='dimmed'>
          {t('overview.description', {})}
        </Text>
        <Group justify='space-between' gap={4}>
          <Text size='xs' c='dimmed'>
            {t('overview.lastView', {})}
          </Text>
          <Text size='xs'>{dimensionLabel(t, place.dimension)}</Text>
        </Group>
        {spawn && (
          <Group justify='space-between' gap={4}>
            <Text size='xs' c='dimmed'>
              {t('overview.spawn', {})}
            </Text>
            <Text size='xs' ff='monospace'>
              X {spawn[0].toFixed(2)} · Y {spawn[1].toFixed(2)} · Z {spawn[2].toFixed(2)}
            </Text>
          </Group>
        )}
        {overview.level.version && (
          <Text size='xs' c='dimmed'>
            {t('overview.version', { version: overview.level.version })}
          </Text>
        )}
        <Text size='xs' fw={500} mt={4}>
          {t('overview.jump', {})}
        </Text>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (typeof x === 'number' && typeof z === 'number') onJump(x, z);
          }}
        >
          <Group gap={6} wrap='nowrap' align='flex-end'>
            <NumberInput size='xs' placeholder='X' value={x} onChange={setX} hideControls allowDecimal />
            <NumberInput size='xs' placeholder='Z' value={z} onChange={setZ} hideControls allowDecimal />
            <Button size='xs' type='submit' disabled={typeof x !== 'number' || typeof z !== 'number'}>
              {t('overview.jumpButton', {})}
            </Button>
          </Group>
        </form>
        <Group gap={6} wrap='nowrap'>
          <Select
            size='xs'
            label={t('overview.world', {})}
            data={overview.worlds.map((candidate) => candidate.name)}
            value={place.world}
            onChange={(name) => {
              const next = overview.worlds.find((candidate) => candidate.name === name);
              if (next) onPlace({ world: next.name, dimension: next.dimensions[0].id });
            }}
          />
          <Select
            size='xs'
            label={t('overview.dimension', {})}
            data={(world?.dimensions ?? []).map((dimension) => ({
              value: dimension.id,
              label: dimensionLabel(t, dimension.id),
            }))}
            value={place.dimension}
            onChange={(dimension) => dimension && onPlace({ world: place.world, dimension })}
          />
        </Group>
      </Stack>
    </Paper>
  );
}
