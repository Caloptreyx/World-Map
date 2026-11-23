import { faDownload, faFileZipper, faUpload } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { useRef, useState } from 'react';
import { axiosInstance, httpErrorToHuman } from '@/api/axios.ts';
import getFileUploadUrl from '@/api/server/files/getFileUploadUrl.ts';
import Button from '@/elements/buttons/Button.tsx';
import Alert from '@/elements/feedback/Alert.tsx';
import Progress from '@/elements/feedback/Progress.tsx';
import Checkbox from '@/elements/input/Checkbox.tsx';
import NumberInput from '@/elements/input/NumberInput.tsx';
import Select from '@/elements/input/Select.tsx';
import Switch from '@/elements/input/Switch.tsx';
import TextInput from '@/elements/input/TextInput.tsx';
import Divider from '@/elements/layout/Divider.tsx';
import Group from '@/elements/layout/Group.tsx';
import SegmentedControl from '@/elements/layout/SegmentedControl.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import ConfirmationModal from '@/elements/modals/ConfirmationModal.tsx';
import Anchor from '@/elements/typography/Anchor.tsx';
import Text from '@/elements/typography/Text.tsx';
import { useServerCan } from '@/plugins/usePermissions.ts';
import { useToast } from '@/providers/ToastProvider.tsx';
import { createImport, exportChunks, extractImport, type JobRequest, type Offset, type Place } from '../api.ts';
import { count, isEmpty, toWire } from '../lib/selection.ts';
import { type ExtT, useExtTranslations } from '../translations.ts';
import { dimensionLabel, placeLabel, worldLabel } from './OverviewCard.tsx';
import { backupFolder, downloadBlob, fileSafe, ProblemNote, type ToolsProps, writeProblem } from './toolsCommon.tsx';

export const IMPORT_WIKI = 'https://github.com/Querz/mcaselector/wiki/Chunk-Import';

type ImportRequest = Extract<JobRequest, { kind: 'import' }>;

interface MoveOptions {
  offset: Offset;
  overwrite: boolean;
  sections: string;
}

const defaultOptions = (): MoveOptions => ({ offset: { x: 0, y: 0, z: 0 }, overwrite: false, sections: '' });

/** World and dimension pickers over the overview's worlds. */
function PlacePicker({
  props,
  value,
  onChange,
  label,
}: {
  props: ToolsProps;
  value: Place;
  onChange: (place: Place) => void;
  label: string;
}) {
  const { t } = useExtTranslations();
  const world = props.overview.worlds.find((candidate) => candidate.name === value.world);
  return (
    <Group gap={6} grow wrap='wrap'>
      <Select
        size='xs'
        label={label}
        data={props.overview.worlds.map((candidate) => ({ value: candidate.name, label: worldLabel(t, candidate) }))}
        value={value.world}
        onChange={(name) => {
          const next = props.overview.worlds.find((candidate) => candidate.name === name);
          if (next) onChange({ world: next.name, dimension: next.dimensions[0]?.id ?? value.dimension });
        }}
      />
      <Select
        size='xs'
        label={t('overview.dimension', {})}
        data={(world?.dimensions ?? []).map((dimension) => ({
          value: dimension.id,
          label: dimensionLabel(t, dimension.id),
        }))}
        value={value.dimension}
        onChange={(dimension) => dimension && onChange({ ...value, dimension })}
      />
    </Group>
  );
}

/** Offsets (x/z in chunks, y in sections, with the block distance), section ranges and overwrite. */
function MoveFields({ value, onChange }: { value: MoveOptions; onChange: (options: MoveOptions) => void }) {
  const { t } = useExtTranslations();
  const setOffset = (axis: keyof Offset, next: number | string) =>
    onChange({ ...value, offset: { ...value.offset, [axis]: typeof next === 'number' ? Math.trunc(next) : 0 } });
  return (
    <Stack gap={6}>
      <Group gap={6} grow wrap='wrap'>
        <NumberInput
          size='xs'
          label={t('copy.offsetX', {})}
          description={t('copy.blocks', { blocks: value.offset.x * 16 })}
          allowDecimal={false}
          value={value.offset.x}
          onChange={(next) => setOffset('x', next)}
        />
        <NumberInput
          size='xs'
          label={t('copy.offsetZ', {})}
          description={t('copy.blocks', { blocks: value.offset.z * 16 })}
          allowDecimal={false}
          value={value.offset.z}
          onChange={(next) => setOffset('z', next)}
        />
        <NumberInput
          size='xs'
          label={t('copy.offsetY', {})}
          description={t('copy.blocks', { blocks: value.offset.y * 16 })}
          allowDecimal={false}
          value={value.offset.y}
          onChange={(next) => setOffset('y', next)}
        />
      </Group>
      <TextInput
        size='xs'
        label={t('copy.sections', {})}
        description={t('copy.sectionsHint', {})}
        placeholder='-4:3,10'
        value={value.sections}
        onChange={(event) => onChange({ ...value, sections: event.currentTarget.value })}
      />
      <Switch
        label={t('copy.overwrite', {})}
        description={t('copy.overwriteHint', {})}
        checked={value.overwrite}
        onChange={(event) => onChange({ ...value, overwrite: event.currentTarget.checked })}
      />
    </Stack>
  );
}

const sameOffset = (offset: Offset) => offset.x === 0 && offset.y === 0 && offset.z === 0;

const offsetText = (t: ExtT, offset: Offset) => t('copy.offsetText', { x: offset.x, y: offset.y, z: offset.z });

/** Copy chunks between worlds and dimensions, import region files, download the selection. */
export default function CopyImportTab(props: ToolsProps) {
  const { server, overview, place, selection, existing, jobs } = props;
  const { t } = useExtTranslations();
  const { addToast } = useToast();
  const canUpload = useServerCan('files.create');
  const canExtract = useServerCan('files.archive');
  const chunks = count(selection, existing);
  const empty = isEmpty(selection);

  const [copyTarget, setCopyTarget] = useState<Place>(place);
  const [copyOptions, setCopyOptions] = useState<MoveOptions>(defaultOptions);

  const [source, setSource] = useState<'world' | 'upload'>('world');
  // importing from another world is the common case; the current place is the fallback
  const [sourcePlace, setSourcePlace] = useState<Place>(() => {
    const other = overview.worlds.find((world) => world.name !== place.world && world.dimensions.length > 0);
    return other ? { world: other.name, dimension: other.dimensions[0].id } : place;
  });
  const [files, setFiles] = useState<File[]>([]);
  /** Which folder loose .mca files belong to (a zip brings its own folders). */
  const [fileKind, setFileKind] = useState<'region' | 'entities' | 'poi'>('region');
  const [staged, setStaged] = useState<{ folder: string; names: string[] } | null>(null);
  const [uploading, setUploading] = useState<number | null>(null);
  const zips = files.filter((file) => file.name.toLowerCase().endsWith('.zip'));
  // one zip, or any number of .mca files of one kind
  const badFiles = zips.length > 1 || (zips.length === 1 && files.length > 1);
  const [importOptions, setImportOptions] = useState<MoveOptions>(defaultOptions);
  const [intoSelection, setIntoSelection] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const [confirm, setConfirm] = useState<{ request: ImportRequest; text: string } | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);

  const copyNoop =
    copyTarget.world === place.world && copyTarget.dimension === place.dimension && sameOffset(copyOptions.offset);
  const copyProblem = writeProblem(t, props, chunks) ?? (copyNoop ? t('copy.samePlace', {}) : null);
  const importNoop =
    source === 'world' &&
    sourcePlace.world === place.world &&
    sourcePlace.dimension === place.dimension &&
    sameOffset(importOptions.offset);
  const importProblem =
    writeProblem(t, props, intoSelection ? chunks : null) ??
    (source === 'upload' && !staged ? t('copy.uploadFirst', {}) : importNoop ? t('copy.samePlace', {}) : null);

  const upload = async () => {
    if (files.length === 0 || badFiles) return;
    setUploading(0);
    try {
      const folder = await createImport(server);
      const zip = zips[0];
      const { url } = await getFileUploadUrl(server, zip ? folder : `${folder}/${fileKind}`);
      const form = new FormData();
      for (const file of files) form.append('files', file, file.name);
      const total = files.reduce((sum, file) => sum + file.size, 0);
      await axiosInstance.post(url, form, {
        headers: { 'Content-Type': 'multipart/form-data' },
        onUploadProgress: (event) => setUploading(total > 0 ? ((event.loaded ?? 0) / total) * 100 : 100),
      });
      const source = zip ? await extractImport(server, folder, zip.name) : folder;
      setStaged({ folder: source, names: files.map((file) => file.name) });
      setFiles([]);
      addToast(t('copy.uploaded', { count: files.length }), 'success');
    } catch (error) {
      addToast(httpErrorToHuman(error), 'error');
    } finally {
      setUploading(null);
    }
  };

  const download = async () => {
    setDownloading(true);
    try {
      const { blob, filename } = await exportChunks(server, place, toWire(selection));
      downloadBlob(blob, filename ?? `${fileSafe(place.world)}-${fileSafe(place.dimension)}-chunks.zip`);
    } catch (error) {
      addToast(httpErrorToHuman(error), 'error');
    } finally {
      setDownloading(false);
    }
  };

  const askCopy = () =>
    setConfirm({
      request: {
        kind: 'import',
        source: place,
        target: copyTarget,
        offset: copyOptions.offset,
        overwrite: copyOptions.overwrite,
        source_selection: toWire(selection),
        sections: copyOptions.sections.trim() || null,
      },
      text: t('copy.confirmCopy', {
        count: chunks,
        from: placeLabel(t, overview, place),
        to: placeLabel(t, overview, copyTarget),
        offset: offsetText(t, copyOptions.offset),
      }),
    });

  const askImport = () => {
    const from = source === 'upload' && staged ? staged.folder : placeLabel(t, overview, sourcePlace);
    setConfirm({
      request: {
        kind: 'import',
        source: source === 'upload' && staged ? { folder: staged.folder } : sourcePlace,
        target: place,
        offset: importOptions.offset,
        overwrite: importOptions.overwrite,
        target_selection: intoSelection ? toWire(selection) : null,
        sections: importOptions.sections.trim() || null,
      },
      text: t('copy.confirmImport', {
        from,
        to: placeLabel(t, overview, place),
        offset: offsetText(t, importOptions.offset),
      }),
    });
  };

  const run = async () => {
    if (!confirm) return;
    const uploaded = 'folder' in confirm.request.source;
    await jobs.start(confirm.request, uploaded ? { onDone: () => setStaged(null) } : undefined);
    setConfirm(null);
  };

  return (
    <Stack gap='sm'>
      {overview.capabilities.import_chunks && (
        <>
          <Text size='sm' fw={600}>
            {t('copy.copyTitle', {})}
          </Text>
          <Text className='wm-label'>
            {t('copy.copyIntro', {})}{' '}
            <Anchor href={IMPORT_WIKI} target='_blank' rel='noreferrer' size='xs'>
              {t('filter.wiki', {})}
            </Anchor>
          </Text>
          <PlacePicker props={props} label={t('copy.target', {})} value={copyTarget} onChange={setCopyTarget} />
          <MoveFields value={copyOptions} onChange={setCopyOptions} />
          <Button size='xs' disabled={!!copyProblem} onClick={askCopy}>
            {t('copy.copyButton', { count: chunks })}
          </Button>
          <ProblemNote problem={copyProblem} />

          <Divider />
          <Text size='sm' fw={600}>
            {t('copy.importTitle', { place: placeLabel(t, overview, place) })}
          </Text>
          <SegmentedControl
            size='xs'
            value={source}
            onChange={(value) => setSource(value as 'world' | 'upload')}
            data={[
              { value: 'world', label: t('copy.fromWorld', {}) },
              { value: 'upload', label: t('copy.fromUpload', {}) },
            ]}
          />
          {source === 'world' ? (
            <PlacePicker props={props} label={t('copy.source', {})} value={sourcePlace} onChange={setSourcePlace} />
          ) : (
            <Stack gap={6}>
              <Text size='sm' fw={500}>
                {t('copy.files', {})}
              </Text>
              <Text className='wm-label'>{t('copy.filesHint', {})}</Text>
              <Group gap={6} wrap='nowrap'>
                <Button
                  size='xs'
                  variant='default'
                  leftSection={<FontAwesomeIcon icon={faFileZipper} />}
                  disabled={!canUpload}
                  onClick={() => fileInput.current?.click()}
                >
                  {t('copy.filesPlaceholder', {})}
                </Button>
                {files.length > 0 && (
                  <Text size='xs' truncate>
                    {files.map((file) => file.name).join(', ')}
                  </Text>
                )}
              </Group>
              <input
                ref={fileInput}
                type='file'
                multiple
                accept='.mca,.zip'
                hidden
                onChange={(event) => {
                  setFiles([...(event.currentTarget.files ?? [])]);
                  event.currentTarget.value = '';
                }}
              />
              {!canUpload && <Text className='wm-label'>{t('copy.noUploadPermission', {})}</Text>}
              {files.length > 0 && zips.length === 0 && (
                <SegmentedControl
                  size='xs'
                  value={fileKind}
                  onChange={(value) => setFileKind(value as 'region' | 'entities' | 'poi')}
                  data={[
                    { value: 'region', label: 'region/' },
                    { value: 'entities', label: 'entities/' },
                    { value: 'poi', label: 'poi/' },
                  ]}
                  aria-label={t('copy.fileKind', {})}
                />
              )}
              {badFiles && (
                <Text size='xs' c='red'>
                  {t('copy.oneZip', {})}
                </Text>
              )}
              {canUpload && !canExtract && zips.length > 0 && (
                <Text size='xs' c='orange'>
                  {t('copy.noArchivePermission', {})}
                </Text>
              )}
              {uploading !== null ? (
                <Progress value={uploading} hourglass={false} size='md' />
              ) : (
                <Button
                  size='xs'
                  variant='default'
                  leftSection={<FontAwesomeIcon icon={faUpload} />}
                  disabled={files.length === 0 || !canUpload || badFiles || (zips.length > 0 && !canExtract)}
                  onClick={upload}
                >
                  {t('copy.upload', {})}
                </Button>
              )}
              {staged && (
                <Alert color='green' p='xs'>
                  <Text size='xs'>
                    {t('copy.staged', { count: staged.names.length })} <span className='wm-mono'>{staged.folder}</span>
                  </Text>
                </Alert>
              )}
            </Stack>
          )}
          <MoveFields value={importOptions} onChange={setImportOptions} />
          <Checkbox
            label={t('copy.intoSelection', {})}
            description={t('copy.intoSelectionHint', {})}
            checked={intoSelection}
            disabled={empty}
            onChange={(event) => setIntoSelection(event.currentTarget.checked)}
          />
          <Button size='xs' disabled={!!importProblem} onClick={askImport}>
            {t('copy.importButton', {})}
          </Button>
          <ProblemNote problem={importProblem} />
        </>
      )}

      {overview.capabilities.export_chunks && (
        <>
          {overview.capabilities.import_chunks && <Divider />}
          <Text size='sm' fw={600}>
            {t('copy.downloadTitle', {})}
          </Text>
          <Text className='wm-label'>{t('copy.downloadIntro', {})}</Text>
          <Button
            size='xs'
            variant='default'
            leftSection={<FontAwesomeIcon icon={faDownload} />}
            disabled={empty}
            loading={downloading}
            onClick={download}
          >
            {t('copy.download', { count: chunks })}
          </Button>
        </>
      )}

      <ConfirmationModal
        opened={confirm !== null}
        onClose={() => setConfirm(null)}
        title={t('copy.confirmTitle', {})}
        confirm={t('copy.confirm', {})}
        confirmColor='orange'
        onConfirmed={run}
      >
        <Stack gap='sm'>
          <Text size='sm'>{confirm?.text}</Text>
          {confirm && (
            <Text size='xs'>
              {confirm.request.overwrite ? t('copy.overwriteOn', {}) : t('copy.overwriteOff', {})}
              {confirm.request.sections ? ` · ${t('copy.sectionsOn', { ranges: confirm.request.sections })}` : ''}
            </Text>
          )}
          <Alert color='yellow'>{t('tools.backupNote', { folder: backupFolder(overview, 'import') })}</Alert>
        </Stack>
      </ConfirmationModal>
    </Stack>
  );
}
