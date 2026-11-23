import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import { useToast } from '@/providers/ToastProvider.tsx';
import {
  cancelJob,
  createJob,
  type FilterResult,
  getJob,
  getJobs,
  isFilterResult,
  type Job,
  type JobRequest,
  PACKAGE,
  type Place,
} from '../api.ts';
import { useExtTranslations } from '../translations.ts';

/** What happens once a job finished successfully, besides refreshing the map. */
export interface JobOptions {
  onDone?: (job: Job) => void;
}

export interface ChunkJobs {
  /** The job shown: running, or the last one finished (until dismissed). */
  job: Job | null;
  running: boolean;
  /** The result of the last finished filter job, and the world and dimension it scanned. */
  filterResult: { place: Place; result: FilterResult } | null;
  start: (request: JobRequest, options?: JobOptions) => Promise<void>;
  /** Follows a job created elsewhere (a restore). */
  track: (job: Job, options?: JobOptions) => void;
  cancel: () => Promise<void>;
  dismiss: () => void;
  clearFilterResult: () => void;
}

/** Background jobs of the chunk tools: one at a time, polled every second while running. After a
 * job that wrote files, the map's regions (and so its tiles, which follow the region files),
 * entities, overlays and the operation history are reloaded. */
export function useChunkJobs(server: string): ChunkJobs {
  const { t } = useExtTranslations();
  const { addToast } = useToast();
  const queryClient = useQueryClient();
  const [id, setId] = useState<string | null>(null);
  const [filterResult, setFilterResult] = useState<{ place: Place; result: FilterResult } | null>(null);
  const options = useRef(new Map<string, JobOptions>());
  const handled = useRef(new Set<string>());

  const query = useQuery({
    queryKey: [PACKAGE, server, 'job', id],
    queryFn: () => getJob(server, id!),
    enabled: !!id,
    refetchInterval: (current) => (current.state.data?.state === 'running' ? 1000 : false),
  });
  const job = id ? (query.data ?? null) : null;
  const running = job?.state === 'running';

  // a job still running from before a reload (or started in another tab) is followed again
  useEffect(() => {
    let cancelled = false;
    getJobs(server)
      .then((jobs) => {
        const active = jobs.find((candidate) => candidate.state === 'running');
        if (!cancelled && active) {
          queryClient.setQueryData([PACKAGE, server, 'job', active.id], active);
          setId((current) => current ?? active.id);
        }
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [server]);

  useEffect(() => {
    if (!job || job.state === 'running' || handled.current.has(job.id)) return;
    handled.current.add(job.id);
    const extra = options.current.get(job.id);
    options.current.delete(job.id);
    if (job.kind === 'filter') {
      if (job.state === 'done' && isFilterResult(job.result)) {
        setFilterResult({ place: { world: job.world, dimension: job.dimension }, result: job.result });
      }
    } else {
      for (const key of ['regions', 'entities', 'overlay', 'operations']) {
        queryClient.invalidateQueries({ queryKey: [PACKAGE, server, key] });
      }
    }
    if (job.state === 'done') {
      if (job.kind !== 'filter' && job.result && !isFilterResult(job.result)) {
        addToast(t('jobs.writeDone', { chunks: job.result.chunks, files: job.result.files }), 'success');
      }
      extra?.onDone?.(job);
    } else if (job.state === 'failed') {
      addToast(t('jobs.failedToast', { error: job.error ?? '' }), 'error');
    }
  }, [job]);

  const track = (next: Job, extra?: JobOptions) => {
    if (extra) options.current.set(next.id, extra);
    queryClient.setQueryData([PACKAGE, server, 'job', next.id], next);
    setId(next.id);
  };

  return {
    job,
    running,
    filterResult,
    start: async (request, extra) => {
      try {
        track(await createJob(server, request), extra);
      } catch (error) {
        addToast(httpErrorToHuman(error), 'error');
      }
    },
    track,
    cancel: async () => {
      if (!job) return;
      try {
        queryClient.setQueryData([PACKAGE, server, 'job', job.id], await cancelJob(server, job.id));
      } catch (error) {
        addToast(httpErrorToHuman(error), 'error');
      }
    },
    dismiss: () => {
      if (!running) setId(null);
    },
    clearFilterResult: () => setFilterResult(null),
  };
}
