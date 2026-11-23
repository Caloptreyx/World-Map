import { faToolbox, faXmark } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { type ReactNode, useState } from 'react';
import ActionIcon from '@/elements/buttons/ActionIcon.tsx';
import Group from '@/elements/layout/Group.tsx';
import Tabs from '@/elements/layout/Tabs.tsx';
import Drawer from '@/elements/overlays/Drawer.tsx';
import Text from '@/elements/typography/Text.tsx';
import type { ToolsProps } from './toolsCommon.tsx';
import { useExtTranslations } from '../translations.ts';
import ChangeFieldsTab from './ChangeFieldsTab.tsx';
import CopyImportTab from './CopyImportTab.tsx';
import FilterTab from './FilterTab.tsx';
import HistoryTab from './HistoryTab.tsx';
import JobProgress from './JobProgress.tsx';
import SelectionTab from './SelectionTab.tsx';

type Tab = 'selection' | 'filter' | 'change' | 'copy' | 'history';

interface Props extends ToolsProps {
  compact: boolean;
  opened: boolean;
  onClose: () => void;
  /** Chunk or region selection and add/remove, shared with the selection bar. */
  controls: ReactNode;
}

/** MCA Selector's chunk tools: a glass card over the map on wide screens, a bottom sheet on narrow
 * ones. Tabs the viewer has no permission for are left out. */
export default function ChunkTools({ compact, opened, onClose, controls, ...props }: Props) {
  const { t } = useExtTranslations();
  const [tab, setTab] = useState<Tab>('selection');
  const { capabilities } = props.overview;
  const tabs: [Tab, string, boolean][] = [
    ['selection', t('tools.tabs.selection', {}), true],
    ['filter', t('tools.tabs.filter', {}), true],
    ['change', t('tools.tabs.change', {}), capabilities.edit_chunks],
    ['copy', t('tools.tabs.copy', {}), capabilities.import_chunks || capabilities.export_chunks],
    [
      'history',
      t('tools.tabs.history', {}),
      capabilities.delete_chunks || capabilities.edit_chunks || capabilities.import_chunks,
    ],
  ];
  const shown = tabs.filter(([, , allowed]) => allowed);
  const active = shown.some(([name]) => name === tab) ? tab : 'selection';

  const body = (
    <Tabs value={active} onChange={(value) => value && setTab(value as Tab)} keepMounted={false}>
      <Tabs.List grow={compact}>
        {shown.map(([name, label]) => (
          <Tabs.Tab key={name} value={name} px={compact ? 6 : 10}>
            {label}
          </Tabs.Tab>
        ))}
      </Tabs.List>
      <div style={{ paddingTop: 12 }}>
        {props.jobs.job && (
          <div style={{ marginBottom: 12 }}>
            <JobProgress jobs={props.jobs} />
          </div>
        )}
        <Tabs.Panel value='selection'>
          <SelectionTab {...props} controls={controls} />
        </Tabs.Panel>
        <Tabs.Panel value='filter'>
          <FilterTab {...props} />
        </Tabs.Panel>
        {capabilities.edit_chunks && (
          <Tabs.Panel value='change'>
            <ChangeFieldsTab {...props} />
          </Tabs.Panel>
        )}
        {(capabilities.import_chunks || capabilities.export_chunks) && (
          <Tabs.Panel value='copy'>
            <CopyImportTab {...props} />
          </Tabs.Panel>
        )}
        <Tabs.Panel value='history'>
          <HistoryTab {...props} />
        </Tabs.Panel>
      </div>
    </Tabs>
  );

  const title = (
    <Group gap={8} wrap='nowrap'>
      <FontAwesomeIcon icon={faToolbox} />
      <Text fw={600}>{t('tools.title', {})}</Text>
    </Group>
  );

  if (compact) {
    return (
      <Drawer
        opened={opened}
        onClose={onClose}
        position='bottom'
        radius='lg'
        styles={{ content: { height: 'auto', maxHeight: '85dvh', flex: '0 0 100%', width: '100%' } }}
        title={title}
        overlayProps={{ backgroundOpacity: 0.25, blur: 2 }}
      >
        <div style={{ paddingBottom: 16 }}>{body}</div>
      </Drawer>
    );
  }
  if (!opened) return null;
  return (
    <section className='wm-glass wm-tools' aria-label={t('tools.title', {})}>
      <header className='wm-panel-title' style={{ padding: '12px 12px 0' }}>
        <span style={{ color: 'var(--mantine-color-dimmed)', display: 'inline-flex' }}>
          <FontAwesomeIcon icon={faToolbox} />
        </span>
        <span style={{ flex: 1 }}>{t('tools.title', {})}</span>
        <ActionIcon variant='subtle' size={28} aria-label={t('tools.close', {})} onClick={onClose}>
          <FontAwesomeIcon icon={faXmark} />
        </ActionIcon>
      </header>
      <div className='wm-tools-body'>{body}</div>
    </section>
  );
}
