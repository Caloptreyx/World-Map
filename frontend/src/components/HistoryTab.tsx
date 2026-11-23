import { faRotateLeft } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import Button from '@/elements/buttons/Button.tsx';
import Badge from '@/elements/data-display/Badge.tsx';
import Alert from '@/elements/feedback/Alert.tsx';
import Group from '@/elements/layout/Group.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import ConfirmationModal from '@/elements/modals/ConfirmationModal.tsx';
import Text from '@/elements/typography/Text.tsx';
import { getOperations, type Operation, type OperationKind, type Overview, PACKAGE, restoreOperation } from '../api.ts';
import { useExtTranslations } from '../translations.ts';
import { dimensionLabel } from './OverviewCard.tsx';
import { ProblemNote, type ToolsProps } from './toolsCommon.tsx';

/** Restoring an operation needs the permission of what it did. */
const mayRestore = (overview: Overview, kind: OperationKind): boolean =>
  kind === 'delete'
    ? overview.capabilities.delete_chunks
    : kind === 'change'
      ? overview.capabilities.edit_chunks
      : kind === 'import'
        ? overview.capabilities.import_chunks
        : false;

const STATE_COLOURS: Record<Operation['state'], string> = {
  done: 'green',
  partial: 'orange',
  restored: 'blue',
  pruned: 'gray',
};

/** The write operations of this server, newest first, with their backups and "Restore". */
export default function HistoryTab(props: ToolsProps) {
  const { server, overview, jobs, offline } = props;
  const { t } = useExtTranslations();
  const [restoring, setRestoring] = useState<Operation | null>(null);
  const operations = useQuery({
    queryKey: [PACKAGE, server, 'operations'],
    queryFn: () => getOperations(server),
    refetchInterval: 30_000,
  });
  const problem = !offline ? t('tools.offline', {}) : jobs.running ? t('tools.busy', {}) : null;

  const restore = async () => {
    if (!restoring) return;
    jobs.track(await restoreOperation(server, restoring.id));
    setRestoring(null);
  };

  if (operations.isLoading) return <div className='wm-skeleton' style={{ height: 120 }} aria-busy='true' />;
  if (operations.error) return <Alert color='red'>{httpErrorToHuman(operations.error)}</Alert>;
  const list = operations.data ?? [];

  return (
    <Stack gap='sm'>
      <Text className='wm-label'>
        {t('history.intro', { folder: overview.backups.folder, days: overview.backups.retention_days })}
      </Text>
      {list.length === 0 && <Text size='sm'>{t('history.empty', {})}</Text>}
      {list.map((operation) => {
        const chunks = typeof operation.summary?.chunks === 'number' ? operation.summary.chunks : null;
        const restorable = operation.restorable && mayRestore(overview, operation.kind);
        return (
          <div key={operation.id} className='wm-result'>
            <Stack gap={4}>
              <Group justify='space-between' gap={6} wrap='nowrap'>
                <Text size='sm' fw={600}>
                  {t(`history.kind.${operation.kind}`, {})}
                </Text>
                <Badge size='sm' variant='light' color={STATE_COLOURS[operation.state]}>
                  {t(`history.state.${operation.state}`, {})}
                </Badge>
              </Group>
              <Text className='wm-label'>
                {new Date(operation.created * 1000).toLocaleString()} ·{' '}
                {operation.username ?? t('history.unknownUser', {})} · {operation.world} ·{' '}
                {dimensionLabel(t, operation.dimension)}
              </Text>
              <Text size='xs'>
                {chunks !== null
                  ? t('history.counts', { chunks, files: operation.files })
                  : t('history.files', { files: operation.files })}
              </Text>
              <Text className='wm-mono' style={{ wordBreak: 'break-all', fontSize: 11 }}>
                {operation.backup}
              </Text>
              {restorable && (
                <Group gap={6}>
                  <Button
                    size='compact-xs'
                    variant='default'
                    leftSection={<FontAwesomeIcon icon={faRotateLeft} />}
                    disabled={!!problem}
                    onClick={() => setRestoring(operation)}
                  >
                    {t('history.restore', {})}
                  </Button>
                  <ProblemNote problem={problem} />
                </Group>
              )}
            </Stack>
          </div>
        );
      })}

      <ConfirmationModal
        opened={restoring !== null}
        onClose={() => setRestoring(null)}
        title={t('history.restoreTitle', {})}
        confirm={t('history.restore', {})}
        confirmColor='orange'
        onConfirmed={restore}
      >
        {restoring && (
          <Stack gap='sm'>
            <Text size='sm'>
              {t('history.restoreContent', {
                kind: t(`history.kind.${restoring.kind}`, {}),
                time: new Date(restoring.created * 1000).toLocaleString(),
                files: restoring.files,
              })}
            </Text>
            <Text className='wm-mono' style={{ wordBreak: 'break-all', fontSize: 11 }}>
              {restoring.backup}
            </Text>
            <Alert color='yellow'>{t('history.restoreNote', {})}</Alert>
          </Stack>
        )}
      </ConfirmationModal>
    </Stack>
  );
}
