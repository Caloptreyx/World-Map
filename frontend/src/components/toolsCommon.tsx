import Group from '@/elements/layout/Group.tsx';
import SegmentedControl from '@/elements/layout/SegmentedControl.tsx';
import Text from '@/elements/typography/Text.tsx';
import type { OperationKind, Overview, Place } from '../api.ts';
import type { Selection } from '../lib/selection.ts';
import type { ChunkJobs } from './useChunkJob.ts';
import { type ExtT, useExtTranslations } from '../translations.ts';

/** Everything a chunk tools tab works with. */
export interface ToolsProps {
  server: string;
  overview: Overview;
  place: Place;
  selection: Selection;
  onSelection: (selection: Selection) => void;
  /** Regions with any file (region/, entities/ or poi/): what an inverted selection resolves within. */
  existing: [number, number][];
  /** The server is stopped, so files may be written. */
  offline: boolean;
  jobs: ChunkJobs;
}

export type Granularity = 'chunk' | 'region';
export type Brush = 'add' | 'remove';

/** Why a write action cannot run now, else null. `chunks` is null when the count is unknown. */
export function writeProblem(t: ExtT, props: ToolsProps, chunks: number | null): string | null {
  if (!props.offline) return t('tools.offline', {});
  if (props.jobs.running) return t('tools.busy', {});
  if (chunks !== null && chunks === 0) return t('tools.nothingSelected', {});
  if (chunks !== null && chunks > props.overview.limits.operation_chunks) {
    return t('tools.tooMany', { limit: props.overview.limits.operation_chunks });
  }
  return null;
}

/** Where a write operation moves the original files (the server adds the time and an id when it starts). */
export const backupFolder = (overview: Overview, kind: OperationKind): string =>
  `${overview.backups.folder}/${new Date().toISOString().slice(0, 10).replaceAll('-', '')}-<time>-${kind}-<id>/`;

/** Saves a blob through a temporary download link. */
export function downloadBlob(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/** A file name part from a world or dimension id. */
export const fileSafe = (text: string): string => text.replace(/[^A-Za-z0-9._-]+/g, '_');

export function ProblemNote({ problem }: { problem: string | null }) {
  if (!problem) return null;
  return (
    <Text size='xs' c='dimmed'>
      {problem}
    </Text>
  );
}

interface ControlsProps {
  granularity: Granularity;
  /** Zoomed out so far that chunks are too small to pick: regions are selected whatever is chosen. */
  autoRegion: boolean;
  onGranularity: (granularity: Granularity) => void;
  brush: Brush;
  onBrush: (brush: Brush) => void;
}

/** Chunks or regions, and whether drags add or remove. */
export function SelectionControls({ granularity, autoRegion, onGranularity, brush, onBrush }: ControlsProps) {
  const { t } = useExtTranslations();
  return (
    <Group gap={6} wrap='wrap'>
      <SegmentedControl
        size='xs'
        value={autoRegion ? 'region' : granularity}
        onChange={(value) => onGranularity(value as Granularity)}
        data={[
          { value: 'chunk', label: t('selection.modeChunks', {}), disabled: autoRegion },
          { value: 'region', label: t('selection.modeRegions', {}) },
        ]}
        aria-label={t('selection.mode', {})}
      />
      <SegmentedControl
        size='xs'
        value={brush}
        onChange={(value) => onBrush(value as Brush)}
        data={[
          { value: 'add', label: t('selection.brushAdd', {}) },
          { value: 'remove', label: t('selection.brushRemove', {}) },
        ]}
        aria-label={t('selection.brush', {})}
      />
      {autoRegion && <Text className='wm-label'>{t('selection.autoRegion', {})}</Text>}
    </Group>
  );
}
