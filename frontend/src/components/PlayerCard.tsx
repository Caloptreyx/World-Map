import { faCrosshairs, faLocationArrow, faRotate } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import Button from '@/elements/buttons/Button.tsx';
import Avatar from '@/elements/data-display/Avatar.tsx';
import Badge from '@/elements/data-display/Badge.tsx';
import Select from '@/elements/input/Select.tsx';
import Group from '@/elements/layout/Group.tsx';
import Stack from '@/elements/layout/Stack.tsx';
import Menu from '@/elements/overlays/Menu.tsx';
import Text from '@/elements/typography/Text.tsx';
import type { Online, Player, Presence } from '../api.ts';
import { isOnline } from '../lib/players.ts';
import { useExtTranslations } from '../translations.ts';
import { dimensionLabel } from './OverviewCard.tsx';

export const avatarUrl = (uuid: string, size: number) => `https://mc-heads.net/avatar/${uuid}/${size}`;

/** A player's name, or the start of their UUID when the server never cached the name. */
export const playerLabel = (player: Player) => player.name ?? player.uuid.slice(0, 8);

interface Props {
  /** The players the layers show. */
  players: Player[];
  /** Why `players` is empty. */
  empty: string;
  selected: Player | null;
  onSelect: (uuid: string | null) => void;
  online: Online | null;
  onlineError: string | null;
  presence: Presence | null;
  /** The server's setting hides online players. */
  onlineHidden: boolean;
  running: boolean;
  canTeleport: boolean;
  /** Whether the shown world and dimension can be teleported into. */
  canPickSpot: boolean;
  onLocate: (player: Player) => void;
  onTeleportTo: (player: Player, target: string) => void;
  onPick: (player: Player) => void;
  onRefreshOnline: () => void;
}

/** How sure the list is about who is online, when it is not sure. */
function PresenceNote({ presence }: { presence: Presence | null }) {
  const { t } = useExtTranslations();
  if (!presence) return null;
  const text =
    presence.state === 'not_running'
      ? t('players.presenceStopped', {})
      : presence.state === 'unknown'
        ? t('players.presenceUnknown', { error: presence.error })
        : presence.state === 'partial' && presence.count !== undefined
          ? t('players.presencePartial', { named: presence.named ?? 0, count: presence.count })
          : null;
  return text ? (
    <Text size='xs' c={presence.state === 'not_running' ? 'dimmed' : 'orange'}>
      {text}
    </Text>
  ) : null;
}

/** Choosing a player, where they were last saved, and locating or teleporting them. */
export default function PlayersContent(props: Props) {
  const { t } = useExtTranslations();
  const { players, selected, online } = props;
  const selectedOnline = selected ? isOnline(selected, online) : false;
  const others = online?.players.filter((name) => name !== selected?.name) ?? [];

  if (players.length === 0) {
    return (
      <Stack gap='sm'>
        <Text className='wm-label'>{props.empty}</Text>
        <PresenceNote presence={props.presence} />
      </Stack>
    );
  }

  return (
    <Stack gap='sm'>
      <Text className='wm-label'>{t('players.description', {})}</Text>
      <Select
        size='sm'
        placeholder={t('players.placeholder', {})}
        aria-label={t('players.placeholder', {})}
        searchable
        allowDeselect
        data={players.map((player) => ({ value: player.uuid, label: playerLabel(player) }))}
        value={selected?.uuid ?? null}
        onChange={props.onSelect}
        leftSection={selected ? <Avatar src={avatarUrl(selected.uuid, 32)} size={20} radius='xs' /> : null}
        renderOption={({ option }) => {
          const player = players.find((candidate) => candidate.uuid === option.value)!;
          return (
            <Group gap={8} wrap='nowrap'>
              <Avatar src={avatarUrl(player.uuid, 32)} size={22} radius='xs' />
              <Text size='sm' style={{ flex: 1 }}>
                {option.label}
              </Text>
              {isOnline(player, online) && (
                <Badge size='xs' color='green' variant='light'>
                  {t('players.online', {})}
                </Badge>
              )}
            </Group>
          );
        }}
      />
      {selected && (
        <>
          <Group gap={10} wrap='nowrap' align='flex-start'>
            <Avatar src={avatarUrl(selected.uuid, 64)} size={36} radius='sm' />
            <Stack gap={2} style={{ minWidth: 0 }}>
              <Group gap={6}>
                <Text size='sm' fw={600}>
                  {playerLabel(selected)}
                </Text>
                <Badge size='xs' variant='light' color={selectedOnline ? 'green' : 'gray'}>
                  {selectedOnline ? t('players.online', {}) : t('players.offline', {})}
                </Badge>
              </Group>
              <span className='wm-mono'>
                {selected.x.toFixed(1)}, {selected.y.toFixed(1)}, {selected.z.toFixed(1)}
              </span>
              <Text className='wm-label'>
                {t('players.elsewhere', { world: selected.world, dimension: dimensionLabel(t, selected.dimension) })}
                {' · '}
                {t('players.lastSeen', { time: new Date(selected.last_seen * 1000).toLocaleString() })}
              </Text>
            </Stack>
          </Group>
          <Group gap={6} grow>
            <Button
              size='sm'
              variant='light'
              leftSection={<FontAwesomeIcon icon={faCrosshairs} />}
              onClick={() => props.onLocate(selected)}
            >
              {t('players.locate', {})}
            </Button>
            {props.canTeleport && (
              <Menu position='bottom-end' withinPortal>
                <Menu.Target>
                  <Button size='sm' variant='light' leftSection={<FontAwesomeIcon icon={faLocationArrow} />}>
                    {t('players.teleport', {})}
                  </Button>
                </Menu.Target>
                <Menu.Dropdown>
                  {!props.running ? (
                    <Menu.Label>{t('players.teleportOffline', {})}</Menu.Label>
                  ) : props.onlineHidden ? (
                    <Menu.Label>{t('players.teleportHidden', {})}</Menu.Label>
                  ) : !selectedOnline ? (
                    <Menu.Label>{t('players.teleportNotOnline', { name: playerLabel(selected) })}</Menu.Label>
                  ) : (
                    <>
                      {props.canPickSpot ? (
                        <Menu.Item onClick={() => props.onPick(selected)}>{t('players.teleportHere', {})}</Menu.Item>
                      ) : (
                        <Menu.Label>{t('players.teleportNoKey', {})}</Menu.Label>
                      )}
                      {others.length > 0 && <Menu.Divider />}
                      {others.slice(0, 20).map((name) => (
                        <Menu.Item key={name} onClick={() => props.onTeleportTo(selected, name)}>
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
        </>
      )}
      <PresenceNote presence={props.presence} />
      {props.onlineError && (
        <Text size='xs' c='orange'>
          {t('players.onlineUnknown', { error: props.onlineError })}
        </Text>
      )}
    </Stack>
  );
}
