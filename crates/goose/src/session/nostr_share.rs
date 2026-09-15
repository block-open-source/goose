use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use nostr::nips::nip19::{FromBech32, Nip19Event, ToBech32};
use nostr::nips::nip44;
use nostr::prelude::*;
use nostr_sdk::client::Client;

use crate::config::{Config, ConfigError};

pub const EVENT_KIND: u16 = 30278;
pub const CONFIG_RELAYS_KEY: &str = "GOOSE_NOSTR_RELAYS";

const DEFAULT_RELAYS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://relay.primal.net",
    "wss://nos.lol",
    "wss://relay.nostr.band",
];
const MAX_RELAYS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NostrShare {
    pub deeplink: String,
    pub nevent: String,
    pub event_id: String,
    pub relays: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedShareLink {
    pub nevent: String,
    pub decryption_key: String,
}

#[async_trait]
pub trait NostrPublisher {
    async fn publish(&self, event: Event, relays: &[String]) -> Result<()>;
}

#[async_trait]
pub trait NostrFetcher {
    async fn fetch(&self, event_id: EventId, relays: &[String]) -> Result<Event>;
}

pub struct LiveNostrClient;

#[async_trait]
impl NostrPublisher for LiveNostrClient {
    async fn publish(&self, event: Event, relays: &[String]) -> Result<()> {
        install_rustls_crypto_provider();
        let client = Client::default();
        for relay in relays {
            client
                .add_relay(relay)
                .await
                .with_context(|| format!("Failed to add relay {relay}"))?;
        }

        client.try_connect().timeout(Duration::from_secs(8)).await;
        let output = client
            .send_event(&event)
            .to(relays.iter().map(String::as_str))
            .await
            .context("Failed to publish session to Nostr relays")?;
        client.shutdown().await;

        if output.success.is_empty() {
            return Err(anyhow!(
                "Failed to publish session to any Nostr relay: {:?}",
                output.failed
            ));
        }

        Ok(())
    }
}

#[async_trait]
impl NostrFetcher for LiveNostrClient {
    async fn fetch(&self, event_id: EventId, relays: &[String]) -> Result<Event> {
        install_rustls_crypto_provider();
        let client = Client::default();
        for relay in relays {
            client
                .add_relay(relay)
                .await
                .with_context(|| format!("Failed to add relay {relay}"))?;
        }

        client.try_connect().timeout(Duration::from_secs(8)).await;
        let filter = Filter::new()
            .id(event_id)
            .kind(Kind::Custom(EVENT_KIND))
            .limit(1);
        let targets = relays
            .iter()
            .map(|relay| (relay.as_str(), vec![filter.clone()]))
            .collect::<Vec<_>>();
        let events = client
            .fetch_events(targets)
            .timeout(Duration::from_secs(10))
            .await
            .context("Failed to fetch shared session from Nostr relays")?;
        client.shutdown().await;

        events
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("Shared session event not found"))
    }
}

#[cfg(feature = "rustls-tls")]
fn install_rustls_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[cfg(not(feature = "rustls-tls"))]
fn install_rustls_crypto_provider() {}

pub fn default_relays() -> Vec<String> {
    DEFAULT_RELAYS
        .iter()
        .map(|relay| relay.to_string())
        .collect()
}

pub fn relays_from_config(config: &Config) -> Vec<String> {
    match config.get_param::<Vec<String>>(CONFIG_RELAYS_KEY) {
        Ok(relays) if !relays.is_empty() => normalize_relays(relays),
        Err(ConfigError::NotFound(_)) => default_relays(),
        _ => default_relays(),
    }
}

pub fn resolve_relays(cli_relays: Vec<String>, config: &Config) -> Vec<String> {
    if cli_relays.is_empty() {
        relays_from_config(config)
    } else {
        normalize_relays(cli_relays)
    }
}

pub async fn publish_session_json(session_json: &str, relays: Vec<String>) -> Result<NostrShare> {
    publish_session_json_with(session_json, relays, &LiveNostrClient).await
}

pub async fn publish_session_json_with<P>(
    session_json: &str,
    relays: Vec<String>,
    publisher: &P,
) -> Result<NostrShare>
where
    P: NostrPublisher + Sync,
{
    let relays = normalize_relays(relays);
    if relays.is_empty() {
        return Err(anyhow!("At least one Nostr relay is required"));
    }
    if relays.len() > MAX_RELAYS {
        return Err(anyhow!("Too many Nostr relays (maximum {MAX_RELAYS})"));
    }
    let relay_urls = relays
        .iter()
        .map(|relay| RelayUrl::parse(relay))
        .collect::<Result<Vec<_>, _>>()?;

    let publish_keys = Keys::generate();
    let encryption_key = SecretKey::generate();
    let encryption_keys = Keys::new(encryption_key.clone());
    let encrypted = nip44::encrypt(
        &encryption_key,
        &encryption_keys.public_key(),
        session_json,
        nip44::Version::V2,
    )?;

    let event = EventBuilder::new(Kind::Custom(EVENT_KIND), encrypted)
        .tag(Tag::identifier(format!(
            "goose-session-{}",
            uuid::Uuid::now_v7()
        )))
        .tag(Tag::parse(["client", "goose"])?)
        .finalize(&publish_keys)?;

    publisher.publish(event.clone(), &relays).await?;

    let nevent = Nip19Event::new(event.id)
        .author(event.pubkey)
        .kind(Kind::Custom(EVENT_KIND))
        .relays(relay_urls)
        .to_bech32()?;
    let decryption_key = encryption_key.to_secret_hex();
    let deeplink = build_deeplink(&nevent, &decryption_key);

    Ok(NostrShare {
        deeplink,
        nevent,
        event_id: event.id.to_hex(),
        relays,
    })
}

pub async fn import_session_json_from_deeplink(deeplink: &str) -> Result<String> {
    import_session_json_from_deeplink_with(deeplink, &LiveNostrClient).await
}

pub async fn import_session_json_from_deeplink_with<F>(
    deeplink: &str,
    fetcher: &F,
) -> Result<String>
where
    F: NostrFetcher + Sync,
{
    let ParsedShareLink {
        nevent,
        decryption_key,
    } = parse_deeplink(deeplink)?;
    let event_ref = Nip19Event::from_bech32(&nevent)?;
    let relays = normalize_import_relays(event_ref.relays.iter().map(ToString::to_string))?;

    if relays.is_empty() {
        return Err(anyhow!("Shared session link does not include any relays"));
    }

    let event = fetcher.fetch(event_ref.event_id, &relays).await?;
    if event.kind != Kind::Custom(EVENT_KIND) {
        return Err(anyhow!(
            "Unexpected Nostr event kind: {}",
            u16::from(event.kind)
        ));
    }

    let secret_key = SecretKey::parse(&decryption_key)?;
    let encryption_keys = Keys::new(secret_key.clone());
    nip44::decrypt(&secret_key, &encryption_keys.public_key(), event.content).map_err(Into::into)
}

pub fn build_deeplink(nevent: &str, decryption_key: &str) -> String {
    format!(
        "goose://sessions/nostr?nevent={}&key={}",
        urlencoding::encode(nevent),
        urlencoding::encode(decryption_key)
    )
}

pub fn parse_deeplink(deeplink: &str) -> Result<ParsedShareLink> {
    let parsed = url::Url::parse(deeplink).context("Invalid Goose session share link")?;
    if parsed.scheme() != "goose"
        || parsed.host_str() != Some("sessions")
        || parsed.path() != "/nostr"
    {
        return Err(anyhow!("Invalid Goose Nostr session share link"));
    }

    let nevent = parsed
        .query_pairs()
        .find_map(|(key, value)| (key == "nevent").then(|| value.into_owned()))
        .ok_or_else(|| anyhow!("Missing nevent parameter"))?;
    let decryption_key = parsed
        .query_pairs()
        .find_map(|(key, value)| (key == "key").then(|| value.into_owned()))
        .ok_or_else(|| anyhow!("Missing decryption key parameter"))?;

    Ok(ParsedShareLink {
        nevent,
        decryption_key,
    })
}

fn normalize_relays(relays: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for relay in relays {
        let relay = relay.trim();
        if relay.is_empty() || normalized.iter().any(|existing| existing == relay) {
            continue;
        }
        normalized.push(relay.to_string());
    }
    normalized
}

fn normalize_import_relays(relays: impl IntoIterator<Item = String>) -> Result<Vec<String>> {
    let mut normalized = Vec::new();
    for relay in relays {
        let relay = relay.trim();
        if relay.is_empty() || normalized.iter().any(|existing| existing == relay) {
            continue;
        }
        if normalized.len() == MAX_RELAYS {
            return Err(anyhow!(
                "Shared session link includes too many relays (maximum {MAX_RELAYS})"
            ));
        }
        normalized.push(relay.to_string());
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct RecordingPublisher {
        event: Arc<Mutex<Option<Event>>>,
        relays: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl NostrPublisher for RecordingPublisher {
        async fn publish(&self, event: Event, relays: &[String]) -> Result<()> {
            *self.event.lock().unwrap() = Some(event);
            *self.relays.lock().unwrap() = relays.to_vec();
            Ok(())
        }
    }

    struct StaticFetcher(Event);

    #[async_trait]
    impl NostrFetcher for StaticFetcher {
        async fn fetch(&self, _event_id: EventId, _relays: &[String]) -> Result<Event> {
            Ok(self.0.clone())
        }
    }

    struct RecordingFetcher {
        event: Event,
        relays: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl NostrFetcher for RecordingFetcher {
        async fn fetch(&self, _event_id: EventId, relays: &[String]) -> Result<Event> {
            *self.relays.lock().unwrap() = relays.to_vec();
            Ok(self.event.clone())
        }
    }

    #[tokio::test]
    async fn publish_builds_deeplink_and_encrypted_kind_30278_event() {
        let event = Arc::new(Mutex::new(None));
        let relays = Arc::new(Mutex::new(Vec::new()));
        let publisher = RecordingPublisher {
            event: event.clone(),
            relays: relays.clone(),
        };

        let share = publish_session_json_with(
            r#"{"id":"session-id","conversation":{"messages":[]}}"#,
            vec!["wss://relay.example".to_string()],
            &publisher,
        )
        .await
        .unwrap();

        assert!(share.deeplink.starts_with("goose://sessions/nostr?"));
        assert!(share.nevent.starts_with("nevent1"));
        assert_eq!(share.relays, vec!["wss://relay.example"]);
        assert_eq!(*relays.lock().unwrap(), vec!["wss://relay.example"]);

        let event = event.lock().unwrap().clone().unwrap();
        assert_eq!(event.kind, Kind::Custom(EVENT_KIND));
        assert_ne!(
            event.content,
            r#"{"id":"session-id","conversation":{"messages":[]}}"#
        );
    }

    #[tokio::test]
    async fn publish_and_import_round_trips_session_json() {
        let event = Arc::new(Mutex::new(None));
        let publisher = RecordingPublisher {
            event: event.clone(),
            relays: Arc::new(Mutex::new(Vec::new())),
        };
        let json = r#"{"id":"session-id","name":"shared"}"#;

        let share =
            publish_session_json_with(json, vec!["wss://relay.example".to_string()], &publisher)
                .await
                .unwrap();

        let fetched_event = event.lock().unwrap().clone().unwrap();
        let imported =
            import_session_json_from_deeplink_with(&share.deeplink, &StaticFetcher(fetched_event))
                .await
                .unwrap();

        assert_eq!(imported, json);
    }

    #[tokio::test]
    async fn import_rejects_too_many_unique_relays_before_fetch() {
        let event = EventBuilder::new(Kind::Custom(EVENT_KIND), "encrypted")
            .finalize(&Keys::generate())
            .unwrap();
        let relay_hints = (0..=MAX_RELAYS)
            .map(|index| format!("wss://relay-{index}.example"))
            .map(|relay| RelayUrl::parse(&relay).unwrap())
            .collect::<Vec<_>>();
        let nevent = Nip19Event::new(event.id)
            .author(event.pubkey)
            .kind(Kind::Custom(EVENT_KIND))
            .relays(relay_hints)
            .to_bech32()
            .unwrap();
        let share_link = build_deeplink(&nevent, &SecretKey::generate().to_secret_hex());
        let fetched_relays = Arc::new(Mutex::new(Vec::new()));
        let fetcher = RecordingFetcher {
            event,
            relays: fetched_relays.clone(),
        };

        let error = import_session_json_from_deeplink_with(&share_link, &fetcher)
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Shared session link includes too many relays (maximum 16)"
        );
        assert!(fetched_relays.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn publish_rejects_too_many_unique_relays_before_publish() {
        let event = Arc::new(Mutex::new(None));
        let published_relays = Arc::new(Mutex::new(Vec::new()));
        let publisher = RecordingPublisher {
            event: event.clone(),
            relays: published_relays.clone(),
        };
        let relays = (0..=MAX_RELAYS)
            .map(|index| format!("wss://relay-{index}.example"))
            .collect();

        let error = publish_session_json_with("{}", relays, &publisher)
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "Too many Nostr relays (maximum 16)");
        assert!(event.lock().unwrap().is_none());
        assert!(published_relays.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn import_deduplicates_relay_hints() {
        let event = Arc::new(Mutex::new(None));
        let publisher = RecordingPublisher {
            event: event.clone(),
            relays: Arc::new(Mutex::new(Vec::new())),
        };
        let share = publish_session_json_with(
            r#"{"id":"session-id"}"#,
            vec!["wss://relay.example".to_string()],
            &publisher,
        )
        .await
        .unwrap();
        let parsed = parse_deeplink(&share.deeplink).unwrap();
        let shared_event = event.lock().unwrap().clone().unwrap();
        let relay = RelayUrl::parse("wss://relay.example").unwrap();
        let nevent = Nip19Event::new(shared_event.id)
            .author(shared_event.pubkey)
            .kind(Kind::Custom(EVENT_KIND))
            .relays(vec![relay.clone(), relay])
            .to_bech32()
            .unwrap();
        let duplicate_link = build_deeplink(&nevent, &parsed.decryption_key);
        let fetched_relays = Arc::new(Mutex::new(Vec::new()));
        let fetcher = RecordingFetcher {
            event: shared_event,
            relays: fetched_relays.clone(),
        };

        let imported = import_session_json_from_deeplink_with(&duplicate_link, &fetcher)
            .await
            .unwrap();

        assert_eq!(imported, r#"{"id":"session-id"}"#);
        assert_eq!(*fetched_relays.lock().unwrap(), vec!["wss://relay.example"]);
    }

    #[test]
    fn parses_deeplink() {
        let parsed = parse_deeplink("goose://sessions/nostr?nevent=abc&key=def").unwrap();
        assert_eq!(parsed.nevent, "abc");
        assert_eq!(parsed.decryption_key, "def");
    }
}
