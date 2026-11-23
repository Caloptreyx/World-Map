import { faXmark } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import ActionIcon from '@/elements/buttons/ActionIcon.tsx';
import Button from '@/elements/buttons/Button.tsx';
import Progress from '@/elements/feedback/Progress.tsx';
import Group from '@/elements/layout/Group.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import Text from '@/elements/typography/Text.tsx';
import type { ChunkJobs } from './useChunkJob.ts';
import { isFilterResult, type Job, type JobKind } from '../api.ts';
import { type ExtT, useExtTranslations } from '../translations.ts';

export const jobLabel = (t: ExtT, kind: JobKind): string => t(`jobs.kind.${kind}`, {});

/** One line about a job, for the notices over the map while the chunk tools are closed. */
export function jobSummary(t: ExtT, job: Job): string {
  const label = jobLabel(t, job.kind);
  if (job.state === 'running')
    return t('jobs.runningShort', { label, done: job.progress.done, total: job.progress.total });
  return `${label}: ${t(`jobs.state.${job.state}`, {})}`;
}

/** The current job: progress by regions, its message, cancel while running, the outcome after. */
export default function JobProgress({ jobs }: { jobs: ChunkJobs }) {
  const { t } = useExtTranslations();
  const job = jobs.job;
  if (!job) return null;
  const { done, total } = job.progress;
  const result = job.result;
  const errors = result?.errors ?? [];
  return (
    <div className='wm-job' data-state={job.state}>
      <Stack gap={6}>
        <Group justify='space-between' wrap='nowrap' gap={6}>
          <Text size='sm' fw={600}>
            {jobLabel(t, job.kind)} · {t(`jobs.state.${job.state}`, {})}
          </Text>
          {job.state === 'running' ? (
            <Button size='compact-xs' variant='default' onClick={jobs.cancel}>
              {t('jobs.cancel', {})}
            </Button>
          ) : (
            <ActionIcon variant='subtle' size={24} aria-label={t('jobs.dismiss', {})} onClick={jobs.dismiss}>
              <FontAwesomeIcon icon={faXmark} size='xs' />
            </ActionIcon>
          )}
        </Group>
        {job.state === 'running' && (
          <Progress value={total > 0 ? (done / total) * 100 : Number.NaN} hourglass={false} size='md' />
        )}
        <Text className='wm-label'>
          {t('jobs.regions', { done, total })}
          {job.message ? ` · ${job.message}` : ''}
        </Text>
        {job.error && (
          <Text size='xs' c='red'>
            {job.error}
          </Text>
        )}
        {result && !isFilterResult(result) && (
          <Text size='xs'>
            {t('jobs.writeResult', { chunks: result.chunks, skipped: result.skipped, files: result.files })}
          </Text>
        )}
        {result && !isFilterResult(result) && result.operation && (
          <Text className='wm-label'>
            {t('jobs.backupAt', {})} <span className='wm-mono'>{result.operation.backup}</span>
          </Text>
        )}
        {result?.partial && (
          <Text size='xs' c='orange'>
            {t('jobs.partial', {})}
          </Text>
        )}
        {!!result?.failures && (
          <Text size='xs' c='orange'>
            {t('jobs.failures', { count: result.failures })}
          </Text>
        )}
        {errors.length > 0 && (
          <details>
            <summary className='wm-label' style={{ cursor: 'pointer' }}>
              {t('jobs.errors', { count: errors.length })}
            </summary>
            <div className='wm-job-errors'>
              {errors.map((error) => (
                <div key={`${error.region}-${error.error}`} className='wm-mono'>
                  {error.region}: {error.error}
                </div>
              ))}
            </div>
          </details>
        )}
      </Stack>
    </div>
  );
}
