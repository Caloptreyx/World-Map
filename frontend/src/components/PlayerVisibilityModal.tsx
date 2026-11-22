import { useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import Button from '@/elements/buttons/Button.tsx';
import Switch from '@/elements/input/Switch.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import { Modal, ModalFooter } from '@/elements/modals/Modal.tsx';
import Text from '@/elements/typography/Text.tsx';
import { useToast } from '@/providers/ToastProvider.tsx';
import { PACKAGE, type PlayerVisibility, updatePlayerVisibility } from '../api.ts';
import { useExtTranslations } from '../translations.ts';

interface Props {
  server: string;
  visibility: PlayerVisibility;
  opened: boolean;
  onClose: () => void;
}

/** Which players the map shows on this server, for everyone who views it. */
export default function PlayerVisibilityModal({ server, visibility, opened, onClose }: Props) {
  const { t } = useExtTranslations();
  const { addToast } = useToast();
  const queryClient = useQueryClient();
  const [value, setValue] = useState<PlayerVisibility>(visibility);
  const [saving, setSaving] = useState(false);

  const save = async () => {
    setSaving(true);
    try {
      await updatePlayerVisibility(server, value);
      addToast(t('visibility.saved', {}), 'success');
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: [PACKAGE, server, 'overview'] }),
        queryClient.invalidateQueries({ queryKey: [PACKAGE, server, 'players'] }),
        queryClient.invalidateQueries({ queryKey: [PACKAGE, server, 'online'] }),
      ]);
      onClose();
    } catch (error) {
      addToast(httpErrorToHuman(error), 'error');
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal opened={opened} onClose={onClose} title={t('visibility.title', {})}>
      <Stack gap='md'>
        <Text size='sm' c='dimmed'>
          {t('visibility.description', {})}
        </Text>
        <Switch
          label={t('visibility.online', {})}
          description={t('visibility.onlineDescription', {})}
          checked={value.online}
          onChange={(event) => setValue((current) => ({ ...current, online: event.currentTarget.checked }))}
        />
        <Switch
          label={t('visibility.offline', {})}
          description={t('visibility.offlineDescription', {})}
          checked={value.offline}
          onChange={(event) => setValue((current) => ({ ...current, offline: event.currentTarget.checked }))}
        />
      </Stack>
      <ModalFooter>
        <Button onClick={save} loading={saving}>
          {t('visibility.save', {})}
        </Button>
      </ModalFooter>
    </Modal>
  );
}
