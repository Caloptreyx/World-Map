import { faCrosshairs, faLocationArrow, faRotate, faTrash } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import ActionIcon from '@/elements/buttons/ActionIcon.tsx';
import Avatar from '@/elements/data-display/Avatar.tsx';
import Select from '@/elements/input/Select.tsx';
import Group from '@/elements/layout/Group.tsx';
import Paper from '@/elements/layout/Paper.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import Menu from '@/elements/overlays/Menu.tsx';
import Tooltip from '@/elements/overlays/Tooltip.tsx';
import Text from '@/elements/typography/Text.tsx';
import type { Online, Player } from '../api.ts';
import { useExtTranslations } from '../translations.ts';
import { CARD_STYLE, dimensionLabel } from './OverviewCard.tsx';

export const avatarUrl = (uuid: string, size: number) => `https://mc-heads.net/avatar/${uuid}/${size}`;

/** A player's name, or the start of their UUID when the server never cached the name. */
export const playerLabel = (player: Player) => player.name ?? player.uuid.slice(0, 8);

interface Props {
  players: Player[];
  selected: Player | null;
  onSelect: (uuid: string | null) => void;
  online: Online | null;
  onlineError: string | null;
  running: boolean;
  canTeleport: boolean;
  canResetHeatmap: boolean;
  onLocate: (player: Player) => void;
  onTeleportTo: (player: Player, target: string) => void;
  onPick: (player: Player) => void;
  onRefreshOnline: () => void;
  onResetHeatmap: () => void;
}

export default function PlayerCard(props: Props) {
  const { t } = useExtTranslations();
  const { players, selected, online } = props;
  const isOnline = (player: Player) => !!player.name && !!online?.players.includes(player.name);
  const selectedOnline = selected ? isOnline(selected) : false;
  const others = online?.players.filter((name) => name !== selected?.name) ?? [];

  return (
    <Paper withBorder radius='md' p='sm' w={300} style={CARD_STYLE}>
      <Stack gap='xs'>
        <Group justify='space-between' wrap='nowrap'>
          <Text fw={600}>{t('players.title', {})}</Text>
          {props.canResetHeatmap && (
            <Tooltip label={t('players.resetHeatmap', {})}>
              <ActionIcon variant='subtle' color='red' size='sm' onClick={props.onResetHeatmap}>
                <FontAwesomeIcon icon={faTrash} />
              </ActionIcon>
            </Tooltip>
          )}
        </Group>
        <Text size='xs' c='dimmed'>
          {t('players.description', {})}
        </Text>
        {players.length === 0 ? (
          <Text size='xs'>{t('players.none', {})}</Text>
        ) : (
          <Group gap={6} wrap='nowrap'>
            <Select
              size='xs'
              style={{ flex: 1 }}
              placeholder={t('players.placeholder', {})}
              searchable
              allowDeselect
              data={players.map((player) => ({ value: player.uuid, label: playerLabel(player) }))}
              value={selected?.uuid ?? null}
              onChange={props.onSelect}
              leftSection={selected ? <Avatar src={avatarUrl(selected.uuid, 32)} size={18} radius='xs' /> : null}
              renderOption={({ option }) => {
                const player = players.find((candidate) => candidate.uuid === option.value)!;
                return (
                  <Group gap={8} wrap='nowrap'>
                    <Avatar src={avatarUrl(player.uuid, 32)} size={20} radius='xs' />
                    <Text size='xs'>{option.label}</Text>
                    {isOnline(player) && (
                      <Text size='xs' c='green'>
                        ●
                      </Text>
                    )}
                  </Group>
                );
              }}
            />
            <Tooltip label={t('players.locate', {})}>
              <ActionIcon variant='light' disabled={!selected} onClick={() => selected && props.onLocate(selected)}>
                <FontAwesomeIcon icon={faCrosshairs} />
              </ActionIcon>
            </Tooltip>
            {props.canTeleport && (
              <Menu position='bottom-end' withinPortal>
                <Menu.Target>
                  <ActionIcon
                    variant='light'
                    disabled={!selected}
                    title={t('players.teleport', {})}
                    aria-label={t('players.teleport', {})}
                  >
                    <FontAwesomeIcon icon={faLocationArrow} />
                  </ActionIcon>
                </Menu.Target>
                <Menu.Dropdown>
                  {!props.running ? (
                    <Menu.Label>{t('players.teleportOffline', {})}</Menu.Label>
                  ) : !selectedOnline ? (
                    <Menu.Label>
                      {t('players.teleportNotOnline', { name: selected ? playerLabel(selected) : '' })}
                    </Menu.Label>
                  ) : (
                    <>
                      <Menu.Item onClick={() => selected && props.onPick(selected)}>
                        {t('players.teleportHere', {})}
                      </Menu.Item>
                      {others.length > 0 && <Menu.Divider />}
                      {others.slice(0, 20).map((name) => (
                        <Menu.Item key={name} onClick={() => selected && props.onTeleportTo(selected, name)}>
                          {t('players.teleportToPlayer', { name })}
                        </Menu.Item>
                      ))}
                    </>
                  )}
                  <Menu.Divider />
                  <Menu.Item leftSection={<FontAwesomeIcon icon={faRotate} />} onClick={props.onRefreshOnline}>
                    {t('players.refreshOnline', {})}
                  </Menu.Item>
                </Menu.Dropdown>
              </Menu>
            )}
          </Group>
        )}
        {selected && (
          <Stack gap={2}>
            <Group gap={6}>
              <Text size='xs' c={selectedOnline ? 'green' : 'dimmed'}>
                {selectedOnline ? t('players.online', {}) : t('players.offline', {})}
              </Text>
              <Text size='xs' ff='monospace'>
                X {selected.x.toFixed(2)} · Y {selected.y.toFixed(2)} · Z {selected.z.toFixed(2)}
              </Text>
            </Group>
            <Text size='xs' c='dimmed'>
              {t('players.elsewhere', { world: selected.world, dimension: dimensionLabel(t, selected.dimension) })}
            </Text>
            <Text size='xs' c='dimmed'>
              {t('players.lastSeen', { time: new Date(selected.last_seen * 1000).toLocaleString() })}
            </Text>
          </Stack>
        )}
        {props.onlineError && (
          <Text size='xs' c='orange'>
            {t('players.onlineUnknown', { error: props.onlineError })}
          </Text>
        )}
      </Stack>
    </Paper>
  );
}
