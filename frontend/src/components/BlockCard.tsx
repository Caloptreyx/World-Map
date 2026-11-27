import type { CSSProperties } from 'react';
import { faCopy, faLocationArrow, faXmark } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import Button from '@/elements/buttons/Button.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import Text from '@/elements/typography/Text.tsx';
import type { BlockInfo } from '../api.ts';
import { chunkOf, regionOf } from '../lib/geometry.ts';
import { useExtTranslations } from '../translations.ts';

/** A clicked column: where it is on screen, and its details once the block endpoint answered. */
export interface BlockCardState {
  x: number;
  /** The column's top block as the loaded surface has it, until `info` arrives. */
  y: number;
  z: number;
  /** The click, in pixels inside the map. */
  left: number;
  top: number;
  info: BlockInfo | null;
  error: string | null;
}

/** A block or biome id as words: `minecraft:grass_block` → `Grass Block`. */
export const idLabel = (id: string): string =>
  (id.split(':').pop() ?? id)
    .split('_')
    .map((word) => (word ? word[0].toUpperCase() + word.slice(1) : word))
    .join(' ');

interface Props {
  card: BlockCardState;
  style: CSSProperties;
  /** Shown when the selected player can be teleported onto the block. */
  teleportName: string | null;
  onCopy: (text: string) => void;
  /** Called with the block the player should stand on: one above the top block. */
  onTeleport: (x: number, y: number, z: number) => void;
  onClose: () => void;
}

/** What the 3D view knows about a clicked block, with Copy coordinates and Teleport here. */
export default function BlockCard({ card, style, teleportName, onCopy, onTeleport, onClose }: Props) {
  const { t } = useExtTranslations();
  const { info } = card;
  const x = info?.x ?? card.x;
  const y = info?.y ?? card.y;
  const z = info?.z ?? card.z;

  return (
    <div
      className='wm-glass wm-block-card'
      style={style}
      role='dialog'
      aria-label={info ? idLabel(info.block) : t('map3d.block.loading', {})}
    >
      <header className='wm-block-card-title'>
        <div style={{ flex: 1, minWidth: 0 }}>
          <Text size='sm' fw={600} truncate>
            {info ? idLabel(info.block) : card.error ? '-' : t('map3d.block.loading', {})}
          </Text>
          {info && <div className='wm-mono wm-block-card-id'>{info.block}</div>}
        </div>
        <button
          type='button'
          className='wm-tool'
          style={{ width: 28, height: 28, fontSize: 12 }}
          aria-label={t('map3d.block.close', {})}
          onClick={onClose}
        >
          <FontAwesomeIcon icon={faXmark} />
        </button>
      </header>
      {card.error && (
        <Text size='xs' c='dimmed' mt={6}>
          {card.error}
        </Text>
      )}
      <div className='wm-facts' style={{ marginTop: 8 }}>
        {info && (
          <>
            <Text className='wm-label'>{t('map3d.block.biome', {})}</Text>
            <span className='wm-mono'>{info.biome ? idLabel(info.biome) : t('map3d.block.unknown', {})}</span>
          </>
        )}
        {info && info.water_depth > 0 && (
          <>
            <Text className='wm-label'>{t('map3d.block.water', {})}</Text>
            <span className='wm-mono'>
              {t('map3d.block.waterDepth', {
                depth: info.water_depth,
                floor: info.floor_block ? idLabel(info.floor_block) : t('map3d.block.unknown', {}),
              })}
            </span>
          </>
        )}
        <Text className='wm-label'>{t('map3d.block.position', {})}</Text>
        <span className='wm-mono'>
          {x} / {y} / {z}
        </span>
      </div>
      <div className='wm-mono' style={{ color: 'var(--mantine-color-dimmed)', fontSize: 11, marginTop: 4 }}>
        {t('cursor.chunk', { x: chunkOf(x), z: chunkOf(z) })} · {t('cursor.region', { x: regionOf(x), z: regionOf(z) })}
      </div>
      <Stack gap={2} mt={8}>
        <Button
          size='xs'
          variant='subtle'
          justify='flex-start'
          leftSection={<FontAwesomeIcon icon={faCopy} />}
          onClick={() => onCopy(`${x} ${y} ${z}`)}
        >
          {t('map.copyCoordinates', {})}
        </Button>
        {teleportName && (
          <Button
            size='xs'
            variant='subtle'
            justify='flex-start'
            leftSection={<FontAwesomeIcon icon={faLocationArrow} />}
            onClick={() => onTeleport(x, y + 1, z)}
          >
            {t('map.teleportHere', { name: teleportName })}
          </Button>
        )}
      </Stack>
    </div>
  );
}
