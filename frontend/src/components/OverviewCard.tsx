import { faArrowRight, faGear } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { useState } from 'react';
import ActionIcon from '@/elements/buttons/ActionIcon.tsx';
import NumberInput from '@/elements/input/NumberInput.tsx';
import Select from '@/elements/input/Select.tsx';
import Group from '@/elements/layout/Group.tsx';
import SegmentedControl from '@/elements/layout/SegmentedControl.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import Tooltip from '@/elements/overlays/Tooltip.tsx';
import Text from '@/elements/typography/Text.tsx';
import type { Overview, Place, World } from '../api.ts';
import { mainProblem, VANILLA_DIMENSIONS } from '../lib/worlds.ts';
import { type ExtT, useExtTranslations } from '../translations.ts';

/** A dimension's name: vanilla ones translated, datapack and mod ones as their id. */
export function dimensionLabel(t: ExtT, id: string): string {
  switch (id) {
    case 'minecraft:overworld':
      return t('dimensions.overworld', {});
    case 'minecraft:the_nether':
      return t('dimensions.nether', {});
    case 'minecraft:the_end':
      return t('dimensions.end', {});
    default:
      return id;
  }
}

/** A world's name: its folder, the main one marked. */
export function worldLabel(t: ExtT, world: World): string {
  return world.main ? t('overview.mainWorld', { name: world.name }) : world.name;
}

/** Where the map is: the world (when there are several) and the dimension. */
export function placeLabel(t: ExtT, overview: Overview, place: Place): string {
  const world = overview.worlds.find((candidate) => candidate.name === place.world);
  const dimension = dimensionLabel(t, place.dimension);
  return overview.worlds.length > 1 && world ? `${world.name} · ${dimension}` : dimension;
}

interface Props {
  overview: Overview;
  place: Place;
  regions: number | null;
  onPlace: (place: Place) => void;
  onJump: (x: number, z: number) => void;
  /** Opens the world folder settings; absent without the permission. */
  onFolders?: () => void;
}

/** The world and dimension, what the level says about them, and jumping to coordinates. */
export default function OverviewContent({ overview, place, regions, onPlace, onJump, onFolders }: Props) {
  const { t } = useExtTranslations();
  const [x, setX] = useState<number | string>('');
  const [z, setZ] = useState<number | string>('');
  const world = overview.worlds.find((candidate) => candidate.name === place.world);
  const dimensions = world?.dimensions ?? [];
  const spawn = world?.level.spawn ?? null;
  const vanillaOnly =
    dimensions.length > 1 && dimensions.every((dimension) => VANILLA_DIMENSIONS.includes(dimension.id));
  const problem = mainProblem(overview);
  const dimensionData = dimensions.map((dimension) => ({
    value: dimension.id,
    label: dimensionLabel(t, dimension.id),
  }));

  return (
    <Stack gap='sm'>
      <Text className='wm-label'>{t('overview.description', {})}</Text>
      {problem && (
        <Text size='xs' c='yellow'>
          {t(problem === 'missing' ? 'overview.levelMissing' : 'overview.levelEmpty', { name: overview.level_name })}
        </Text>
      )}
      <Group gap={6} wrap='nowrap'>
        <Select
          size='sm'
          style={{ flex: 1 }}
          aria-label={t('overview.world', {})}
          data={overview.worlds.map((candidate) => ({ value: candidate.name, label: worldLabel(t, candidate) }))}
          value={place.world}
          allowDeselect={false}
          searchable={overview.worlds.length > 8}
          onChange={(name) => {
            const next = overview.worlds.find((candidate) => candidate.name === name);
            if (next) onPlace({ world: next.name, dimension: next.dimensions[0].id });
          }}
        />
        {onFolders && (
          <Tooltip label={t('overview.folders', {})} withinPortal>
            <ActionIcon size={36} variant='light' aria-label={t('overview.folders', {})} onClick={onFolders}>
              <FontAwesomeIcon icon={faGear} />
            </ActionIcon>
          </Tooltip>
        )}
      </Group>
      {vanillaOnly ? (
        <SegmentedControl
          fullWidth
          size='xs'
          aria-label={t('overview.dimension', {})}
          data={dimensionData}
          value={place.dimension}
          onChange={(dimension) => onPlace({ world: place.world, dimension })}
        />
      ) : (
        <Select
          size='sm'
          aria-label={t('overview.dimension', {})}
          data={dimensionData}
          value={place.dimension}
          allowDeselect={false}
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
        {world?.level.version && (
          <>
            <Text className='wm-label'>{t('overview.savedBy', {})}</Text>
            <span className='wm-mono'>{world.level.version}</span>
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
