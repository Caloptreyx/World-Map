import { useDebouncedValue } from '@mantine/hooks';
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import Button from '@/elements/buttons/Button.tsx';
import Alert from '@/elements/feedback/Alert.tsx';
import Checkbox from '@/elements/input/Checkbox.tsx';
import Select from '@/elements/input/Select.tsx';
import TextInput from '@/elements/input/TextInput.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import ConfirmationModal from '@/elements/modals/ConfirmationModal.tsx';
import Anchor from '@/elements/typography/Anchor.tsx';
import Text from '@/elements/typography/Text.tsx';
import { checkChanges, PACKAGE } from '../api.ts';
import { CHANGE_FIELDS, changesText } from '../lib/filter.ts';
import { count, selectedRegions, toWire } from '../lib/selection.ts';
import { useExtTranslations } from '../translations.ts';
import { placeLabel } from './OverviewCard.tsx';
import { backupFolder, ProblemNote, type ToolsProps, writeProblem } from './toolsCommon.tsx';

export const CHANGE_WIKI = 'https://github.com/Querz/mcaselector/wiki/NBT-Changer';

interface FieldState {
  enabled: boolean;
  value: string;
}

/** MCA Selector's "Change NBT fields": set fields of every selected chunk. */
export default function ChangeFieldsTab(props: ToolsProps) {
  const { server, overview, place, selection, existing, jobs } = props;
  const { t } = useExtTranslations();
  const [fields, setFields] = useState<Record<string, FieldState>>({});
  const [force, setForce] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const enabled = CHANGE_FIELDS.filter((field) => fields[field.name]?.enabled).map((field) => ({
    name: field.name,
    value: fields[field.name].value,
  }));
  const text = enabled.length > 0 ? changesText(enabled) : '';
  const [settled] = useDebouncedValue(text, 300);
  const check = useQuery({
    queryKey: [PACKAGE, server, 'changes-check', settled],
    queryFn: () => checkChanges(server, settled),
    enabled: settled !== '',
    staleTime: Infinity,
  });
  const checked = text === settled && text !== '' ? check.data : undefined;
  const chunks = count(selection, existing);
  const problem =
    writeProblem(t, props, chunks) ??
    (text === '' ? t('change.nothing', {}) : checked?.ok ? null : t('change.checking', {}));

  const set = (name: string, change: Partial<FieldState>) =>
    setFields((current) => ({
      ...current,
      [name]: { ...(current[name] ?? { enabled: false, value: '' }), ...change },
    }));

  const apply = async () => {
    if (!checked?.ok) return;
    await jobs.start({
      kind: 'change',
      world: place.world,
      dimension: place.dimension,
      selection: toWire(selection),
      changes: checked.normalized,
      force,
    });
    setConfirm(false);
  };

  return (
    <Stack gap='sm'>
      <Text className='wm-label'>
        {t('change.intro', {})}{' '}
        <Anchor href={CHANGE_WIKI} target='_blank' rel='noreferrer' size='xs'>
          {t('filter.wiki', {})}
        </Anchor>
      </Text>
      <div className='wm-fields'>
        {CHANGE_FIELDS.map((field) => {
          const state = fields[field.name] ?? { enabled: false, value: '' };
          return (
            <div key={field.name} className='wm-field' data-enabled={state.enabled}>
              <Checkbox
                size='xs'
                label={field.name}
                checked={state.enabled}
                onChange={(event) => set(field.name, { enabled: event.currentTarget.checked })}
              />
              {state.enabled &&
                (field.values ? (
                  <Select
                    size='xs'
                    aria-label={field.name}
                    data={field.values}
                    value={state.value || null}
                    placeholder={field.example}
                    description={field.hint}
                    onChange={(value) => set(field.name, { value: value ?? '' })}
                  />
                ) : (
                  <TextInput
                    size='xs'
                    aria-label={field.name}
                    placeholder={field.example}
                    description={field.hint}
                    value={state.value}
                    onChange={(event) => set(field.name, { value: event.currentTarget.value })}
                  />
                ))}
            </div>
          );
        })}
      </div>
      <Checkbox
        label={t('change.force', {})}
        description={t('change.forceHint', {})}
        checked={force}
        onChange={(event) => setForce(event.currentTarget.checked)}
      />
      {text !== '' && (
        <div>
          <Text className='wm-label'>{t('change.preview', {})}</Text>
          <div className='wm-mono wm-code'>{checked?.ok ? checked.normalized : text}</div>
        </div>
      )}
      {checked && !checked.ok && (
        <Alert color='red' p='xs'>
          <Text size='xs'>{t('filter.errorAt', { position: checked.position + 1, error: checked.error })}</Text>
        </Alert>
      )}
      {check.error && text === settled && (
        <Alert color='red' p='xs'>
          <Text size='xs'>{httpErrorToHuman(check.error)}</Text>
        </Alert>
      )}
      <Button size='xs' color='orange' disabled={!!problem} onClick={() => setConfirm(true)}>
        {t('change.apply', { count: chunks })}
      </Button>
      <ProblemNote problem={problem} />

      <ConfirmationModal
        opened={confirm}
        onClose={() => setConfirm(false)}
        title={t('change.confirmTitle', {})}
        confirm={t('change.confirm', {})}
        confirmColor='orange'
        onConfirmed={apply}
      >
        <Stack gap='sm'>
          <Text size='sm'>
            {t('change.confirmContent', {
              count: chunks,
              regions: selectedRegions(selection, existing),
              place: placeLabel(t, overview, place),
            })}
          </Text>
          <div className='wm-mono wm-code'>{checked?.ok ? checked.normalized : text}</div>
          {force && <Text size='xs'>{t('change.forceOn', {})}</Text>}
          <Alert color='yellow'>{t('tools.backupNote', { folder: backupFolder(overview, 'change') })}</Alert>
        </Stack>
      </ConfirmationModal>
    </Stack>
  );
}
