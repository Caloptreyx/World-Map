import { faPlay } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import Button from '@/elements/buttons/Button.tsx';
import Alert from '@/elements/feedback/Alert.tsx';
import Checkbox from '@/elements/input/Checkbox.tsx';
import TextArea from '@/elements/input/TextArea.tsx';
import Group from '@/elements/layout/Group.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import ConfirmationModal from '@/elements/modals/ConfirmationModal.tsx';
import Anchor from '@/elements/typography/Anchor.tsx';
import Text from '@/elements/typography/Text.tsx';
import { checkFilter } from '../api.ts';
import { type FilterNode, parseQuery, toQuery } from '../lib/filter.ts';
import { fromWire, isEmpty, merge, toWire } from '../lib/selection.ts';
import { useExtTranslations } from '../translations.ts';
import FilterBuilder, { emptyGroup, type GroupNode } from './FilterBuilder.tsx';
import { placeLabel } from './OverviewCard.tsx';
import { backupFolder, ProblemNote, type ToolsProps, writeProblem } from './toolsCommon.tsx';

export const FILTER_WIKI = 'https://github.com/Querz/mcaselector/wiki/Chunk-Filter';

interface QueryError {
  error: string;
  position: number;
}

const asGroup = (node: FilterNode): GroupNode =>
  node.kind === 'group' ? node : { ...emptyGroup(), children: [{ ...node, operator: 'AND' }] };

/** The query with a marker where it went wrong. */
function ErrorAt({ text, error }: { text: string; error: QueryError }) {
  const { t } = useExtTranslations();
  const position = Math.max(0, Math.min(text.length, error.position));
  const start = Math.max(0, position - 24);
  return (
    <Alert color='red' p='xs'>
      <Text size='xs'>{t('filter.errorAt', { position: position + 1, error: error.error })}</Text>
      {text && (
        <div className='wm-mono wm-query-error'>
          {start > 0 ? '…' : ''}
          {text.slice(start, position)}
          <mark>{text.slice(position, position + 1) || ' '}</mark>
          {text.slice(position + 1, position + 24)}
          {position + 24 < text.length ? '…' : ''}
        </div>
      )}
    </Alert>
  );
}

/** MCA Selector's chunk filter: build or type a query, scan the dimension (or the selection), then
 * select or delete what matched. */
export default function FilterTab(props: ToolsProps) {
  const { server, overview, place, selection, onSelection, jobs } = props;
  const { t } = useExtTranslations();
  const [root, setRoot] = useState<GroupNode>(emptyGroup);
  const [text, setText] = useState('');
  const [error, setError] = useState<QueryError | null>(null);
  const [onlySelection, setOnlySelection] = useState(false);
  const [checking, setChecking] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const fromBuilder = (next: GroupNode) => {
    setRoot(next);
    setText(toQuery(next));
    setError(null);
  };
  const fromText = (next: string) => {
    setText(next);
    if (!next.trim()) {
      setRoot(emptyGroup());
      setError(null);
      return;
    }
    const parsed = parseQuery(next);
    if (parsed.ok) {
      setRoot(asGroup(parsed.root));
      setError(null);
    } else {
      setError({ error: parsed.error, position: parsed.position });
    }
  };

  const run = async () => {
    setChecking(true);
    try {
      const checked = await checkFilter(server, text);
      if (!checked.ok) {
        setError({ error: checked.error, position: checked.position });
        return;
      }
      jobs.clearFilterResult();
      await jobs.start({
        kind: 'filter',
        world: place.world,
        dimension: place.dimension,
        query: checked.normalized,
        selection: onlySelection ? toWire(selection) : null,
      });
    } catch (failure) {
      setError({ error: httpErrorToHuman(failure), position: 0 });
    } finally {
      setChecking(false);
    }
  };

  const found = jobs.filterResult;
  const result =
    found && found.place.world === place.world && found.place.dimension === place.dimension ? found.result : null;
  const deleteProblem = result ? writeProblem(t, props, result.matched) : null;

  const runDelete = async () => {
    if (!result) return;
    await jobs.start({ kind: 'delete', world: place.world, dimension: place.dimension, selection: result.selection });
    setConfirmDelete(false);
  };

  return (
    <Stack gap='sm'>
      <Text className='wm-label'>
        {t('filter.intro', {})}{' '}
        <Anchor href={FILTER_WIKI} target='_blank' rel='noreferrer' size='xs'>
          {t('filter.wiki', {})}
        </Anchor>
      </Text>
      <FilterBuilder root={root} onChange={fromBuilder} />
      <TextArea
        label={t('filter.query', {})}
        description={t('filter.queryHint', {})}
        placeholder='InhabitedTime < "5 minutes" AND Status = "full"'
        autosize
        minRows={2}
        maxRows={6}
        value={text}
        onChange={(event) => fromText(event.currentTarget.value)}
        styles={{ input: { fontFamily: 'var(--mantine-font-family-monospace)', fontSize: 12 } }}
        spellCheck={false}
      />
      {error && <ErrorAt text={text} error={error} />}
      <Checkbox
        label={t('filter.onlySelection', {})}
        checked={onlySelection}
        disabled={isEmpty(selection)}
        onChange={(event) => setOnlySelection(event.currentTarget.checked)}
      />
      <Group gap={6}>
        <Button
          size='xs'
          leftSection={<FontAwesomeIcon icon={faPlay} />}
          loading={checking}
          disabled={!text.trim() || !!error || jobs.running}
          onClick={run}
        >
          {t('filter.run', {})}
        </Button>
        {jobs.running && <Text className='wm-label'>{t('tools.busy', {})}</Text>}
      </Group>

      {result && (
        <div className='wm-result'>
          <Stack gap={6}>
            <Text size='sm' fw={600}>
              {t('filter.matched', { count: result.matched, regions: result.regions_read })}
            </Text>
            {result.failures > 0 && (
              <Text size='xs' c='orange'>
                {t('filter.regionErrors', { count: result.failures })}
              </Text>
            )}
            {result.partial && (
              <Text size='xs' c='orange'>
                {t('filter.partial', {})}
              </Text>
            )}
            <Group gap={6}>
              <Button
                size='xs'
                variant='default'
                disabled={result.matched === 0}
                onClick={() => onSelection(fromWire(result.selection))}
              >
                {t('filter.select', {})}
              </Button>
              <Button
                size='xs'
                variant='default'
                disabled={result.matched === 0}
                onClick={() => onSelection(merge(selection, fromWire(result.selection)))}
              >
                {t('filter.addSelection', {})}
              </Button>
              {overview.capabilities.delete_chunks && (
                <Button size='xs' color='red' disabled={!!deleteProblem} onClick={() => setConfirmDelete(true)}>
                  {t('filter.delete', {})}
                </Button>
              )}
            </Group>
            {overview.capabilities.delete_chunks && result.matched > 0 && <ProblemNote problem={deleteProblem} />}
          </Stack>
        </div>
      )}

      <ConfirmationModal
        opened={confirmDelete}
        onClose={() => setConfirmDelete(false)}
        title={t('filter.deleteTitle', {})}
        confirm={t('filter.delete', {})}
        onConfirmed={runDelete}
      >
        <Stack gap='sm'>
          <Text size='sm'>
            {t('filter.deleteContent', { count: result?.matched ?? 0, place: placeLabel(t, overview, place) })}
          </Text>
          <Text size='xs' className='wm-mono'>
            {text}
          </Text>
          <Alert color='yellow'>{t('tools.backupNote', { folder: backupFolder(overview, 'delete') })}</Alert>
        </Stack>
      </ConfirmationModal>
    </Stack>
  );
}
