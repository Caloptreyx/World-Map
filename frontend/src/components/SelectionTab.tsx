import { faClipboard, faCopy, faFileExport, faFileImport, faPaste, faTrash } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { useRef, useState } from 'react';
import Button from '@/elements/buttons/Button.tsx';
import Badge from '@/elements/data-display/Badge.tsx';
import Alert from '@/elements/feedback/Alert.tsx';
import Group from '@/elements/layout/Group.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import ConfirmationModal from '@/elements/modals/ConfirmationModal.tsx';
import { Modal, ModalFooter } from '@/elements/modals/Modal.tsx';
import Text from '@/elements/typography/Text.tsx';
import { useToast } from '@/providers/ToastProvider.tsx';
import {
  count,
  emptySelection,
  fromCsv,
  invert,
  isEmpty,
  merge,
  type Selection,
  selectedRegions,
  selectionText,
  subtract,
  toCsv,
  toWire,
} from '../lib/selection.ts';
import { useExtTranslations } from '../translations.ts';
import { placeLabel } from './OverviewCard.tsx';
import { backupFolder, downloadBlob, fileSafe, ProblemNote, type ToolsProps, writeProblem } from './toolsCommon.tsx';

/** The selection: how much it holds, select all / invert / clear, CSV in and out, delete. */
export default function SelectionTab(props: ToolsProps & { controls: React.ReactNode }) {
  const { overview, place, selection, onSelection, existing } = props;
  const { t } = useExtTranslations();
  const { addToast } = useToast();
  const [pending, setPending] = useState<Selection | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);
  const chunks = count(selection, existing);
  const regions = selectedRegions(selection, existing);
  const empty = isEmpty(selection);
  const deleteProblem = writeProblem(t, props, chunks);

  // a loaded selection replaces an empty one; otherwise ask (MCA Selector: overwrite or merge)
  const offer = (loaded: Selection) => {
    if (empty) {
      onSelection(loaded);
      addToast(t('selection.loaded', { count: count(loaded, existing) }), 'success');
    } else {
      setPending(loaded);
    }
  };
  const load = (text: string) => {
    try {
      offer(fromCsv(text));
    } catch (error) {
      addToast(t('selection.csvError', { error: error instanceof Error ? error.message : String(error) }), 'error');
    }
  };

  const copyText = async (text: string, message: string) => {
    try {
      await navigator.clipboard.writeText(text);
      addToast(message, 'success');
    } catch {
      addToast(t('selection.clipboardFailed', {}), 'error');
    }
  };

  const paste = async () => {
    let text: string;
    try {
      text = await navigator.clipboard.readText();
    } catch {
      addToast(t('selection.clipboardFailed', {}), 'error');
      return;
    }
    load(text);
  };

  const runDelete = async () => {
    await props.jobs.start(
      { kind: 'delete', world: place.world, dimension: place.dimension, selection: toWire(selection) },
      { onDone: () => onSelection(emptySelection()) },
    );
    setConfirmDelete(false);
  };

  return (
    <Stack gap='sm'>
      <Group gap={8} wrap='wrap'>
        <Text size='sm' fw={600}>
          {t('selection.count', { count: chunks })}
        </Text>
        <Badge size='sm' variant='light' color='gray'>
          {t('selection.regions', { count: regions })}
        </Badge>
        {selection.inverted && (
          <Badge size='sm' variant='light' color='violet'>
            {t('selection.inverted', {})}
          </Badge>
        )}
      </Group>
      {props.controls}
      <Group gap={6}>
        <Button size='xs' variant='default' onClick={() => onSelection(invert(emptySelection()))}>
          {t('selection.selectAll', {})}
        </Button>
        <Button size='xs' variant='default' onClick={() => onSelection(invert(selection))}>
          {t('selection.invert', {})}
        </Button>
        <Button size='xs' variant='subtle' disabled={empty} onClick={() => onSelection(emptySelection())}>
          {t('selection.clear', {})}
        </Button>
      </Group>

      <Text className='wm-label'>{t('selection.csvTitle', {})}</Text>
      <Group gap={6}>
        <Button
          size='xs'
          variant='default'
          disabled={empty}
          leftSection={<FontAwesomeIcon icon={faClipboard} />}
          onClick={() => copyText(toCsv(selection), t('selection.csvCopied', {}))}
        >
          {t('selection.copyCsv', {})}
        </Button>
        <Button size='xs' variant='default' leftSection={<FontAwesomeIcon icon={faPaste} />} onClick={paste}>
          {t('selection.pasteCsv', {})}
        </Button>
        <Button
          size='xs'
          variant='default'
          disabled={empty}
          leftSection={<FontAwesomeIcon icon={faFileExport} />}
          onClick={() =>
            downloadBlob(
              new Blob([toCsv(selection)], { type: 'text/csv' }),
              `${fileSafe(place.world)}-${fileSafe(place.dimension)}-selection.csv`,
            )
          }
        >
          {t('selection.exportCsv', {})}
        </Button>
        <Button
          size='xs'
          variant='default'
          leftSection={<FontAwesomeIcon icon={faFileImport} />}
          onClick={() => fileInput.current?.click()}
        >
          {t('selection.importCsv', {})}
        </Button>
        <input
          ref={fileInput}
          type='file'
          accept='.csv,text/csv,text/plain'
          hidden
          onChange={async (event) => {
            const file = event.currentTarget.files?.[0];
            event.currentTarget.value = '';
            if (file) load(await file.text());
          }}
        />
      </Group>
      <Group gap={6}>
        <Button
          size='xs'
          variant='default'
          disabled={empty || selection.inverted}
          leftSection={<FontAwesomeIcon icon={faCopy} />}
          onClick={() => copyText(selectionText(selection), t('selection.coordinatesCopied', {}))}
        >
          {t('selection.copy', {})}
        </Button>
        {overview.capabilities.delete_chunks && (
          <Button
            size='xs'
            color='red'
            disabled={!!deleteProblem}
            leftSection={<FontAwesomeIcon icon={faTrash} />}
            onClick={() => setConfirmDelete(true)}
          >
            {t('selection.delete', {})}
          </Button>
        )}
      </Group>
      {overview.capabilities.delete_chunks && !empty && <ProblemNote problem={deleteProblem} />}

      <Modal opened={pending !== null} onClose={() => setPending(null)} title={t('selection.loadTitle', {})}>
        <Text size='sm'>
          {t('selection.loadContent', { count: pending ? count(pending, existing) : 0, current: chunks })}
        </Text>
        <ModalFooter>
          <Button
            onClick={() => {
              if (pending) onSelection(pending);
              setPending(null);
            }}
          >
            {t('selection.loadReplace', {})}
          </Button>
          <Button
            variant='default'
            onClick={() => {
              if (pending) onSelection(merge(selection, pending));
              setPending(null);
            }}
          >
            {t('selection.loadMerge', {})}
          </Button>
          <Button
            variant='default'
            onClick={() => {
              if (pending) onSelection(subtract(selection, pending));
              setPending(null);
            }}
          >
            {t('selection.loadSubtract', {})}
          </Button>
        </ModalFooter>
      </Modal>

      <ConfirmationModal
        opened={confirmDelete}
        onClose={() => setConfirmDelete(false)}
        title={t('selection.deleteTitle', {})}
        confirm={t('selection.delete', {})}
        onConfirmed={runDelete}
      >
        <Stack gap='sm'>
          <Text size='sm'>
            {t('selection.deleteContent', { count: chunks, regions, place: placeLabel(t, overview, place) })}
          </Text>
          <Alert color='yellow'>{t('tools.backupNote', { folder: backupFolder(overview, 'delete') })}</Alert>
        </Stack>
      </ConfirmationModal>
    </Stack>
  );
}
