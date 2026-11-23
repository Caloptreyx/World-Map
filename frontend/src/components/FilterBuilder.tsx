import { faLayerGroup, faPlus, faXmark } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import ActionIcon from '@/elements/buttons/ActionIcon.tsx';
import Button from '@/elements/buttons/Button.tsx';
import Checkbox from '@/elements/input/Checkbox.tsx';
import Select from '@/elements/input/Select.tsx';
import TextInput from '@/elements/input/TextInput.tsx';
import Group from '@/elements/layout/Group.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import Text from '@/elements/typography/Text.tsx';
import { type Comparator, FILTER_TYPES, type FilterNode, type Operator } from '../lib/filter.ts';
import { useExtTranslations } from '../translations.ts';

/** A group of conditions: the root of the builder, and nested groups. */
export type GroupNode = Extract<FilterNode, { kind: 'group' }>;
type Leaf = Extract<FilterNode, { kind: 'filter' }>;

export const emptyGroup = (): GroupNode => ({ kind: 'group', operator: 'AND', negated: false, children: [] });

const newFilter = (operator: Operator): Leaf => {
  const type = FILTER_TYPES[0];
  return { kind: 'filter', operator, type: type.name, comparator: type.comparators[0], value: '' };
};

const typeData = FILTER_TYPES.map((type) => ({ value: type.name, label: type.name }));

interface NodeProps<T extends FilterNode> {
  node: T;
  /** The first condition of a group has no operator: it links to nothing before it. */
  first: boolean;
  onChange: (node: FilterNode) => void;
  onRemove?: () => void;
}

function OperatorSelect({ value, onChange }: { value: Operator; onChange: (operator: Operator) => void }) {
  const { t } = useExtTranslations();
  return (
    <Select
      size='xs'
      w={74}
      aria-label={t('filter.operator', {})}
      data={['AND', 'OR']}
      value={value}
      onChange={(next) => next && onChange(next as Operator)}
    />
  );
}

function FilterRow({ node, first, onChange, onRemove }: NodeProps<Leaf>) {
  const { t } = useExtTranslations();
  const type = FILTER_TYPES.find((candidate) => candidate.name === node.type);
  const comparators = type?.comparators ?? [node.comparator];
  return (
    <div className='wm-filter-row'>
      <Group gap={6} wrap='wrap' align='flex-start'>
        {first ? (
          <span className='wm-filter-where'>{t('filter.where', {})}</span>
        ) : (
          <OperatorSelect value={node.operator} onChange={(operator) => onChange({ ...node, operator })} />
        )}
        <Select
          size='xs'
          w={150}
          searchable
          aria-label={t('filter.type', {})}
          data={
            typeData.some((item) => item.value === node.type)
              ? typeData
              : [...typeData, { value: node.type, label: node.type }]
          }
          value={node.type}
          onChange={(name) => {
            const next = FILTER_TYPES.find((candidate) => candidate.name === name);
            if (!next) return;
            const comparator = next.comparators.includes(node.comparator) ? node.comparator : next.comparators[0];
            onChange({ ...node, type: next.name, comparator });
          }}
        />
        <Select
          size='xs'
          w={110}
          aria-label={t('filter.comparator', {})}
          data={comparators}
          value={node.comparator}
          onChange={(comparator) => comparator && onChange({ ...node, comparator: comparator as Comparator })}
        />
        <TextInput
          size='xs'
          style={{ flex: '1 1 140px' }}
          aria-label={t('filter.value', {})}
          placeholder={type?.example}
          value={node.value}
          onChange={(event) => onChange({ ...node, value: event.currentTarget.value })}
        />
        {onRemove && (
          <ActionIcon variant='subtle' color='gray' size={30} aria-label={t('filter.remove', {})} onClick={onRemove}>
            <FontAwesomeIcon icon={faXmark} />
          </ActionIcon>
        )}
      </Group>
      {type?.hint && <Text className='wm-label'>{type.hint}</Text>}
    </div>
  );
}

function GroupBox({ node, first, onChange, onRemove }: NodeProps<GroupNode>) {
  const { t } = useExtTranslations();
  const setChild = (index: number, child: FilterNode) =>
    onChange({ ...node, children: node.children.map((current, i) => (i === index ? child : current)) });
  const removeChild = (index: number) => onChange({ ...node, children: node.children.filter((_, i) => i !== index) });
  return (
    <div className='wm-filter-group' data-root={!onRemove}>
      <Group gap={6} justify='space-between' wrap='nowrap'>
        <Group gap={8} wrap='wrap'>
          {onRemove &&
            (first ? (
              <span className='wm-filter-where'>{t('filter.where', {})}</span>
            ) : (
              <OperatorSelect value={node.operator} onChange={(next) => onChange({ ...node, operator: next })} />
            ))}
          <Checkbox
            size='xs'
            label={t('filter.negate', {})}
            checked={node.negated}
            onChange={(event) => onChange({ ...node, negated: event.currentTarget.checked })}
          />
        </Group>
        {onRemove && (
          <ActionIcon
            variant='subtle'
            color='gray'
            size={30}
            aria-label={t('filter.removeGroup', {})}
            onClick={onRemove}
          >
            <FontAwesomeIcon icon={faXmark} />
          </ActionIcon>
        )}
      </Group>
      <Stack gap={6} mt={6}>
        {node.children.length === 0 && <Text className='wm-label'>{t('filter.emptyGroup', {})}</Text>}
        {node.children.map((child, index) =>
          child.kind === 'group' ? (
            <GroupBox
              key={index}
              node={child}
              first={index === 0}
              onChange={(next) => setChild(index, next)}
              onRemove={() => removeChild(index)}
            />
          ) : (
            <FilterRow
              key={index}
              node={child}
              first={index === 0}
              onChange={(next) => setChild(index, next)}
              onRemove={() => removeChild(index)}
            />
          ),
        )}
        <Group gap={6}>
          <Button
            size='compact-xs'
            variant='subtle'
            leftSection={<FontAwesomeIcon icon={faPlus} />}
            onClick={() => onChange({ ...node, children: [...node.children, newFilter('AND')] })}
          >
            {t('filter.addCondition', {})}
          </Button>
          <Button
            size='compact-xs'
            variant='subtle'
            leftSection={<FontAwesomeIcon icon={faLayerGroup} />}
            onClick={() =>
              onChange({ ...node, children: [...node.children, { ...emptyGroup(), children: [newFilter('AND')] }] })
            }
          >
            {t('filter.addGroup', {})}
          </Button>
        </Group>
      </Stack>
    </div>
  );
}

/** MCA Selector's filter editor: conditions linked by AND / OR, nested groups, negated groups. */
export default function FilterBuilder({ root, onChange }: { root: GroupNode; onChange: (root: GroupNode) => void }) {
  return <GroupBox node={root} first onChange={(next) => onChange(next as GroupNode)} />;
}
