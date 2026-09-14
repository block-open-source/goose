use std::{
    borrow::Cow,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

use anyhow::Result;
use async_trait::async_trait;
use goose_agent::{
    machine::{MachineSession, StateMachine, Step},
    operation::{
        ConversationEffect, Emitter, Inference, InferenceInput, Operation, OperationResult,
    },
    tool::{ToolOperation, ToolProvider},
};
use goose_provider_types::conversation::{
    message::{Message, MessageContent},
    Conversation,
};
use rmcp::{
    handler::server::router::tool::{AsyncTool, SyncTool, ToolBase},
    model::{CallToolRequestParams, CallToolResult, ErrorData, Tool},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Default, Deserialize, JsonSchema)]
struct AddInput {
    left: u64,
    right: u64,
}

#[derive(Serialize, JsonSchema)]
struct AddOutput {
    sum: u64,
}

struct Add;

impl ToolBase for Add {
    type Parameter = AddInput;
    type Output = AddOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "add".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some("Add two integers".into())
    }
}

impl SyncTool<()> for Add {
    fn invoke(_session: &(), input: AddInput) -> Result<AddOutput, ErrorData> {
        Ok(AddOutput {
            sum: input.left + input.right,
        })
    }
}

#[derive(Default, Deserialize, JsonSchema)]
struct GreetInput {
    name: String,
}

#[derive(Serialize, JsonSchema)]
struct GreetOutput {
    greeting: String,
}

struct Greet;

impl ToolBase for Greet {
    type Parameter = GreetInput;
    type Output = GreetOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "greet".into()
    }
}

impl AsyncTool<()> for Greet {
    async fn invoke(_session: &(), input: GreetInput) -> Result<GreetOutput, ErrorData> {
        Ok(GreetOutput {
            greeting: format!("Hello, {}!", input.name),
        })
    }
}

fn emitter() -> Emitter {
    emitter_with_token(CancellationToken::new())
}

fn emitter_with_token(cancel: CancellationToken) -> Emitter {
    let (tx, _rx) = mpsc::channel(1);
    Emitter::new(tx, cancel)
}

fn operation() -> ToolOperation<()> {
    ToolOperation::new()
        .with_sync_tool::<Add>()
        .with_async_tool::<Greet>()
}

fn appended_message(result: OperationResult<ConversationEffect>) -> Message {
    let OperationResult::Applied(result) = result else {
        panic!("operation should apply");
    };
    let ConversationEffect::AppendMessage(message) = result.effects.into_iter().next().unwrap()
    else {
        panic!("operation should append a message");
    };
    message
}

#[tokio::test]
async fn advertises_user_defined_tools_directly() {
    let tools = <ToolOperation<()> as Operation<(), ConversationEffect>>::inference_tools(
        &operation(),
        &(),
    )
    .await
    .unwrap();

    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].name, "add");
    assert_eq!(tools[0].description.as_deref(), Some("Add two integers"));
    assert!(tools[0].input_schema["properties"]["left"].is_object());
    assert_eq!(tools[1].name, "greet");
}

#[tokio::test]
async fn dispatches_calls_to_user_defined_tools() {
    let conversation = Conversation::new_unvalidated([
        Message::user().with_text("do both"),
        Message::assistant()
            .with_tool_request(
                "call-1",
                Ok(CallToolRequestParams::new("add").with_arguments(
                    serde_json::from_value(json!({"left": 2, "right": 3})).unwrap(),
                )),
            )
            .with_tool_request(
                "call-2",
                Ok(CallToolRequestParams::new("greet")
                    .with_arguments(serde_json::from_value(json!({"name": "Goose"})).unwrap())),
            ),
    ]);

    let message = appended_message(
        <ToolOperation<()> as Operation<(), ConversationEffect>>::run(
            &operation(),
            &(),
            &conversation,
            &emitter(),
        )
        .await
        .unwrap(),
    );
    let responses = message
        .content
        .iter()
        .map(|content| match content {
            MessageContent::ToolResponse(response) => response,
            _ => panic!("expected tool responses"),
        })
        .collect::<Vec<_>>();
    assert_eq!(responses[0].id, "call-1");
    assert_eq!(
        responses[0]
            .tool_result
            .as_ref()
            .unwrap()
            .structured_content,
        Some(json!({"sum": 5}))
    );
    assert_eq!(responses[1].id, "call-2");
    assert_eq!(
        responses[1]
            .tool_result
            .as_ref()
            .unwrap()
            .structured_content,
        Some(json!({"greeting": "Hello, Goose!"}))
    );
}

struct DynamicTools {
    discoveries: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolProvider<bool> for DynamicTools {
    async fn tools(&self, enabled: &bool) -> Result<Vec<Tool>> {
        self.discoveries.fetch_add(1, Ordering::SeqCst);
        Ok(if *enabled {
            vec![Tool::new(
                "dynamic",
                "A session-dependent tool",
                Arc::new(serde_json::from_value(json!({"type": "object"}))?),
            )]
        } else {
            Vec::new()
        })
    }

    async fn call(
        &self,
        _session: &bool,
        request_id: &str,
        call: CallToolRequestParams,
        _emit: &Emitter,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(CallToolResult::structured(json!({
            "request_id": request_id,
            "name": call.name
        })))
    }
}

#[tokio::test]
async fn rediscovers_dynamic_tools_for_inference_and_execution() {
    let discoveries = Arc::new(AtomicUsize::new(0));
    let operation = ToolOperation::new().with_provider(Arc::new(DynamicTools {
        discoveries: discoveries.clone(),
    }));
    let tools = <ToolOperation<bool> as Operation<bool, ConversationEffect>>::inference_tools(
        &operation, &true,
    )
    .await
    .unwrap();
    assert_eq!(tools[0].name, "dynamic");

    let conversation = Conversation::new_unvalidated([
        Message::user().with_text("call the dynamic tool"),
        Message::assistant()
            .with_tool_request("dynamic-call", Ok(CallToolRequestParams::new("dynamic"))),
    ]);
    let message = appended_message(
        <ToolOperation<bool> as Operation<bool, ConversationEffect>>::run(
            &operation,
            &true,
            &conversation,
            &emitter(),
        )
        .await
        .unwrap(),
    );

    assert_eq!(discoveries.load(Ordering::SeqCst), 2);
    assert_eq!(
        message.content[0]
            .as_tool_response()
            .unwrap()
            .tool_result
            .as_ref()
            .unwrap()
            .structured_content,
        Some(json!({"request_id": "dynamic-call", "name": "dynamic"}))
    );
}

#[tokio::test]
async fn dynamic_tools_vary_by_session() {
    let operation = ToolOperation::new().with_provider(Arc::new(DynamicTools {
        discoveries: Arc::new(AtomicUsize::new(0)),
    }));
    let enabled = <ToolOperation<bool> as Operation<bool, ConversationEffect>>::inference_tools(
        &operation, &true,
    )
    .await
    .unwrap();
    let disabled = <ToolOperation<bool> as Operation<bool, ConversationEffect>>::inference_tools(
        &operation, &false,
    )
    .await
    .unwrap();

    assert_eq!(enabled[0].name, "dynamic");
    assert!(disabled.is_empty());
}

#[tokio::test]
async fn rejects_duplicate_dynamic_tool_names_at_both_boundaries() {
    let operation = ToolOperation::new()
        .with_provider(Arc::new(DynamicTools {
            discoveries: Arc::new(AtomicUsize::new(0)),
        }))
        .with_provider(Arc::new(DynamicTools {
            discoveries: Arc::new(AtomicUsize::new(0)),
        }));
    let kickoff = Message::user().with_text("use tools");

    let inference_error =
        <ToolOperation<bool> as Operation<bool, ConversationEffect>>::inference_tools(
            &operation, &true,
        )
        .await
        .unwrap_err();
    assert_eq!(
        inference_error.to_string(),
        "multiple tool providers registered 'dynamic'"
    );

    let conversation = Conversation::new_unvalidated([
        kickoff,
        Message::assistant()
            .with_tool_request("dynamic-call", Ok(CallToolRequestParams::new("dynamic"))),
    ]);
    let execution_error = match <ToolOperation<bool> as Operation<bool, ConversationEffect>>::run(
        &operation,
        &true,
        &conversation,
        &emitter(),
    )
    .await
    {
        Ok(_) => panic!("duplicate tools should fail during execution discovery"),
        Err(error) => error,
    };
    assert_eq!(
        execution_error.to_string(),
        "multiple tool providers registered 'dynamic'"
    );
}

#[tokio::test]
async fn ignores_requests_outside_the_current_turn() {
    let conversation = Conversation::new_unvalidated([
        Message::user().with_text("old turn"),
        Message::assistant().with_tool_request(
            "stale-call",
            Ok(CallToolRequestParams::new("add")
                .with_arguments(serde_json::from_value(json!({"left": 2, "right": 3})).unwrap())),
        ),
        Message::user().with_text("new turn"),
    ]);

    let result = <ToolOperation<()> as Operation<(), ConversationEffect>>::run(
        &operation(),
        &(),
        &conversation,
        &emitter(),
    )
    .await
    .unwrap();

    assert!(matches!(result, OperationResult::NotApplicable));
}

#[tokio::test]
async fn responds_to_unparseable_tool_requests() {
    let conversation = Conversation::new_unvalidated([
        Message::user().with_text("call a tool"),
        Message::assistant().with_tool_request(
            "invalid-call",
            Err(ErrorData::invalid_params("malformed arguments", None)),
        ),
    ]);

    let message = appended_message(
        <ToolOperation<()> as Operation<(), ConversationEffect>>::run(
            &operation(),
            &(),
            &conversation,
            &emitter(),
        )
        .await
        .unwrap(),
    );
    let response = message.content[0].as_tool_response().unwrap();
    assert_eq!(response.id, "invalid-call");
    assert_eq!(
        response.tool_result.as_ref().unwrap_err().message,
        "malformed arguments"
    );
}

#[derive(Default, Clone)]
struct BlockingSession {
    started: Arc<AtomicBool>,
}

struct BlockingSyncTool;

impl ToolBase for BlockingSyncTool {
    type Parameter = ();
    type Output = ();
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "blocking_sync".into()
    }

    fn input_schema() -> Option<Arc<serde_json::Map<String, serde_json::Value>>> {
        None
    }
}

impl SyncTool<BlockingSession> for BlockingSyncTool {
    fn invoke(session: &BlockingSession, _input: ()) -> Result<(), ErrorData> {
        session.started.store(true, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(100));
        Ok(())
    }
}

#[tokio::test]
async fn cancellation_interrupts_blocking_sync_tools() {
    let session = BlockingSession::default();
    let started = session.started.clone();
    let operation = ToolOperation::new().with_sync_tool::<BlockingSyncTool>();
    let conversation = Conversation::new_unvalidated([
        Message::user().with_text("call it"),
        Message::assistant()
            .with_tool_request("call-1", Ok(CallToolRequestParams::new("blocking_sync"))),
    ]);
    let cancel = CancellationToken::new();
    let run_cancel = cancel.clone();
    let run = tokio::spawn(async move {
        <ToolOperation<BlockingSession> as Operation<BlockingSession, ConversationEffect>>::run(
            &operation,
            &session,
            &conversation,
            &emitter_with_token(run_cancel),
        )
        .await
    });
    while !started.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    cancel.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_millis(50), run)
        .await
        .expect("operation should not wait for the blocking tool")
        .unwrap()
        .unwrap();

    let message = appended_message(result);
    assert!(message.content[0]
        .as_tool_response()
        .unwrap()
        .tool_result
        .as_ref()
        .unwrap()
        .is_error
        .is_some_and(|is_error| is_error));
}

struct BlockingDiscovery {
    started: Arc<AtomicBool>,
}

#[async_trait]
impl<S: Send + Sync> ToolProvider<S> for BlockingDiscovery {
    async fn tools(&self, _session: &S) -> Result<Vec<Tool>> {
        self.started.store(true, Ordering::SeqCst);
        std::future::pending().await
    }

    async fn call(
        &self,
        _session: &S,
        _request_id: &str,
        _call: CallToolRequestParams,
        _emit: &Emitter,
    ) -> Result<CallToolResult, ErrorData> {
        unreachable!()
    }
}

struct TestSession {
    conversation: Conversation,
}

impl MachineSession for TestSession {
    fn id(&self) -> &str {
        "test"
    }

    fn conversation(&self) -> Option<&Conversation> {
        Some(&self.conversation)
    }
}

struct TestInference;

#[async_trait]
impl Operation<TestSession> for TestInference {
    fn name(&self) -> &'static str {
        "inference"
    }
}

#[async_trait]
impl Inference<TestSession> for TestInference {
    fn applies(&self, _conversation: &Conversation) -> bool {
        true
    }

    async fn infer(
        &self,
        _session: &TestSession,
        _conversation: &Conversation,
        _input: InferenceInput,
        _emit: &Emitter,
    ) -> Result<OperationResult<ConversationEffect>> {
        unreachable!()
    }
}

#[tokio::test]
async fn cancellation_interrupts_inference_discovery() {
    let started = Arc::new(AtomicBool::new(false));
    let operation = ToolOperation::new().with_provider(Arc::new(BlockingDiscovery {
        started: started.clone(),
    }));
    let cancel = CancellationToken::new();
    let machine = StateMachine::new(
        vec![
            Step::Operation(Arc::new(operation)),
            Step::Inference(Arc::new(TestInference)),
        ],
        cancel.clone(),
    );
    let session = TestSession {
        conversation: Conversation::new_unvalidated([Message::user().with_text("use tools")]),
    };
    let emit = emitter_with_token(cancel.clone());

    let step = tokio::spawn(async move { machine.step(&session, &emit).await });
    while !started.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    cancel.cancel();

    assert!(step.await.unwrap().unwrap().is_none());
}

#[tokio::test]
async fn cancellation_interrupts_execution_discovery() {
    let started = Arc::new(AtomicBool::new(false));
    let operation = ToolOperation::new().with_provider(Arc::new(BlockingDiscovery {
        started: started.clone(),
    }));
    let conversation = Conversation::new_unvalidated([
        Message::user().with_text("call it"),
        Message::assistant()
            .with_tool_request("call-1", Ok(CallToolRequestParams::new("blocking"))),
    ]);
    let cancel = CancellationToken::new();
    let run_cancel = cancel.clone();
    let run = tokio::spawn(async move {
        <ToolOperation<()> as Operation<(), ConversationEffect>>::run(
            &operation,
            &(),
            &conversation,
            &emitter_with_token(run_cancel),
        )
        .await
    });
    while !started.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    cancel.cancel();
    let message = appended_message(run.await.unwrap().unwrap());

    let response = message.content[0].as_tool_response().unwrap();
    assert_eq!(response.id, "call-1");
    assert!(response
        .tool_result
        .as_ref()
        .unwrap()
        .is_error
        .is_some_and(|is_error| is_error));
}

struct BlockingTools {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolProvider<()> for BlockingTools {
    async fn tools(&self, _session: &()) -> Result<Vec<Tool>> {
        Ok(vec![Tool::new(
            "blocking",
            "A tool that never finishes",
            Arc::new(serde_json::from_value(json!({"type": "object"}))?),
        )])
    }

    async fn call(
        &self,
        _session: &(),
        _request_id: &str,
        _call: CallToolRequestParams,
        _emit: &Emitter,
    ) -> Result<CallToolResult, ErrorData> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }
}

#[tokio::test]
async fn cancellation_interrupts_current_and_remaining_calls() {
    let calls = Arc::new(AtomicUsize::new(0));
    let operation = ToolOperation::new().with_provider(Arc::new(BlockingTools {
        calls: calls.clone(),
    }));
    let conversation = Conversation::new_unvalidated([
        Message::user().with_text("call twice"),
        Message::assistant()
            .with_tool_request("call-1", Ok(CallToolRequestParams::new("blocking")))
            .with_tool_request("call-2", Ok(CallToolRequestParams::new("blocking"))),
    ]);
    let cancel = CancellationToken::new();
    let run_cancel = cancel.clone();
    let run = tokio::spawn(async move {
        <ToolOperation<()> as Operation<(), ConversationEffect>>::run(
            &operation,
            &(),
            &conversation,
            &emitter_with_token(run_cancel),
        )
        .await
    });
    while calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    cancel.cancel();
    let message = appended_message(run.await.unwrap().unwrap());

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        message.get_tool_response_ids(),
        ["call-1", "call-2"].into_iter().collect()
    );
    for content in &message.content {
        let result = content
            .as_tool_response()
            .unwrap()
            .tool_result
            .as_ref()
            .unwrap();
        assert!(result.is_error.is_some_and(|is_error| is_error));
        assert_eq!(
            result.content[0].as_text().unwrap().text,
            "Tool call was interrupted before completing"
        );
    }
}

#[tokio::test]
async fn ignores_unavailable_answered_and_externally_dispatched_requests() {
    let conversation = Conversation::new_unvalidated([
        Message::user().with_text("call tools"),
        Message::assistant()
            .with_tool_request("answered", Ok(CallToolRequestParams::new("add")))
            .with_tool_request(
                "unavailable",
                Ok(CallToolRequestParams::new("not_registered")),
            )
            .with_tool_request_with_metadata(
                "external",
                Ok(CallToolRequestParams::new("add")),
                None,
                Some(json!({"goose.external_dispatch": true})),
            ),
        Message::user().with_tool_response(
            "answered",
            Ok(CallToolResult::structured(json!({"sum": 5}))),
        ),
    ]);

    let result = <ToolOperation<()> as Operation<(), ConversationEffect>>::run(
        &operation(),
        &(),
        &conversation,
        &emitter(),
    )
    .await
    .unwrap();

    assert!(matches!(result, OperationResult::NotApplicable));
}
