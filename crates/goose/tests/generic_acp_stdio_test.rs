#![recursion_limit = "256"]

//! Behavioural coverage for the generic custom-ACP provider route.
//!
//! These tests launch a real stdio ACP agent (tests/fake_acp_agent.py) through
//! the same registry path the CLI and desktop use, then assert on what the agent
//! observed: its argv, environment, session working directory, forwarded MCP
//! servers, mode selection, and prompt traffic. Nothing here reaches into
//! provider internals, so the assertions stay meaningful if the transport is
//! rewritten.

use futures::StreamExt;
use goose::agents::extension::Envs;
use goose::agents::ExtensionConfig;
use goose::config::DeclarativeProviderConfig;
use goose::conversation::message::{Message, MessageContent};
use goose::providers::base::{Provider, ProviderType};
use goose::providers::pi_acp::PiAcpProvider;
use goose::providers::provider_registry::ProviderRegistry;
use goose_providers::errors::ProviderError;
use goose_providers::model::ModelConfig;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const REPLY: &str = "generic-acp-reply";
const WAIT_LIMIT: Duration = Duration::from_secs(15);

fn agent_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fake_acp_agent.py")
}

/// The system interpreter to launch the fixture agent with. Returns None when no
/// interpreter is available so the test can skip instead of failing.
fn python_interpreter() -> Option<String> {
    ["python3", "python"].into_iter().find_map(|candidate| {
        match Command::new(candidate).arg("--version").output() {
            Ok(output) if output.status.success() => Some(candidate.to_string()),
            _ => None,
        }
    })
}

/// Skips the test when the fixture agent cannot be launched at all.
fn python_or_skip() -> Option<String> {
    let interpreter = python_interpreter();
    if interpreter.is_none() {
        eprintln!("skipping: no python interpreter available for the ACP fixture");
    }
    interpreter
}

fn read_transcript(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn wait_for_transcript(path: &Path, describe: &str, ready: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + WAIT_LIMIT;
    while Instant::now() < deadline {
        if let Some(value) = read_transcript(path) {
            if ready(&value) {
                return value;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for the fake ACP agent to report {describe}");
}

fn read_ticks(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// The persisted shape of a generic ACP custom provider, with the launch details
/// supplied as the `acp` block.
fn acp_config(name: &str, command: &str, acp: Value) -> DeclarativeProviderConfig {
    let mut acp = acp;
    acp.as_object_mut()
        .expect("ACP launch details should be a JSON object")
        .insert("command".to_string(), Value::String(command.to_string()));

    let config: DeclarativeProviderConfig = serde_json::from_value(json!({
        "name": name,
        "engine": "acp",
        "display_name": "Generic stdio ACP",
        "base_url": "",
        "models": [],
        "requires_auth": false,
        "acp": acp,
    }))
    .expect("ACP custom provider config should deserialize");
    config
        .validate_auth()
        .expect("ACP custom provider config should validate");
    config
}

fn registry_with(config: &DeclarativeProviderConfig) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new(None);
    registry.register_generic_acp(config, ProviderType::Custom);
    registry
}

/// The full error chain: these messages are what a user sees when a provider
/// cannot start, so the underlying cause has to survive the context layers.
async fn connect_error(registry: &ProviderRegistry, name: &str) -> String {
    match registry.create(name, vec![]).await {
        Ok(_) => panic!("provider '{name}' should not have connected"),
        Err(error) => format!("{error:#}"),
    }
}

/// Drives one prompt through the provider and returns the streamed text, or the
/// provider error that stopped it.
async fn prompt(provider: &dyn Provider, text: &str) -> Result<String, ProviderError> {
    let model = ModelConfig::new("fake-model-v1");
    let messages = vec![Message::user().with_text(text.to_string())];
    let stream = goose::session_context::with_session_id(
        Some("generic-acp-test-session".to_string()),
        provider.stream(&model, "", &messages, &[]),
    )
    .await?;

    let mut stream = stream;
    let mut collected = String::new();
    while let Some(item) = stream.next().await {
        let (message, _usage) = item?;
        if let Some(message) = message {
            for content in message.content {
                if let MessageContent::Text(text) = content {
                    collected.push_str(&text.text);
                }
            }
        }
    }
    Ok(collected)
}

fn mcp_stdio_extension() -> ExtensionConfig {
    ExtensionConfig::Stdio {
        name: "fake-mcp".to_string(),
        description: String::new(),
        cmd: "/opt/fake-mcp/server".to_string(),
        args: vec!["--serve".to_string()],
        envs: Envs::new(HashMap::from([(
            "MCP_TOKEN".to_string(),
            "mcp-secret".to_string(),
        )])),
        env_keys: vec![],
        timeout: None,
        cwd: None,
        bundled: None,
        available_tools: vec![],
    }
}

#[tokio::test]
async fn generic_acp_provider_launches_configured_stdio_agent() {
    let Some(python) = python_or_skip() else {
        return;
    };

    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let _env = env_lock::lock_env([
        ("GOOSE_PATH_ROOT", Some(root_path.as_str())),
        ("GOOSE_DISABLE_KEYRING", Some("1")),
        ("FAKE_ACP_REMOVED", Some("must-not-reach-the-agent")),
    ]);

    let launch_dir = tempfile::tempdir().unwrap();
    let transcript_dir = tempfile::tempdir().unwrap();
    let transcript = transcript_dir.path().join("transcript.json");
    let heartbeat = transcript_dir.path().join("heartbeat");

    let script = agent_script();
    let config = acp_config(
        "custom_generic_acp",
        &python,
        json!({
            "args": [script.to_string_lossy(), "--mode", "a b; echo pwned"],
            "env": [
                ["FAKE_ACP_RECORD", transcript.display().to_string()],
                ["FAKE_ACP_REPLY", REPLY],
                ["FAKE_ACP_HEARTBEAT", heartbeat.display().to_string()],
                ["FAKE_ACP_MARKER", "configured-value"],
                ["FAKE_ACP_WATCH_ENV", "FAKE_ACP_MARKER,FAKE_ACP_REMOVED,FAKE_ACP_MODE"],
            ],
            "env_remove": ["FAKE_ACP_REMOVED"],
            "work_dir": launch_dir.path().to_string_lossy(),
            "model_config_option_id": "model",
            "session_config_options": [["model", "fake-model-v1"]],
        }),
    );

    let registry = registry_with(&config);
    let provider = registry
        .create(&config.name, vec![mcp_stdio_extension()])
        .await
        .expect("generic ACP provider should connect to the fixture agent");
    assert_eq!(provider.get_name(), "custom_generic_acp");

    let observed = wait_for_transcript(&transcript, "its launch details", |value| {
        value.get("sessionNew").is_some()
    });

    assert_eq!(
        observed["argv"],
        json!(["--mode", "a b; echo pwned"]),
        "the configured argument vector should reach the agent verbatim, unparsed by a shell"
    );
    assert!(
        observed["interpreter"]
            .as_str()
            .is_some_and(|interpreter| interpreter.contains("python")),
        "the configured command should be the launched interpreter"
    );
    assert_eq!(
        observed["cwd"].as_str(),
        Some(std::env::current_dir().unwrap().to_string_lossy().as_ref()),
        "the agent inherits goose's working directory; the configured work_dir is the session cwd"
    );
    assert_eq!(
        observed["env"]["FAKE_ACP_MARKER"],
        json!("configured-value"),
        "configured environment additions should be forwarded"
    );
    assert_eq!(
        observed["env"]["FAKE_ACP_REMOVED"],
        Value::Null,
        "env_remove entries should be stripped from the agent environment"
    );
    assert_eq!(
        observed["sessionNew"]["cwd"].as_str(),
        Some(launch_dir.path().to_string_lossy().as_ref()),
        "the configured work_dir should be the ACP session working directory"
    );
    assert_eq!(
        observed["sessionNew"]["mcpServers"],
        json!([{
            "name": "fake-mcp",
            "command": "/opt/fake-mcp/server",
            "args": ["--serve"],
            "env": [{ "name": "MCP_TOKEN", "value": "mcp-secret" }],
        }]),
        "configured goose extensions should be forwarded to the agent as MCP servers"
    );
    assert_eq!(
        observed["configOptions"],
        json!([{
            "sessionId": "fake-session-1",
            "configId": "model",
            "value": "fake-model-v1",
        }]),
        "structured session config options should be applied when the session is created"
    );

    let streamed = prompt(provider.as_ref(), "hello generic acp")
        .await
        .expect("the fixture agent should answer a prompt");
    assert_eq!(streamed, REPLY);

    let after_prompt = read_transcript(&transcript).expect("transcript should persist");
    assert_eq!(
        after_prompt["prompts"],
        json!([{ "sessionId": "fake-session-1", "text": "hello generic acp" }]),
        "the prompt should reach the agent unchanged"
    );
    assert_eq!(
        after_prompt["configOptions"]
            .as_array()
            .map(|entries| entries.len()),
        Some(1),
        "the active model should not be re-sent when it already matches"
    );
}

#[tokio::test]
async fn generic_acp_provider_uses_supplied_working_directory_when_unconfigured() {
    let Some(python) = python_or_skip() else {
        return;
    };

    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let _env = env_lock::lock_env([
        ("GOOSE_PATH_ROOT", Some(root_path.as_str())),
        ("GOOSE_DISABLE_KEYRING", Some("1")),
    ]);

    let session_dir = tempfile::tempdir().unwrap();
    let transcript_dir = tempfile::tempdir().unwrap();
    let transcript = transcript_dir.path().join("transcript.json");

    let script = agent_script();
    let config = acp_config(
        "custom_generic_acp_default_dir",
        &python,
        json!({
            "args": [script.to_string_lossy()],
            "env": [["FAKE_ACP_RECORD", transcript.display().to_string()]],
        }),
    );

    let registry = registry_with(&config);
    let entry = registry
        .entry(&config.name)
        .expect("registering a generic ACP provider should create an entry");
    let provider = entry
        .create_with_working_dir(vec![], session_dir.path().to_path_buf())
        .await
        .expect("generic ACP provider should connect");

    let observed = wait_for_transcript(&transcript, "its session", |value| {
        value.get("sessionNew").is_some()
    });
    assert_eq!(
        observed["sessionNew"]["cwd"].as_str(),
        Some(session_dir.path().to_string_lossy().as_ref()),
        "callers that pass a working directory should override the unconfigured one"
    );
    assert_eq!(
        observed["sessionNew"]["mcpServers"],
        json!([]),
        "no extensions were requested for this session"
    );
    drop(provider);
}

/// A Pi-shaped custom configuration must run through the generic route rather
/// than being folded into the compiled `pi-acp` provider: the two providers have
/// to coexist under their own identifiers, and the custom one has to launch the
/// command the user configured.
#[tokio::test]
async fn pi_shaped_custom_acp_config_stays_distinct_from_builtin_pi_acp() {
    let Some(python) = python_or_skip() else {
        return;
    };

    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let _env = env_lock::lock_env([
        ("GOOSE_PATH_ROOT", Some(root_path.as_str())),
        ("GOOSE_DISABLE_KEYRING", Some("1")),
    ]);

    let workspace = tempfile::tempdir().unwrap();
    let transcript_dir = tempfile::tempdir().unwrap();
    let transcript = transcript_dir.path().join("transcript.json");

    let script = agent_script();
    let config = acp_config(
        "pi-custom-acp",
        &python,
        json!({
            "args": [script.to_string_lossy()],
            "env": [
                ["FAKE_ACP_RECORD", transcript.display().to_string()],
                ["FAKE_ACP_REPLY", REPLY],
            ],
            "work_dir": workspace.path().to_string_lossy(),
        }),
    );

    let mut registry = ProviderRegistry::new(None);
    registry.register::<PiAcpProvider>(false);
    registry.register_generic_acp(&config, ProviderType::Custom);

    let builtin = registry
        .entry("pi-acp")
        .expect("the compiled Pi provider should still be registered");
    let custom = registry
        .entry("pi-custom-acp")
        .expect("the Pi-shaped custom provider should have its own entry");
    assert_eq!(builtin.provider_type(), ProviderType::Builtin);
    assert_eq!(custom.provider_type(), ProviderType::Custom);
    assert_ne!(
        builtin.metadata().display_name,
        custom.metadata().display_name
    );
    assert_eq!(custom.inventory_identity().unwrap().provider_family, "acp");

    let provider = registry
        .create("pi-custom-acp", vec![])
        .await
        .expect("the custom provider should launch its configured command");
    assert_eq!(provider.get_name(), "pi-custom-acp");

    let observed = wait_for_transcript(&transcript, "its launch details", |value| {
        value.get("sessionNew").is_some()
    });
    assert!(
        observed["interpreter"]
            .as_str()
            .is_some_and(|interpreter| interpreter.contains("python")),
        "the custom route should run the configured command instead of the compiled pi-acp binary"
    );
    assert_eq!(
        prompt(provider.as_ref(), "pi-shaped prompt")
            .await
            .expect("the Pi-shaped custom provider should answer a prompt"),
        REPLY,
        "the Pi-shaped custom provider should use the generic runtime end to end"
    );
}

fn mcp_http_extension() -> ExtensionConfig {
    ExtensionConfig::StreamableHttp {
        name: "fake-http-mcp".to_string(),
        description: String::new(),
        uri: "https://mcp.example/mcp".to_string(),
        envs: Default::default(),
        env_keys: vec![],
        headers: HashMap::new(),
        timeout: None,
        socket: None,
        client_id: None,
        client_secret_key: None,
        scopes: vec![],
        bundled: None,
        available_tools: vec![],
    }
}

/// Agents decide which MCP transports they can host; goose must forward only the
/// ones the agent advertised during initialization.
#[tokio::test]
async fn generic_acp_mcp_forwarding_follows_agent_capabilities() {
    let Some(python) = python_or_skip() else {
        return;
    };

    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let _env = env_lock::lock_env([
        ("GOOSE_PATH_ROOT", Some(root_path.as_str())),
        ("GOOSE_DISABLE_KEYRING", Some("1")),
    ]);

    let workspace = tempfile::tempdir().unwrap();
    let transcript_dir = tempfile::tempdir().unwrap();
    let script = agent_script();
    let stdio_server = json!({
        "name": "fake-mcp",
        "command": "/opt/fake-mcp/server",
        "args": ["--serve"],
        "env": [{ "name": "MCP_TOKEN", "value": "mcp-secret" }],
    });
    let http_server = json!({
        "type": "http",
        "name": "fake-http-mcp",
        "url": "https://mcp.example/mcp",
        "headers": [],
    });

    for (name, advertise_http, expected) in [
        (
            "custom_acp_capabilities_none",
            false,
            json!([stdio_server.clone()]),
        ),
        (
            "custom_acp_capabilities_http",
            true,
            json!([stdio_server.clone(), http_server.clone()]),
        ),
    ] {
        let transcript = transcript_dir.path().join(format!("{name}.json"));
        let mut env = vec![
            (
                "FAKE_ACP_RECORD".to_string(),
                transcript.display().to_string(),
            ),
            (
                "FAKE_ACP_WATCH_ENV".to_string(),
                "FAKE_ACP_MCP_HTTP".to_string(),
            ),
        ];
        if advertise_http {
            env.push(("FAKE_ACP_MCP_HTTP".to_string(), "1".to_string()));
        }
        let config = acp_config(
            name,
            &python,
            json!({
                "args": [script.to_string_lossy()],
                "env": env,
                "work_dir": workspace.path().to_string_lossy(),
            }),
        );

        let provider = registry_with(&config)
            .create(
                &config.name,
                vec![mcp_stdio_extension(), mcp_http_extension()],
            )
            .await
            .expect("generic ACP provider should connect");

        let observed = wait_for_transcript(&transcript, "its session", |value| {
            value.get("sessionNew").is_some()
        });
        assert_eq!(
            observed["sessionNew"]["mcpServers"], expected,
            "{name}: MCP servers should be filtered by the agent's advertised capabilities"
        );
        drop(provider);
    }
}

/// Resuming must move the provider onto the saved ACP session and retire the one
/// it created during connect.
#[tokio::test]
async fn generic_acp_provider_resumes_a_recorded_session() {
    let Some(python) = python_or_skip() else {
        return;
    };

    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let _env = env_lock::lock_env([
        ("GOOSE_PATH_ROOT", Some(root_path.as_str())),
        ("GOOSE_DISABLE_KEYRING", Some("1")),
    ]);

    let workspace = tempfile::tempdir().unwrap();
    let transcript_dir = tempfile::tempdir().unwrap();
    let transcript = transcript_dir.path().join("transcript.json");

    let script = agent_script();
    let config = acp_config(
        "custom_acp_resume",
        &python,
        json!({
            "args": [script.to_string_lossy()],
            "env": [
                ["FAKE_ACP_RECORD", transcript.display().to_string()],
                ["FAKE_ACP_REPLY", REPLY],
                ["FAKE_ACP_SESSION_RESUME", "1"],
            ],
            "work_dir": workspace.path().to_string_lossy(),
        }),
    );

    let provider = registry_with(&config)
        .create(&config.name, vec![])
        .await
        .expect("generic ACP provider should connect");
    assert_eq!(
        provider.provider_session_id().as_deref(),
        Some("fake-session-1"),
        "connect should create the initial session"
    );

    provider
        .resume("prior-session")
        .await
        .expect("resuming a saved session should succeed");
    assert_eq!(
        provider.provider_session_id().as_deref(),
        Some("prior-session")
    );

    let observed = wait_for_transcript(&transcript, "the resumed session", |value| {
        value.get("sessionLoad").is_some()
    });
    assert_eq!(observed["sessionLoad"]["sessionId"], json!("prior-session"));
    assert_eq!(
        observed["sessionLoad"]["cwd"].as_str(),
        Some(workspace.path().to_string_lossy().as_ref()),
        "a resumed session should reuse the configured working directory"
    );

    let observed = wait_for_transcript(&transcript, "the retired session", |value| {
        value.get("closes").is_some()
    });
    assert_eq!(
        observed["closes"],
        json!([{ "sessionId": "fake-session-1" }]),
        "the session created during connect should be closed once it is replaced"
    );

    assert_eq!(
        prompt(provider.as_ref(), "prompt after resume")
            .await
            .expect("the resumed session should accept subsequent prompts"),
        REPLY
    );
    let observed = wait_for_transcript(&transcript, "the resumed prompt", |value| {
        value
            .get("prompts")
            .and_then(Value::as_array)
            .is_some_and(|prompts| {
                prompts.iter().any(|prompt| {
                    prompt["sessionId"] == json!("prior-session")
                        && prompt["text"] == json!("prompt after resume")
                })
            })
    });
    assert!(
        observed["prompts"].as_array().is_some_and(|prompts| prompts
            .iter()
            .any(|prompt| prompt["sessionId"] == json!("prior-session"))),
        "subsequent prompts must use the loaded ACP session"
    );
}

#[tokio::test]
async fn generic_acp_mode_mapping_applies_the_agent_permission_mode() {
    let Some(python) = python_or_skip() else {
        return;
    };

    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let _env = env_lock::lock_env([
        ("GOOSE_PATH_ROOT", Some(root_path.as_str())),
        ("GOOSE_DISABLE_KEYRING", Some("1")),
    ]);

    let workspace = tempfile::tempdir().unwrap();
    let transcript_dir = tempfile::tempdir().unwrap();
    let transcript = transcript_dir.path().join("transcript.json");

    let script = agent_script();
    let config = acp_config(
        "custom_acp_modes",
        &python,
        json!({
            "args": [script.to_string_lossy()],
            "env": [
                ["FAKE_ACP_RECORD", transcript.display().to_string()],
                ["FAKE_ACP_MODES", "agent-default,agent-auto"],
            ],
            "work_dir": workspace.path().to_string_lossy(),
            "mode_mapping": { "auto": ["agent-auto"] },
        }),
    );

    let provider = registry_with(&config)
        .create(&config.name, vec![])
        .await
        .expect("generic ACP provider should connect");

    let observed = wait_for_transcript(&transcript, "its mode selection", |value| {
        value.get("modes").is_some()
    });
    assert_eq!(
        observed["modes"],
        json!([{ "sessionId": "fake-session-1", "modeId": "agent-auto" }]),
        "goose's current permission mode should be translated through the mode mapping"
    );
    drop(provider);
}

/// Unknown modes must fail loudly: silently leaving the agent in its own default
/// mode would run tool calls under a permission level the user did not choose.
#[tokio::test]
async fn generic_acp_unsupported_mode_mapping_fails_clearly() {
    let Some(python) = python_or_skip() else {
        return;
    };

    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let _env = env_lock::lock_env([
        ("GOOSE_PATH_ROOT", Some(root_path.as_str())),
        ("GOOSE_DISABLE_KEYRING", Some("1")),
    ]);

    let workspace = tempfile::tempdir().unwrap();
    let transcript_dir = tempfile::tempdir().unwrap();
    let transcript = transcript_dir.path().join("transcript.json");

    let script = agent_script();
    let config = acp_config(
        "custom_acp_bad_modes",
        &python,
        json!({
            "args": [script.to_string_lossy()],
            "env": [
                ["FAKE_ACP_RECORD", transcript.display().to_string()],
                ["FAKE_ACP_MODES", "agent-default"],
            ],
            "work_dir": workspace.path().to_string_lossy(),
            "mode_mapping": { "auto": ["agent-missing"] },
        }),
    );

    let message = connect_error(&registry_with(&config), &config.name).await;
    assert!(
        message.contains("agent-missing") && message.contains("agent-default"),
        "an unsupported mode mapping should report both the requested and available modes: {message}"
    );
    assert!(
        !message.contains("could not resolve") && !message.contains("failed to spawn"),
        "an unsupported mode is a negotiation failure, not a launch failure: {message}"
    );
}

/// The distinct stages a generic ACP provider can fail at: resolving the
/// configured command, launching it, completing the ACP handshake, and the
/// agent rejecting the turn for authentication.
#[tokio::test]
async fn generic_acp_failures_identify_the_stage_that_failed() {
    let Some(python) = python_or_skip() else {
        return;
    };

    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let _env = env_lock::lock_env([
        ("GOOSE_PATH_ROOT", Some(root_path.as_str())),
        ("GOOSE_DISABLE_KEYRING", Some("1")),
    ]);

    let workspace = tempfile::tempdir().unwrap();
    let note = workspace.path().join("not-executable");
    std::fs::write(&note, "#!/bin/sh\necho should never run\n").unwrap();

    let unresolvable = acp_config(
        "custom_acp_unresolvable",
        "goose-test-missing-acp-agent",
        json!({ "work_dir": workspace.path().to_string_lossy() }),
    );
    let discovery = connect_error(&registry_with(&unresolvable), &unresolvable.name).await;
    assert!(
        discovery.contains("could not resolve command 'goose-test-missing-acp-agent'"),
        "discovery failures should name the command that could not be found: {discovery}"
    );
    assert!(
        !discovery.contains("spawn") && !discovery.contains("initialize"),
        "a command that was never found should not be reported as a launch or handshake failure: {discovery}"
    );

    let unlaunchable = acp_config(
        "custom_acp_unlaunchable",
        note.to_string_lossy().as_ref(),
        json!({ "work_dir": workspace.path().to_string_lossy() }),
    );
    let launch = connect_error(&registry_with(&unlaunchable), &unlaunchable.name).await;
    assert!(
        launch.contains("failed to spawn ACP process"),
        "launch failures should report that the process could not be started: {launch}"
    );
    assert!(
        launch.contains("os error") || launch.contains(&note.to_string_lossy().into_owned()),
        "launch failures should keep the operating system cause: {launch}"
    );

    let transcript_dir = tempfile::tempdir().unwrap();
    let handshake_transcript = transcript_dir.path().join("handshake.json");
    let script = agent_script();
    let exiting_handshake = acp_config(
        "custom_acp_handshake_exit",
        &python,
        json!({
            "args": [script.to_string_lossy()],
            "env": [
                ["FAKE_ACP_RECORD", handshake_transcript.display().to_string()],
                ["FAKE_ACP_MODE", "exit_before_handshake"],
            ],
            "work_dir": workspace.path().to_string_lossy(),
        }),
    );
    let exit_error =
        connect_error(&registry_with(&exiting_handshake), &exiting_handshake.name).await;
    assert!(
        exit_error.contains("ACP initialize failed"),
        "an agent that exits before initialize should report the initialization stage: {exit_error}"
    );
    assert!(
        !exit_error.contains("spawn") && !exit_error.contains("could not resolve"),
        "an agent that launched and then died should not be reported as a launch failure: {exit_error}"
    );
    wait_for_transcript(&handshake_transcript, "its launch", |value| {
        value.get("argv").is_some()
    });

    let json_error_transcript = transcript_dir.path().join("handshake-error.json");
    let rejected_handshake = acp_config(
        "custom_acp_handshake_error",
        &python,
        json!({
            "args": [script.to_string_lossy()],
            "env": [
                ["FAKE_ACP_RECORD", json_error_transcript.display().to_string()],
                ["FAKE_ACP_MODE", "fail_handshake"],
            ],
            "work_dir": workspace.path().to_string_lossy(),
        }),
    );
    let json_error = connect_error(
        &registry_with(&rejected_handshake),
        &rejected_handshake.name,
    )
    .await;
    assert!(
        json_error.contains("ACP initialize failed") && json_error.contains("handshake refused"),
        "JSON-RPC handshake errors should preserve the initialization stage and agent cause: {json_error}"
    );
    assert!(
        !json_error.contains("spawn") && !json_error.contains("could not resolve"),
        "an agent that launched and rejected initialize should not be reported as a launch failure: {json_error}"
    );
    wait_for_transcript(&json_error_transcript, "its initialize request", |value| {
        value.get("initialize").is_some()
    });

    let auth_transcript = transcript_dir.path().join("auth.json");
    let requiring_auth = acp_config(
        "custom_acp_auth",
        &python,
        json!({
            "args": [script.to_string_lossy()],
            "env": [
                ["FAKE_ACP_RECORD", auth_transcript.display().to_string()],
                ["FAKE_ACP_MODE", "require_auth"],
            ],
            "work_dir": workspace.path().to_string_lossy(),
        }),
    );
    let provider = registry_with(&requiring_auth)
        .create(&requiring_auth.name, vec![])
        .await
        .expect("the agent should still complete the handshake");
    let error = prompt(provider.as_ref(), "hello")
        .await
        .expect_err("an agent that requires auth should fail the turn");
    assert!(
        matches!(error, ProviderError::Authentication(_)),
        "the agent's auth rejection should surface as an authentication error, got {error:?}"
    );
    let observed = wait_for_transcript(&auth_transcript, "the rejected prompt", |value| {
        value.get("prompts").is_some()
    });
    assert_eq!(
        observed["prompts"][0]["text"],
        json!("hello"),
        "the prompt should have reached the agent before it rejected it"
    );
}

/// Abandoning a turn must tell the agent to stop. Otherwise a cancelled goose
/// turn leaves the agent working to completion with nobody reading its output.
#[tokio::test]
async fn abandoned_generic_acp_turn_cancels_the_agent() {
    let Some(python) = python_or_skip() else {
        return;
    };

    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let _env = env_lock::lock_env([
        ("GOOSE_PATH_ROOT", Some(root_path.as_str())),
        ("GOOSE_DISABLE_KEYRING", Some("1")),
    ]);

    let workspace = tempfile::tempdir().unwrap();
    let transcript_dir = tempfile::tempdir().unwrap();
    let transcript = transcript_dir.path().join("transcript.json");

    let script = agent_script();
    let config = acp_config(
        "custom_acp_cancel",
        &python,
        json!({
            "args": [script.to_string_lossy()],
            "env": [
                ["FAKE_ACP_RECORD", transcript.display().to_string()],
                ["FAKE_ACP_REPLY", REPLY],
                ["FAKE_ACP_MODE", "slow"],
            ],
            "work_dir": workspace.path().to_string_lossy(),
        }),
    );

    let provider = registry_with(&config)
        .create(&config.name, vec![])
        .await
        .expect("generic ACP provider should connect");

    let model = ModelConfig::new("fake-model-v1");
    let messages = vec![Message::user().with_text("long turn".to_string())];
    let mut stream = goose::session_context::with_session_id(
        Some("generic-acp-cancel-session".to_string()),
        provider.stream(&model, "", &messages, &[]),
    )
    .await
    .expect("the turn should start");

    // The agent streams a chunk and then holds the turn open, so the next update
    // never arrives and dropping the stream is the only way the turn ends.
    let first = stream
        .next()
        .await
        .expect("the agent should stream something")
        .expect("the first update should not fail");
    assert!(first.0.is_some(), "the agent's chunk should reach goose");
    drop(stream);

    let observed = wait_for_transcript(&transcript, "the cancellation", |value| {
        value.get("cancels").is_some()
    });
    assert_eq!(
        observed["cancels"],
        json!([{ "sessionId": "fake-session-1" }]),
        "abandoning the turn should send session/cancel for the active session"
    );
}

/// Dropping a provider must tear the agent process down rather than leaking it.
#[tokio::test]
async fn dropping_generic_acp_provider_terminates_agent_process() {
    let Some(python) = python_or_skip() else {
        return;
    };

    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let _env = env_lock::lock_env([
        ("GOOSE_PATH_ROOT", Some(root_path.as_str())),
        ("GOOSE_DISABLE_KEYRING", Some("1")),
    ]);

    let workspace = tempfile::tempdir().unwrap();
    let transcript_dir = tempfile::tempdir().unwrap();
    let transcript = transcript_dir.path().join("transcript.json");
    let heartbeat = transcript_dir.path().join("heartbeat");

    let script = agent_script();
    let config = acp_config(
        "custom_acp_teardown",
        &python,
        json!({
            "args": [script.to_string_lossy()],
            "env": [
                ["FAKE_ACP_RECORD", transcript.display().to_string()],
                ["FAKE_ACP_MODE", "slow"],
                ["FAKE_ACP_HEARTBEAT", heartbeat.display().to_string()],
            ],
            "work_dir": workspace.path().to_string_lossy(),
        }),
    );

    let provider = registry_with(&config)
        .create(&config.name, vec![])
        .await
        .expect("generic ACP provider should connect");

    let first = wait_for_transcript(&transcript, "its session", |value| {
        value.get("sessionNew").is_some()
    });
    assert!(first.get("stdinClosed").is_none());

    let before = read_ticks(&heartbeat).expect("the agent should be reporting a heartbeat");
    std::thread::sleep(Duration::from_millis(200));
    let alive = read_ticks(&heartbeat).expect("the agent should still be reporting a heartbeat");
    assert!(
        alive > before,
        "the agent process should be running before the provider is dropped"
    );

    drop(provider);

    let deadline = Instant::now() + WAIT_LIMIT;
    let mut previous = read_ticks(&heartbeat);
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let current = read_ticks(&heartbeat);
        if previous.is_some() && previous == current {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the agent process kept running after the provider was dropped"
        );
        previous = current;
    }
}
