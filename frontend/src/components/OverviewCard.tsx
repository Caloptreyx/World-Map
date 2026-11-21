import { faArrowRight } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { useState } from 'react';
import ActionIcon from '@/elements/buttons/ActionIcon.tsx';
import NumberInput from '@/elements/input/NumberInput.tsx';
import Select from '@/elements/input/Select.tsx';
import Group from '@/elements/layout/Group.tsx';
import SegmentedControl from '@/elements/layout/SegmentedControl.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import Text from '@/elements/typography/Text.tsx';
import type { Overview, Place } from '../api.ts';
import { type ExtT, useExtTranslations } from '../translations.ts';

const VANILLA = ['minecraft:overworld', 'minecraft:the_nether', 'minecraft:the_end'];

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

interface Props {
  overview: Overview;
  place: Place;
  regions: number | null;
  onPlace: (place: Place) => void;
  onJump: (x: number, z: number) => void;
}

/** The world and dimension, what the level says about them, and jumping to coordinates. */
export default function OverviewContent({ overview, place, regions, onPlace, onJump }: Props) {
  const { t } = useExtTranslations();
  const [x, setX] = useState<number | string>('');
  const [z, setZ] = useState<number | string>('');
  const world = overview.worlds.find((candidate) => candidate.name === place.world);
  const dimensions = world?.dimensions ?? [];
  const spawn = overview.level.spawn;
  const vanillaOnly = dimensions.length > 1 && dimensions.every((dimension) => VANILLA.includes(dimension.id));

  return (
    <Stack gap='sm'>
      <Text className='wm-label'>{t('overview.description', {})}</Text>
      {overview.worlds.length > 1 && (
        <Select
          size='sm'
          aria-label={t('overview.world', {})}
          data={overview.worlds.map((candidate) => candidate.name)}
          value={place.world}
          onChange={(name) => {
            const next = overview.worlds.find((candidate) => candidate.name === name);
            if (next) onPlace({ world: next.name, dimension: next.dimensions[0].id });
          }}
        />
      )}
      {vanillaOnly ? (
        <SegmentedControl
          fullWidth
          size='xs'
          aria-label={t('overview.dimension', {})}
          data={dimensions.map((dimension) => ({ value: dimension.id, label: dimensionLabel(t, dimension.id) }))}
          value={place.dimension}
          onChange={(dimension) => onPlace({ world: place.world, dimension })}
        />
      ) : (
        <Select
          size='sm'
          aria-label={t('overview.dimension', {})}
          data={dimensions.map((dimension) => ({ value: dimension.id, label: dimensionLabel(t, dimension.id) }))}
          value={place.dimension}
          onChange={(dimension) => dimension && onPlace({ world: place.world, dimension })}
        />
      )}
      <div className='wm-facts'>
        {spawn && (
          <>
            <Text className='wm-label'>{t('overview.spawn', {})}</Text>
            <span className='wm-mono'>
              {spawn[0].toFixed(0)}, {spawn[1].toFixed(0)}, {spawn[2].toFixed(0)}
            </span>
          </>
        )}
        {overview.level.version && (
          <>
            <Text className='wm-label'>{t('overview.savedBy', {})}</Text>
            <span className='wm-mono'>{overview.level.version}</span>
          </>
        )}
        {regions !== null && (
          <>
            <Text className='wm-label'>{t('overview.regions', {})}</Text>
            <span className='wm-mono'>{regions}</span>
          </>
        )}
      </div>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (typeof x === 'number' && typeof z === 'number') onJump(x, z);
        }}
      >
        <Text className='wm-label' mb={4}>
          {t('overview.jump', {})}
        </Text>
        <Group gap={6} wrap='nowrap'>
          <NumberInput size='sm' style={{ flex: 1 }} placeholder='X' value={x} onChange={setX} hideControls />
          <NumberInput size='sm' style={{ flex: 1 }} placeholder='Z' value={z} onChange={setZ} hideControls />
          <ActionIcon
            type='submit'
            size={36}
            variant='light'
            aria-label={t('overview.jumpButton', {})}
            disabled={typeof x !== 'number' || typeof z !== 'number'}
          >
            <FontAwesomeIcon icon={faArrowRight} />
          </ActionIcon>
        </Group>
      </form>
    </Stack>
  );
}
