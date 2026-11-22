//! Who is online on a running Java server: the query port when `enable-query` is on and the
//! Server List Ping (both without side effects), and, for users who may use the console, its
//! answer to `list` when they do not name everyone. Adapted from Caloptreyx/Player-Manager (MIT).
use crate::{
    files::Wings,
    query,
    tunnel::{self, TunnelError},
};
use serde::Serialize;
use serde_json::{Value, json};
use shared::response::ApiResponse;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

const JAVA_PORT: u16 = 25565;
/// The protocol's packet size limit.
const MAX_PACKET: usize = (1 << 21) - 1;
const NIL_UUID: &str = "00000000-0000-0000-0000-000000000000";
/// The vanilla command first (plugins may replace `list`), then whatever `list` is.
pub const LIST_COMMANDS: [&str; 2] = ["minecraft:list", "list"];
const TAIL_LINES: u64 = 100;
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(3);

/// Where an answer about who is online came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Query,
    Ping,
    Console,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Online {
    pub count: u32,
    pub max: u32,
    pub players: Vec<String>,
    /// The UUIDs the answer gave (the ping's sample), for players without a cached name.
    #[serde(skip)]
    pub ids: Vec<Uuid>,
    /// Every player online is named.
    pub complete: bool,
    pub source: Source,
}

impl From<query::FullStat> for Online {
    fn from(stat: query::FullStat) -> Self {
        Self {
            complete: stat.names.len() >= stat.count as usize,
            count: stat.count,
            max: stat.max,
            players: stat.names,
            ids: Vec::new(),
            source: Source::Query,
        }
    }
}

/// What the map knows about who is online, for the player layer.
#[derive(Debug)]
pub enum Presence {
    /// The server is not running: nobody is online.
    NotRunning,
    /// No source said who is online (or the node did not report the server's state).
    Unknown(String),
    Answered(Online),
}

impl Presence {
    /// Whether the answer names the player (by UUID, or by name ignoring case as the game
    /// does). Players it does not name count as offline, also when it is incomplete.
    pub fn is_online(&self, uuid: Uuid, name: Option<&str>) -> bool {
        let Self::Answered(online) = self else {
            return false;
        };
        online.ids.contains(&uuid)
            || name.is_some_and(|name| {
                online
                    .players
                    .iter()
                    .any(|player| player.eq_ignore_ascii_case(name))
            })
    }

    /// The state for the page; the counts only when online players are shown.
    pub fn summary(&self, counts: bool) -> Value {
        match self {
            Self::NotRunning => json!({ "state": "not_running" }),
            Self::Unknown(error) => json!({ "state": "unknown", "error": error }),
            Self::Answered(online) => {
                let state = if online.complete { "known" } else { "partial" };
                if counts {
                    json!({
                        "state": state,
                        "source": online.source,
                        "count": online.count,
                        "max": online.max,
                        "named": online.players.len(),
                    })
                } else {
                    json!({ "state": state, "source": online.source })
                }
            }
        }
    }
}

/// A value from `server.properties`.
pub fn property<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    content.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix(key)?.trim_start();
        let value = rest.strip_prefix('=').or_else(|| rest.strip_prefix(':'))?;
        Some(value.trim())
    })
}

fn write_varint(output: &mut Vec<u8>, value: i32) {
    let mut value = value as u32;
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            output.push(byte);
            return;
        }
        output.push(byte | 0x80);
    }
}

/// The VarInt at the start of `bytes` and its length; `None` while its last byte is missing.
fn decode_varint(bytes: &[u8]) -> Result<Option<(i32, usize)>, TunnelError> {
    let mut value: u32 = 0;
    for (index, &byte) in bytes.iter().enumerate() {
        if index == 5 {
            return Err(TunnelError::failed("a VarInt is longer than 5 bytes"));
        }
        value |= u32::from(byte & 0x7F) << (7 * index);
        if byte & 0x80 == 0 {
            return Ok(Some((value as i32, index + 1)));
        }
    }
    Ok(None)
}

async fn read_varint<S: AsyncRead + Unpin>(stream: &mut S) -> Result<i32, TunnelError> {
    let mut bytes = [0; 5];
    for length in 1..=bytes.len() {
        bytes[length - 1] = stream.read_u8().await?;
        if let Some((value, _)) = decode_varint(&bytes[..length])? {
            return Ok(value);
        }
    }
    Err(TunnelError::failed("a VarInt is longer than 5 bytes"))
}

/// The handshake into the status state (protocol -1: any version) and the status request.
fn status_request(host: &str, port: u16) -> Vec<u8> {
    let mut handshake = Vec::new();
    write_varint(&mut handshake, 0x00);
    write_varint(&mut handshake, -1);
    write_varint(&mut handshake, host.len() as i32);
    handshake.extend_from_slice(host.as_bytes());
    handshake.extend_from_slice(&port.to_be_bytes());
    write_varint(&mut handshake, 1);

    let mut request = Vec::with_capacity(handshake.len() + 5);
    write_varint(&mut request, handshake.len() as i32);
    request.extend_from_slice(&handshake);
    request.extend_from_slice(&[0x01, 0x00]);
    request
}

/// A sample entry that names a real player, and its UUID: servers fill the sample with text
/// lines (the nil UUID, `§` formatting, spaces).
fn sample_player(entry: &Value) -> Option<(String, Option<Uuid>)> {
    let name = entry.get("name")?.as_str()?;
    if name.is_empty() || name.contains(['§', ' ']) {
        return None;
    }
    let id = entry.get("id").and_then(Value::as_str);
    if id == Some(NIL_UUID) {
        return None;
    }
    Some((name.to_string(), id.and_then(|id| Uuid::parse_str(id).ok())))
}

/// The players of a status JSON; complete when every player online is in the sample.
fn parse_status(json: &[u8]) -> Result<Online, TunnelError> {
    let status: Value = serde_json::from_slice(json)
        .map_err(|err| TunnelError::failed(format!("unreadable status ({err})")))?;
    let players = status
        .get("players")
        .ok_or_else(|| TunnelError::failed("the status has no player count"))?;
    let number = |key: &str| {
        players
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| TunnelError::failed(format!("the status has no players.{key}")))
    };
    let (count, max) = (number("online")?, number("max")?);
    let (players, ids): (Vec<String>, Vec<Option<Uuid>>) = players
        .get("sample")
        .and_then(Value::as_array)
        .map(|sample| sample.iter().filter_map(sample_player).unzip())
        .unwrap_or_default();
    Ok(Online {
        complete: players.len() == count as usize,
        count,
        max,
        players,
        ids: ids.into_iter().flatten().collect(),
        source: Source::Ping,
    })
}

/// A status response packet (after its length): id 0, then the JSON as a string.
fn parse_response(packet: &[u8]) -> Result<Online, TunnelError> {
    let unreadable = || TunnelError::failed("unreadable status response");
    let (id, id_length) = decode_varint(packet)?.ok_or_else(unreadable)?;
    if id != 0x00 {
        return Err(unreadable());
    }
    let rest = &packet[id_length..];
    let (length, length_length) = decode_varint(rest)?.ok_or_else(unreadable)?;
    let json = usize::try_from(length)
        .ok()
        .and_then(|length| rest.get(length_length..length_length + length))
        .ok_or_else(unreadable)?;
    parse_status(json)
}

async fn status<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    host: &str,
    port: u16,
) -> Result<Online, TunnelError> {
    stream.write_all(&status_request(host, port)).await?;
    stream.flush().await?;
    let length = read_varint(stream).await?;
    let length = usize::try_from(length)
        .ok()
        .filter(|length| (1..=MAX_PACKET).contains(length))
        .ok_or_else(|| TunnelError::failed(format!("status packet length {length}")))?;
    let mut packet = vec![0; length];
    stream.read_exact(&mut packet).await?;
    parse_response(&packet)
}

/// `line` without ANSI escape sequences, `§x` colour codes and other control characters.
fn strip_ansi(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            '§' => {
                chars.next();
            }
            c if c.is_control() => {}
            c => output.push(c),
        }
    }
    output
}

fn split_lines(log: &str) -> Vec<String> {
    log.lines().map(strip_ansi).collect()
}

/// The lines of `current` after `baseline`: the longest suffix of `baseline` that starts
/// `current` is the part both saw.
fn new_lines<'a>(baseline: &[String], current: &'a [String]) -> &'a [String] {
    let longest = baseline.len().min(current.len());
    for overlap in (1..=longest).rev() {
        if baseline[baseline.len() - overlap..] == current[..overlap] {
            return &current[overlap..];
        }
    }
    current
}

/// The message of a log line: without leading `[...]` groups that look like log prefixes
/// (they contain `:` or `/`, which player names cannot), and the `: ` after them.
fn message(line: &str) -> &str {
    let mut rest = line.trim_start();
    let mut stripped = false;
    while rest.starts_with('[') {
        let Some(end) = rest.find(']') else { break };
        if !rest[..end].contains([':', '/']) {
            break;
        }
        rest = rest[end + 1..].trim_start();
        stripped = true;
    }
    if stripped && let Some(after) = rest.strip_prefix(':') {
        rest = after.trim_start();
    }
    rest
}

fn number(text: &str) -> Option<(u32, &str)> {
    let end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    Some((text[..end].parse().ok()?, &text[end..]))
}

/// `message` without a plain label such as Paper's `System chat: ` (letters and spaces, then
/// `: `). Player chat (`<Steve> ...`) never qualifies.
fn without_label(message: &str) -> &str {
    if let Some((label, rest)) = message.split_once(": ")
        && !label.is_empty()
        && label.chars().all(|c| c.is_ascii_alphabetic() || c == ' ')
    {
        rest
    } else {
        message
    }
}

/// `N of a max of M players online: names` (modern) or `N/M players online:` with the names
/// on the next line (legacy): count, max and the names part (`None` for the next line).
fn header(message: &str) -> Option<(u32, u32, Option<&str>)> {
    let message = message
        .strip_prefix("There are ")
        .or_else(|| without_label(message).strip_prefix("There are "))?;
    let (count, rest) = number(message)?;
    if let Some(rest) = rest.strip_prefix(" of a max of ") {
        let (max, rest) = number(rest)?;
        return Some((count, max, Some(rest.strip_prefix(" players online:")?)));
    }
    let (max, rest) = number(rest.strip_prefix('/')?)?;
    let names = rest.strip_prefix(" players online:")?;
    Some((count, max, (!names.trim().is_empty()).then_some(names)))
}

/// `a, b (uuid)` as names.
fn names(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| match name.rfind(" (") {
            Some(open) if name.ends_with(')') => name[..open].to_string(),
            _ => name.to_string(),
        })
        .collect()
}

/// The first complete `list` answer in `lines`.
fn parse_list(lines: &[String]) -> Option<Online> {
    for (index, line) in lines.iter().enumerate() {
        let Some((count, max, inline)) = header(message(line)) else {
            continue;
        };
        let text = match inline {
            Some(text) => text,
            None if count == 0 => "",
            None => message(lines.get(index + 1)?),
        };
        let players = names(text);
        return Some(Online {
            complete: players.len() >= count as usize,
            count,
            max,
            players,
            ids: Vec::new(),
            source: Source::Console,
        });
    }
    None
}

/// Sends `command` and watches the console tail for the answer to it.
async fn ask_list(wings: &Wings, command: &str) -> Result<Option<Online>, ApiResponse> {
    let baseline = split_lines(&wings.logs(TAIL_LINES).await?);
    wings.commands(&[command]).await?;
    let deadline = tokio::time::Instant::now() + ANSWER_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        let current = split_lines(&wings.logs(TAIL_LINES).await?);
        if let Some(answer) = parse_list(new_lines(&baseline, &current)) {
            return Ok(Some(answer));
        }
    }
    Ok(None)
}

/// What the sources tried so far gave: the fullest incomplete answer and the last error.
#[derive(Default)]
struct Attempts {
    partial: Option<Online>,
    error: Option<TunnelError>,
}

impl Attempts {
    /// `online` when it is complete; otherwise kept when it names more players than the
    /// answer kept so far.
    fn answer(&mut self, online: Online) -> Option<Online> {
        if online.complete {
            return Some(online);
        }
        if self
            .partial
            .as_ref()
            .is_none_or(|kept| online.players.len() > kept.players.len())
        {
            self.partial = Some(online);
        }
        None
    }

    fn failed(&mut self, error: TunnelError) {
        self.error = Some(error);
    }

    /// The fullest incomplete answer, else the last error.
    fn finish(self) -> Result<Online, TunnelError> {
        match (self.partial, self.error) {
            (Some(online), _) => Ok(online),
            (None, Some(error)) => Err(error),
            (None, None) => Err(TunnelError::failed("no source answered")),
        }
    }
}

/// The port players join on (`server-port`, else the allocation's) and the query port when
/// `enable-query` is on (`query.port`, else the game port).
fn ports(properties: Option<&str>, allocation_port: Option<u16>) -> (u16, Option<u16>) {
    let content = properties.unwrap_or_default();
    let number = |key: &str| {
        property(content, key)
            .and_then(|port| port.parse::<u16>().ok())
            .filter(|&port| port != 0)
    };
    let game = number("server-port")
        .or(allocation_port)
        .unwrap_or(JAVA_PORT);
    let query = (property(content, "enable-query") == Some("true"))
        .then(|| number("query.port").unwrap_or(game));
    (game, query)
}

/// Asks the sources without side effects: the query port, then the Server List Ping.
async fn ask_quietly(
    wings: &Wings,
    properties: Option<&str>,
    attempts: &mut Attempts,
) -> Option<Online> {
    let (host, allocation_port) = match wings.address() {
        Some((host, port)) => (host.clone(), Some(*port)),
        None => ("localhost".to_string(), None),
    };
    let (port, query_port) = ports(properties, allocation_port);
    if let Some(query_port) = query_port {
        match query::full_stat(wings, query_port).await {
            Ok(stat) => {
                if let Some(online) = attempts.answer(stat.into()) {
                    return Some(online);
                }
            }
            Err(err) => attempts.failed(err),
        }
    }
    let ping = tunnel::bounded(async {
        let mut stream = wings.tcp(port).await?;
        status(&mut stream, &host, port).await
    })
    .await;
    match ping {
        Ok(online) => attempts.answer(online),
        Err(err) => {
            attempts.failed(err);
            None
        }
    }
}

/// The players online as far as the query port and the ping tell, without the console: for
/// the player layer, which refreshes in the background.
pub async fn quietly(wings: &Wings, properties: Option<&str>) -> Result<Online, TunnelError> {
    let mut attempts = Attempts::default();
    match ask_quietly(wings, properties, &mut attempts).await {
        Some(online) => Ok(online),
        None => attempts.finish(),
    }
}

/// The players online: the query port or the ping when they name everyone, else the console
/// (when `console`, the caller may use it; every command sent is returned for the activity
/// log).
pub async fn java(
    wings: &Wings,
    properties: Option<&str>,
    console: bool,
    sent: &mut Vec<&'static str>,
) -> Result<Online, ApiResponse> {
    let mut attempts = Attempts::default();
    if let Some(online) = ask_quietly(wings, properties, &mut attempts).await {
        return Ok(online);
    }
    if console {
        for command in LIST_COMMANDS {
            sent.push(command);
            match ask_list(wings, command).await? {
                Some(answer) => {
                    if let Some(online) = attempts.answer(answer) {
                        return Ok(online);
                    }
                }
                None => attempts.failed(TunnelError::NoAnswer(
                    "the server did not answer the list command".into(),
                )),
            }
        }
    }
    attempts.finish().map_err(TunnelError::response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    fn response(json: &str) -> Vec<u8> {
        let mut packet = vec![0x00];
        write_varint(&mut packet, json.len() as i32);
        packet.extend_from_slice(json.as_bytes());
        packet
    }

    #[test]
    fn varints_and_the_status_request() {
        for (value, bytes) in [
            (0, &[0x00][..]),
            (300, &[0xac, 0x02][..]),
            (-1, &[0xff, 0xff, 0xff, 0xff, 0x0f][..]),
        ] {
            let mut output = Vec::new();
            write_varint(&mut output, value);
            assert_eq!(output, bytes);
            assert_eq!(decode_varint(bytes).unwrap(), Some((value, bytes.len())));
        }
        let mut expected = vec![0x13, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F, 0x09];
        expected.extend_from_slice(b"localhost");
        expected.extend_from_slice(&[0x63, 0xDD, 0x01, 0x01, 0x00]);
        assert_eq!(status_request("localhost", 25565), expected);
    }

    #[test]
    fn ping_samples_name_real_players_only() {
        let online = parse_response(&response(
            r#"{"players":{"max":20,"online":3,"sample":[
                {"name":"Notch","id":"069a79f4-44e9-4726-a5be-fca90e38aaf5"},
                {"name":"§7and 2 more","id":"00000000-0000-0000-0000-000000000000"},
                {"name":"jeb_","id":"853c80ef-3c37-49fd-aa49-938b674adae6"}]}}"#,
        ))
        .unwrap();
        assert_eq!(online.players, vec!["Notch", "jeb_"]);
        assert!(!online.complete);
        assert_eq!(
            online.ids,
            vec![
                Uuid::parse_str("069a79f4-44e9-4726-a5be-fca90e38aaf5").unwrap(),
                Uuid::parse_str("853c80ef-3c37-49fd-aa49-938b674adae6").unwrap(),
            ]
        );
        assert_eq!(online.source, Source::Ping);
        let empty = parse_response(&response(r#"{"players":{"max":10,"online":0}}"#)).unwrap();
        assert!(empty.complete);
    }

    #[tokio::test]
    async fn pings_over_a_stream() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let (online, ()) = tokio::join!(status(&mut client, "localhost", 25565), async {
            let mut request = vec![0; status_request("localhost", 25565).len()];
            server.read_exact(&mut request).await.unwrap();
            let packet = response(r#"{"players":{"max":20,"online":0}}"#);
            let mut framed = Vec::new();
            write_varint(&mut framed, packet.len() as i32);
            framed.extend_from_slice(&packet);
            server.write_all(&framed).await.unwrap();
        });
        assert_eq!(online.unwrap().max, 20);
    }

    #[test]
    fn parses_list_answers() {
        let modern = owned(&[
            "[12:00:00 INFO]: <Steve> There are 5 of a max of 5 players online: fake",
            "\u{1b}[0m[12:00:00] [Server thread/INFO]: There are 2 of a max of 20 players online: Notch (069a79f4-44e9-4726-a5be-fca90e38aaf5), jeb_",
        ]);
        let modern: Vec<String> = modern.iter().map(|line| strip_ansi(line)).collect();
        assert_eq!(
            parse_list(&modern),
            Some(Online {
                count: 2,
                max: 20,
                players: vec!["Notch".into(), "jeb_".into()],
                ids: Vec::new(),
                complete: true,
                source: Source::Console,
            })
        );
        let legacy = owned(&[
            "[12:00:00] [Server thread/INFO]: There are 1/20 players online:",
            "[12:00:00] [Server thread/INFO]: Alex",
        ]);
        assert_eq!(parse_list(&legacy).unwrap().players, vec!["Alex"]);
        let empty = owned(&["[19:20:24 INFO]: System chat: There are 0 of a max of 20 players online: "]);
        assert_eq!(parse_list(&empty).unwrap().count, 0);
        assert_eq!(parse_list(&owned(&["There are 1/10 players online:"])), None);
    }

    #[test]
    fn finds_lines_after_the_baseline() {
        let baseline = owned(&["a", "b", "c"]);
        assert_eq!(new_lines(&baseline, &owned(&["b", "c", "d"])), owned(&["d"]));
        assert!(new_lines(&baseline, &baseline).is_empty());
        assert_eq!(new_lines(&baseline, &owned(&["x"])), owned(&["x"]));
    }

    #[test]
    fn reads_properties() {
        let content = "motd=hi\nserver-port=25570\nserver-ip=\n";
        assert_eq!(property(content, "server-port"), Some("25570"));
        assert_eq!(property(content, "server-ip"), Some(""));
        assert_eq!(property(content, "query.port"), None);
    }

    #[test]
    fn query_only_when_enabled() {
        assert_eq!(ports(None, None), (25565, None));
        assert_eq!(ports(Some("server-port=0\n"), Some(25570)), (25570, None));
        assert_eq!(
            ports(Some("server-port=25580\nenable-query=true\n"), Some(25570)),
            (25580, Some(25580))
        );
        assert_eq!(
            ports(Some("enable-query=true\nquery.port=25600\n"), Some(25570)),
            (25570, Some(25600))
        );
        assert_eq!(
            ports(Some("enable-query=false\nquery.port=25600\n"), None),
            (25565, None)
        );
    }

    fn online(players: &[&str], ids: &[u128], count: u32, source: Source) -> Online {
        Online {
            count,
            max: 20,
            players: players.iter().map(|name| name.to_string()).collect(),
            ids: ids.iter().copied().map(Uuid::from_u128).collect(),
            complete: players.len() >= count as usize,
            source,
        }
    }

    #[test]
    fn the_fullest_answer_wins() {
        let mut attempts = Attempts::default();
        assert!(attempts.answer(online(&["a"], &[], 3, Source::Ping)).is_none());
        attempts.failed(TunnelError::failed("query: no answer"));
        assert!(attempts.answer(online(&[], &[], 3, Source::Console)).is_none());
        let kept = attempts.finish().unwrap();
        assert_eq!((kept.players.len(), kept.source), (1, Source::Ping));

        let mut attempts = Attempts::default();
        attempts.failed(TunnelError::failed("ping: refused"));
        assert_eq!(
            attempts.finish().unwrap_err(),
            TunnelError::failed("ping: refused")
        );

        let mut attempts = Attempts::default();
        let complete = attempts.answer(online(&["a", "b"], &[], 2, Source::Query));
        assert_eq!(complete.unwrap().source, Source::Query);
    }

    #[test]
    fn players_are_online_only_when_named() {
        let steve = Uuid::from_u128(1);
        let alex = Uuid::from_u128(2);
        let unnamed = Uuid::from_u128(3);

        let answered = Presence::Answered(online(&["Steve"], &[3], 2, Source::Ping));
        assert!(answered.is_online(steve, Some("Steve")));
        // names are matched like the game does, ignoring case
        assert!(answered.is_online(steve, Some("steve")));
        // the sample's UUID places a player usercache.json has no name for
        assert!(answered.is_online(unnamed, None));
        // not named in an incomplete answer: counted offline
        assert!(!answered.is_online(alex, Some("Alex")));
        assert!(!answered.is_online(Uuid::from_u128(9), None));

        for nobody in [Presence::NotRunning, Presence::Unknown("ping: refused".into())] {
            assert!(!nobody.is_online(steve, Some("Steve")));
        }
    }

    #[test]
    fn the_summary_hides_counts_with_online_players() {
        let partial = Presence::Answered(online(&["Steve"], &[], 4, Source::Ping));
        assert_eq!(
            partial.summary(true),
            json!({ "state": "partial", "source": "ping", "count": 4, "max": 20, "named": 1 })
        );
        assert_eq!(
            partial.summary(false),
            json!({ "state": "partial", "source": "ping" })
        );
        let known = Presence::Answered(online(&["Steve"], &[], 1, Source::Query));
        assert_eq!(known.summary(true)["state"], "known");
        assert_eq!(
            Presence::NotRunning.summary(true),
            json!({ "state": "not_running" })
        );
        assert_eq!(
            Presence::Unknown("no answer in time".into()).summary(false),
            json!({ "state": "unknown", "error": "no answer in time" })
        );
    }
}
