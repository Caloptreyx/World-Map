import Select from '@/elements/input/Select.tsx';
import Switch from '@/elements/input/Switch.tsx';
import TextInput from '@/elements/input/TextInput.tsx';
import Group from '@/elements/layout/Group.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import Anchor from '@/elements/typography/Anchor.tsx';
import Text from '@/elements/typography/Text.tsx';
import { OVERLAY_TYPES } from '../lib/filter.ts';
import { parseOverlayBound } from '../lib/overlay.ts';
import { useExtTranslations } from '../translations.ts';

export const OVERLAY_WIKI = 'https://github.com/Querz/mcaselector/wiki/Overlays';

/** The overlay as chosen (saved with the view): bounds stay text, as typed. */
export interface OverlayChoice {
  enabled: boolean;
  type: string;
  parameter: string;
  min: string;
  max: string;
}

export const DEFAULT_OVERLAY: OverlayChoice = {
  enabled: false,
  type: OVERLAY_TYPES[0]?.name ?? 'InhabitedTime',
  parameter: '',
  min: '',
  max: '',
};

/** A bound typed by the viewer: null when empty (auto), undefined when it cannot be read. */
export function overlayBound(choice: OverlayChoice, text: string): number | null | undefined {
  if (!text.trim()) return null;
  return parseOverlayBound(choice.type, text) ?? undefined;
}

/** Whether the overlay can be requested: on, and a parameter where the type needs one. */
export function overlayReady(choice: OverlayChoice): boolean {
  const type = OVERLAY_TYPES.find((candidate) => candidate.name === choice.type);
  return choice.enabled && !!type && (!type.parameter || choice.parameter.trim() !== '');
}

/** Turn the overlay on, pick its type, its parameter and the colour range. */
export default function OverlaySettings({
  value,
  onChange,
}: {
  value: OverlayChoice;
  onChange: (choice: OverlayChoice) => void;
}) {
  const { t } = useExtTranslations();
  const type = OVERLAY_TYPES.find((candidate) => candidate.name === value.type);
  const minError = overlayBound(value, value.min) === undefined;
  const maxError = overlayBound(value, value.max) === undefined;
  return (
    <Stack gap='xs'>
      <Switch
        size='md'
        label={t('overlay.show', {})}
        checked={value.enabled}
        onChange={(event) => onChange({ ...value, enabled: event.currentTarget.checked })}
      />
      <Select
        size='xs'
        label={t('overlay.type', {})}
        description={type ? `${type.hint}${type.unit ? ` (${type.unit})` : ''}` : undefined}
        data={OVERLAY_TYPES.map((candidate) => candidate.name)}
        value={value.type}
        onChange={(name) => name && onChange({ ...value, type: name, min: '', max: '' })}
        comboboxProps={{ withinPortal: false }}
      />
      {type?.parameter && (
        <TextInput
          size='xs'
          label={t('overlay.parameter', {})}
          description={t('overlay.parameterHint', {})}
          placeholder='minecraft:diamond_ore,minecraft:deepslate_diamond_ore'
          value={value.parameter}
          error={value.enabled && !value.parameter.trim() ? t('overlay.parameterMissing', {}) : undefined}
          onChange={(event) => onChange({ ...value, parameter: event.currentTarget.value })}
        />
      )}
      <Group gap={6} grow>
        <TextInput
          size='xs'
          label={t('overlay.min', {})}
          placeholder={t('overlay.auto', {})}
          value={value.min}
          error={minError ? t('overlay.badBound', {}) : undefined}
          onChange={(event) => onChange({ ...value, min: event.currentTarget.value })}
        />
        <TextInput
          size='xs'
          label={t('overlay.max', {})}
          placeholder={t('overlay.auto', {})}
          value={value.max}
          error={maxError ? t('overlay.badBound', {}) : undefined}
          onChange={(event) => onChange({ ...value, max: event.currentTarget.value })}
        />
      </Group>
      <Text className='wm-label'>
        {t('overlay.boundsHint', {})}{' '}
        <Anchor href={OVERLAY_WIKI} target='_blank' rel='noreferrer' size='xs'>
          {t('filter.wiki', {})}
        </Anchor>
      </Text>
    </Stack>
  );
}
