import { useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import Button from '@/elements/buttons/Button.tsx';
import Badge from '@/elements/data-display/Badge.tsx';
import TagsInput from '@/elements/input/TagsInput.tsx';
import Group from '@/elements/layout/Group.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import { Modal, ModalFooter } from '@/elements/modals/Modal.tsx';
import Text from '@/elements/typography/Text.tsx';
import { useToast } from '@/providers/ToastProvider.tsx';
import { type Overview, PACKAGE, updateWorlds } from '../api.ts';
import { useExtTranslations } from '../translations.ts';

interface Props {
  server: string;
  overview: Overview;
  opened: boolean;
  onClose: () => void;
}

/** World folders added by hand, for layouts the map does not find by itself. */
export default function WorldsModal({ server, overview, opened, onClose }: Props) {
  const { t } = useExtTranslations();
  const { addToast } = useToast();
  const queryClient = useQueryClient();
  const [paths, setPaths] = useState<string[]>(() => overview.extra_worlds.map((world) => world.path));
  const [saving, setSaving] = useState(false);
  const limit = overview.limits.extra_worlds;

  const save = async () => {
    setSaving(true);
    try {
      await updateWorlds(server, paths);
      addToast(t('worlds.saved', {}), 'success');
      await queryClient.invalidateQueries({ queryKey: [PACKAGE, server, 'overview'] });
      onClose();
    } catch (error) {
      addToast(httpErrorToHuman(error), 'error');
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal opened={opened} onClose={onClose} title={t('worlds.title', {})} size='lg'>
      <Stack gap='sm'>
        <Text size='sm' c='dimmed'>
          {t('worlds.description', {
            name: overview.level_name,
            container: overview.world_container ? t('worlds.container', { path: overview.world_container }) : '',
          })}
        </Text>
        <TagsInput
          label={t('worlds.label', {})}
          placeholder={t('worlds.placeholder', {})}
          value={paths}
          onChange={setPaths}
          allowReordering={false}
        />
        {overview.extra_worlds.length > 0 && (
          <Stack gap={4}>
            {overview.extra_worlds.map((world) => (
              <Group key={world.path} gap={8} wrap='nowrap'>
                <Badge size='sm' variant='light' color={world.found ? 'green' : 'yellow'}>
                  {world.found ? t('worlds.found', {}) : t('worlds.notFound', {})}
                </Badge>
                <span className='wm-mono'>{world.path}</span>
              </Group>
            ))}
          </Stack>
        )}
        <Text size='xs' c={paths.length > limit ? 'red' : 'dimmed'}>
          {t('worlds.limit', { count: limit })}
        </Text>
      </Stack>
      <ModalFooter>
        <Button onClick={save} loading={saving} disabled={paths.length > limit}>
          {t('worlds.save', {})}
        </Button>
      </ModalFooter>
    </Modal>
  );
}
