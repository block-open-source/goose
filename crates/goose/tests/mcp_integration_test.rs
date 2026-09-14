use serde::Deserialize;

use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::{env, fs};

use rmcp::model::{CallToolRequestParams, CallToolResult, Tool};
use rmcp::object;
use tokio_util::sync::CancellationToken;

use goose::agents::extension::{Envs, ExtensionConfig};
use goose::agents::extension_manager::{ExtensionManager, ExtensionManagerCapabilities};
use goose::agents::{GoosePlatform, MCP_PROTOCOL_VERSION};
use goose_providers::model::ModelConfig;

use test_case::test_case;

use async_trait::async_trait;
use goose::conversation::message::Message;
use goose::providers::base::{
    stream_from_single_message, MessageStream, Provider, ProviderDef, ProviderMetadata,
};
use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
use goose_providers::errors::ProviderError;
use once_cell::sync::Lazy;
use std::process::Command;

#[derive(Deserialize)]
struct CargoBuildMessage {
    reason: String,
    target: Target,
    executable: String,
}

#[derive(Deserialize)]
struct Target {
    name: String,
    kind: Vec<String>,
}

#[derive(Clone, Default)]
pub struct MockProvider;

impl MockProvider {
    pub fn new() -> Self {
        Self
    }
}

impl goose::providers::base::ProviderDescriptor for MockProvider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata::empty()
    }
}

impl ProviderDef for MockProvider {
    type Provider = Self;

    fn from_env(
        _extensions: Vec<goose::config::ExtensionConfig>,
        _tls_config: Option<goose::providers::api_client::TlsConfig>,
    ) -> futures::future::BoxFuture<'static, anyhow::Result<Self>> {
        Box::pin(async move { Ok(Self::new()) })
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn get_name(&self) -> &str {
        "mock"
    }

    async fn stream(
        &self,
        _model_config: &ModelConfig,
        _system: &str,
        _messages: &[Message],
        _tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let message = Message::assistant().with_text("\"So we beat on, boats against the current, borne back ceaselessly into the past.\" — F. Scott Fitzgerald, The Great Gatsby (1925)");
        let usage = ProviderUsage::new("mock".to_string(), Usage::default());
        Ok(stream_from_single_message(message, usage))
    }
}

fn build_and_get_binary_path() -> PathBuf {
    let output = Command::new("cargo")
        .args([
            "build",
            "--frozen",
            "-p",
            "goose-test",
            "--bin",
            "capture",
            "--message-format=json",
        ])
        .output()
        .expect("failed to build binary");

    if !output.status.success() {
        panic!("build failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(serde_json::from_str::<CargoBuildMessage>)
        .filter_map(Result::ok)
        .filter(|message| message.reason == "compiler-artifact")
        .filter_map(|message| {
            if message.target.name == "capture"
                && message.target.kind.contains(&String::from("bin"))
            {
                Some(PathBuf::from(message.executable))
            } else {
                None
            }
        })
        .next()
        .expect("failed to parse binary path")
}

static REPLAY_BINARY_PATH: Lazy<PathBuf> = Lazy::new(build_and_get_binary_path);

enum TestMode {
    Record,
    Playback,
}

#[test_case(
    vec!["npx", "-y", "@modelcontextprotocol/server-everything@2026.1.14"],
    vec![
        CallToolRequestParams::new("echo").with_arguments(object!({"message": "Hello, world!" })),
        CallToolRequestParams::new("get-sum").with_arguments(object!({"a": 1, "b": 2 })),
        CallToolRequestParams::new("trigger-long-running-operation").with_arguments(object!({"duration": 1, "steps": 5 })),
        CallToolRequestParams::new("get-structured-content").with_arguments(object!({"location": "New York"})),
        CallToolRequestParams::new("trigger-sampling-request").with_arguments(object!({"prompt": "Please provide a quote from The Great Gatsby", "maxTokens": 100 }))
    ],
    vec![]
)]
#[test_case(
    vec!["github-mcp-server", "stdio"],
    vec![
        CallToolRequestParams::new("get_file_contents").with_arguments(object!({
            "owner": "block",
            "repo": "goose",
            "path": "README.md",
            "sha": "ab62b863c1666232a67048b6c4e10007a2a5b83c"
        })),
    ],
    vec!["GITHUB_PERSONAL_ACCESS_TOKEN"]
)]
#[test_case(
    vec!["uv", "run", "--with", "fastmcp==2.14.4", "fastmcp", "run", "tests/fastmcp_test_server.py"],
    vec![
        CallToolRequestParams::new("divide").with_arguments(object!({
            "dividend": 10,
            "divisor": 2
        }))
    ],
    vec![]
)]
#[tokio::test]
async fn test_replayed_session(
    command: Vec<&str>,
    tool_calls: Vec<CallToolRequestParams>,
    required_envs: Vec<&str>,
) {
    // The working directory is sent to the server verbatim in our `roots/list`
    // response, so it is part of the recorded protocol traffic. It must be an
    // absolute path (relative paths are not convertible to a `file://` URL) and
    // it must be stable across machines, otherwise playback compares a recorded
    // path against whatever cwd the test happens to run in.
    const TEST_WORKING_DIR: &str = "/tmp/goose_test";
    fs::create_dir_all(TEST_WORKING_DIR).ok();

    let _env = env_lock::lock_env([
        ("GOOSE_MCP_CLIENT_VERSION", Some("0.0.0")),
        ("GOOSE_PROVIDER", Some("openai")),
        ("GOOSE_MODEL", Some("gpt-4o")),
        ("GOOSE_WORKING_DIR", Some(TEST_WORKING_DIR)),
    ]);

    // Setup test file for developer extension tests
    let test_file_path = "/tmp/goose_test/goose.txt";
    fs::write(test_file_path, "# goose\n").ok();
    let replay_file_name = command
        .iter()
        .map(|s| s.replace("/", "_"))
        .collect::<Vec<String>>()
        .join("");
    let mut replay_file_path =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("should find the project root"));
    replay_file_path.push("tests");
    replay_file_path.push("mcp_replays");
    replay_file_path.push(&replay_file_name);

    let mode = if env::var("GOOSE_RECORD_MCP").is_ok() {
        TestMode::Record
    } else {
        assert!(replay_file_path.exists(), "replay file doesn't exist");
        TestMode::Playback
    };

    let mode_arg = match mode {
        TestMode::Record => "record",
        TestMode::Playback => "playback",
    };
    let cmd = REPLAY_BINARY_PATH.to_string_lossy().to_string();
    let mut args = vec!["stdio", mode_arg]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<String>>();

    args.push(replay_file_path.to_string_lossy().to_string());

    let mut env = HashMap::new();

    if matches!(mode, TestMode::Record) {
        args.extend(command.into_iter().map(str::to_string));

        for key in required_envs {
            match env::var(key) {
                Ok(v) => {
                    env.insert(key.to_string(), v);
                }
                Err(_) => {
                    eprintln!("skipping due to missing required env variable: {}", key);
                    return;
                }
            }
        }
    }

    let envs = Envs::new(env);
    let extension_config = ExtensionConfig::Stdio {
        name: "test".to_string(),
        description: "Test".to_string(),
        cmd,
        args,
        envs,
        env_keys: vec![],
        timeout: Some(30),
        cwd: None,
        bundled: Some(false),
        available_tools: vec![],
    };

    let provider = Arc::new(tokio::sync::Mutex::new(Some(
        Arc::new(MockProvider::new()) as Arc<dyn Provider>
    )));
    let temp_dir = tempfile::tempdir().unwrap();
    let session_manager = Arc::new(goose::session::SessionManager::new(
        temp_dir.path().to_path_buf(),
    ));
    let extension_manager = Arc::new(ExtensionManager::new(
        provider,
        session_manager,
        None,
        GoosePlatform::GooseDesktop.to_string(),
        ExtensionManagerCapabilities {
            mcpui: true,
            host_info: None,
            elicitation_handler: None,
            protocol_version: Some(MCP_PROTOCOL_VERSION),
        },
        true,
    ));

    #[allow(clippy::redundant_closure_call)]
    let result = (async || -> Result<(), Box<dyn std::error::Error>> {
        extension_manager
            .add_extension(extension_config, None, None, None)
            .await?;
        let mut results = Vec::new();
        for tool_call in tool_calls {
            let mut new_call = CallToolRequestParams::new(format!("test__{}", tool_call.name));
            if let Some(args) = tool_call.arguments {
                new_call = new_call.with_arguments(args);
            }
            let tool_call = new_call;
            let ctx = goose::agents::ToolCallContext::new(
                "test-session-id".to_string(),
                None,
                Some("test-id".to_string()),
            );
            let result = extension_manager
                .dispatch_tool_call(&ctx, tool_call, CancellationToken::default())
                .await;

            let tool_result = result?;
            results.push(tool_result.result.await?);
        }

        let mut results_path = replay_file_path.clone();
        results_path.pop();
        results_path.push(format!("{}.results.json", &replay_file_name));

        match mode {
            TestMode::Record => {
                serde_json::to_writer_pretty(File::create(results_path)?, &results)?
            }
            TestMode::Playback => assert_eq!(
                serde_json::from_reader::<_, Vec<CallToolResult>>(File::open(results_path)?)?,
                results
            ),
        };

        Ok(())
    })()
    .await;

    if let Err(err) = result {
        if matches!(mode, TestMode::Playback) {
            let errors =
                fs::read_to_string(format!("{}.errors.txt", replay_file_path.to_string_lossy()))
                    .expect("could not read errors");
            eprintln!("errors from {}", replay_file_path.to_string_lossy());
            eprintln!("{}", errors);
            eprintln!();
        }
        panic!("Test failed: {:?}", err);
    }
}
