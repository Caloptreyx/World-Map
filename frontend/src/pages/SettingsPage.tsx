import { faFire, faImage, faServer } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import getAllEggs from '@/api/admin/nests/getAllEggs.ts';
import { httpErrorToHuman } from '@/api/axios.ts';
import Button from '@/elements/buttons/Button.tsx';
import TitleCard from '@/elements/data-display/TitleCard.tsx';
import Alert from '@/elements/feedback/Alert.tsx';
import Spinner from '@/elements/feedback/Spinner.tsx';
import MultiSelect from '@/elements/input/MultiSelect.tsx';
import NumberInput from '@/elements/input/NumberInput.tsx';
import Switch from '@/elements/input/Switch.tsx';
import Group from '@/elements/layout/Group.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import { useAdminCan } from '@/plugins/usePermissions.ts';
import { useToast } from '@/providers/ToastProvider.tsx';
import { getSettings, PACKAGE, type Settings, updateSettings } from '../api.ts';
import { useExtTranslations } from '../translations.ts';

const settingsKey = [PACKAGE, 'settings'] as const;

type NumberField = {
  [K in keyof Settings]: Settings[K] extends number ? K : never;
}[keyof Settings];

export default function SettingsPage() {
  const { t } = useExtTranslations();
  const { addToast } = useToast();
  const queryClient = useQueryClient();
  const canRead = useAdminCan('extensions.read');
  const canManage = useAdminCan('extensions.manage');
  const query = useQuery({ queryKey: settingsKey, queryFn: getSettings, enabled: canRead });
  const eggs = useQuery({ queryKey: [PACKAGE, 'eggs'], queryFn: getAllEggs, enabled: canRead });
  const [form, setForm] = useState<Settings | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (query.data) setForm(query.data);
  }, [query.data]);

  if (query.isLoading) {
    return (
      <Group justify='center' py='lg'>
        <Spinner />
      </Group>
    );
  }
  if (query.error || !form) {
    return <Alert color='red'>{query.error ? httpErrorToHuman(query.error) : null}</Alert>;
  }

  const update = (patch: Partial<Settings>) => setForm({ ...form, ...patch });
  const number = (field: NumberField, label: string, min: number, max: number, description?: string) => (
    <NumberInput
      label={label}
      description={description}
      min={min}
      max={max}
      allowDecimal={false}
      value={form[field]}
      onChange={(value) => typeof value === 'number' && update({ [field]: value })}
      disabled={!canManage}
    />
  );

  const save = async () => {
    setSaving(true);
    try {
      const saved = await updateSettings(form);
      setForm(saved);
      queryClient.setQueryData(settingsKey, saved);
      queryClient.invalidateQueries({ queryKey: [PACKAGE] });
      addToast(t('settings.saved', {}), 'success');
    } catch (error) {
      addToast(httpErrorToHuman(error), 'error');
    } finally {
      setSaving(false);
    }
  };

  return (
    <Stack>
      <TitleCard title={t('settings.access', {})} icon={<FontAwesomeIcon icon={faServer} />}>
        <MultiSelect
          label={t('settings.eggs', {})}
          description={t('settings.eggsDescription', {})}
          searchable
          data={(eggs.data ?? []).map(({ nest, eggs: nestEggs }) => ({
            group: nest.name,
            items: nestEggs.map((egg) => ({ value: egg.uuid, label: egg.name })),
          }))}
          value={form.eggs}
          onChange={(value) => update({ eggs: value })}
          disabled={!canManage}
        />
      </TitleCard>
      <TitleCard title={t('settings.heatmap', {})} icon={<FontAwesomeIcon icon={faFire} />}>
        <Stack>
          <Switch
            label={t('settings.heatmapEnabled', {})}
            checked={form.heatmap_enabled}
            onChange={(event) => update({ heatmap_enabled: event.currentTarget.checked })}
            disabled={!canManage}
          />
          {number(
            'heatmap_interval_minutes',
            t('settings.heatmapInterval', {}),
            1,
            60,
            t('settings.heatmapIntervalDescription', {}),
          )}
          {number('heatmap_retention_days', t('settings.heatmapRetention', {}), 1, 365)}
        </Stack>
      </TitleCard>
      <TitleCard title={t('settings.rendering', {})} icon={<FontAwesomeIcon icon={faImage} />}>
        <Stack>
          {number('max_region_mb', t('settings.maxRegion', {}), 1, 1024)}
          {number('render_concurrency', t('settings.concurrency', {}), 1, 16)}
          {number(
            'nether_ceiling',
            t('settings.netherCeiling', {}),
            -64,
            320,
            t('settings.netherCeilingDescription', {}),
          )}
          {number('entity_limit', t('settings.entityLimit', {}), 100, 50_000)}
          {number('max_delete_chunks', t('settings.maxDelete', {}), 1, 1_048_576)}
          {number('tile_cache_mb', t('settings.tileCache', {}), 64, 1_048_576, t('settings.tileCacheDescription', {}))}
        </Stack>
      </TitleCard>
      {canManage && (
        <Group justify='flex-end'>
          <Button loading={saving} onClick={save}>
            {t('settings.save', {})}
          </Button>
        </Group>
      )}
    </Stack>
  );
}
