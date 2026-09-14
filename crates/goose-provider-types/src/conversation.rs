use crate::conversation::message::{Message, MessageContentBlock, MessageMetadata};
use crate::mcp_utils::extract_text_from_resource;
use rmcp::model::{ContentBlock, Role};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;

pub mod message;
pub mod token_usage;
mod tool_request;
mod tool_result_serde;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Conversation(Vec<Message>);

#[derive(Error, Debug)]
#[error("invalid conversation: {reason}")]
pub struct InvalidConversation {
    reason: String,
    conversation: Conversation,
}

impl Conversation {
    pub fn new<I>(messages: I) -> Result<Self, InvalidConversation>
    where
        I: IntoIterator<Item = Message>,
    {
        Self::new_unvalidated(messages).validate()
    }

    pub fn new_unvalidated<I>(messages: I) -> Self
    where
        I: IntoIterator<Item = Message>,
    {
        Self(messages.into_iter().collect())
    }

    pub fn empty() -> Self {
        Self::new_unvalidated([])
    }

    pub fn messages(&self) -> &Vec<Message> {
        &self.0
    }

    pub fn messages_mut(&mut self) -> &mut Vec<Message> {
        &mut self.0
    }

    pub fn push(&mut self, message: Message) {
        let output_token_limit_reached = message.metadata.output_token_limit_reached;
        if message.content.is_empty()
            && (message.metadata.inference.is_some() || output_token_limit_reached)
        {
            if let Some(existing) = self.0.iter_mut().rev().find(|existing| {
                existing.role == message.role
                    && existing.is_user_visible()
                    && (!output_token_limit_reached
                        || (message.id.is_some()
                            && existing.id.as_deref() == message.id.as_deref()))
            }) {
                if let Some(inference) = message.metadata.inference.clone() {
                    existing.metadata.inference = Some(inference);
                }
                existing.metadata.output_token_limit_reached |= output_token_limit_reached;
                return;
            }

            if output_token_limit_reached {
                self.0.push(message.with_visibility(true, false));
            }
            return;
        }

        if let Some(last) = self
            .0
            .last_mut()
            .filter(|m| m.id.is_some() && m.id == message.id)
        {
            if message.metadata.inference.is_some() {
                last.metadata.inference = message.metadata.inference.clone();
            }
            last.metadata.output_token_limit_reached |= message.metadata.output_token_limit_reached;
            match (last.content.last_mut(), message.content.last()) {
                (
                    Some(MessageContentBlock::Text(ref mut last)),
                    Some(MessageContentBlock::Text(new)),
                ) if message.content.len() == 1
                    && last.annotations.as_ref().and_then(|a| a.audience.as_ref())
                        == new.annotations.as_ref().and_then(|a| a.audience.as_ref()) =>
                {
                    last.text.push_str(&new.text);
                }
                (
                    Some(MessageContentBlock::Thinking(ref mut last)),
                    Some(MessageContentBlock::Thinking(new)),
                ) if message.content.len() == 1
                    && (last.signature.is_empty() || new.signature == last.signature) =>
                {
                    // Merge cases:
                    //   - `last` is still unsigned (block in progress) — append
                    //     and adopt `new.signature` if it's the closing delta.
                    //   - signatures match — same block continuing.
                    // An unsigned delta arriving after a signed block belongs
                    // to the next block (signature-at-end streams emit the
                    // first text of block N+1 before its signature), so the
                    // outer match arm falls through to push it separately.
                    last.thinking.push_str(&new.thinking);
                    if !new.signature.is_empty() {
                        last.signature = new.signature.clone();
                    }
                }
                (_, _) => {
                    last.content.extend(message.content);
                }
            }
        } else {
            self.0.push(message);
        }
    }

    pub fn last(&self) -> Option<&Message> {
        self.0.last()
    }

    pub fn first(&self) -> Option<&Message> {
        self.0.first()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = Message>,
    {
        for message in iter {
            self.push(message);
        }
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Message> {
        self.0.iter()
    }

    pub fn pop(&mut self) -> Option<Message> {
        self.0.pop()
    }

    pub fn remove(&mut self, index: usize) -> Message {
        self.0.remove(index)
    }

    pub fn truncate(&mut self, len: usize) {
        self.0.truncate(len);
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn filtered_messages<F>(&self, filter: F) -> Vec<Message>
    where
        F: Fn(&MessageMetadata) -> bool,
    {
        self.0
            .iter()
            .filter(|msg| filter(&msg.metadata))
            .cloned()
            .collect()
    }

    pub fn agent_visible_messages(&self) -> Vec<Message> {
        self.0
            .iter()
            .filter(|message| message.metadata.agent_visible)
            .map(Message::agent_visible_content)
            .filter(|message| !message.content.is_empty())
            .collect()
    }

    pub fn user_visible_messages(&self) -> Vec<Message> {
        self.0
            .iter()
            .filter(|message| message.metadata.user_visible)
            .map(Message::user_visible_content)
            .filter(|message| !message.content.is_empty())
            .collect()
    }

    fn validate(self) -> Result<Self, InvalidConversation> {
        let (_messages, issues) = fix_messages(self.0.clone());
        if !issues.is_empty() {
            let reason = issues.join("\n");
            Err(InvalidConversation {
                reason,
                conversation: self,
            })
        } else {
            Ok(self)
        }
    }
}

impl Default for Conversation {
    fn default() -> Self {
        Self::empty()
    }
}

impl IntoIterator for Conversation {
    type Item = Message;
    type IntoIter = std::vec::IntoIter<Message>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
impl<'a> IntoIterator for &'a Conversation {
    type Item = &'a Message;
    type IntoIter = std::slice::Iter<'a, Message>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// Fix a conversation that we're about to send to an LLM. So the first and last
/// messages should always be from the user.
pub fn fix_conversation(conversation: Conversation) -> (Conversation, Vec<String>) {
    let all_messages = conversation.messages();

    // Create a shadow map: track each message as either Visible or NonVisible with its index
    enum MessageSlot {
        Visible(usize),      // Index into agent_visible_messages
        NonVisible(Message), // Non-visible messages pass through unchanged
    }

    let mut agent_visible_messages = Vec::new();
    let shadow_map: Vec<MessageSlot> = all_messages
        .iter()
        .map(|msg| {
            if msg.metadata.agent_visible {
                let idx = agent_visible_messages.len();
                agent_visible_messages.push(msg.clone());
                MessageSlot::Visible(idx)
            } else {
                MessageSlot::NonVisible(msg.clone())
            }
        })
        .collect();

    // Fix only the agent-visible messages
    let (fixed_visible, issues) = fix_messages(agent_visible_messages);

    // Reconstruct using shadow map: replace Visible slots with fixed messages
    let final_messages: Vec<Message> = shadow_map
        .into_iter()
        .filter_map(|slot| match slot {
            MessageSlot::Visible(idx) => fixed_visible.get(idx).cloned(),
            MessageSlot::NonVisible(msg) => Some(msg),
        })
        .collect();

    (Conversation::new_unvalidated(final_messages), issues)
}

fn fix_messages(messages: Vec<Message>) -> (Vec<Message>, Vec<String>) {
    [
        merge_text_content_items,
        trim_assistant_text_whitespace,
        remove_empty_messages,
        fix_empty_tool_results,
        fix_tool_calling,
        merge_consecutive_messages,
        dedupe_signed_thinking,
        fix_lead_trail,
        populate_if_empty,
    ]
    .into_iter()
    .fold(
        (messages, Vec::new()),
        |(msgs, mut all_issues), processor| {
            let (new_msgs, issues) = processor(msgs);
            all_issues.extend(issues);
            (new_msgs, all_issues)
        },
    )
}

fn merge_text_content_in_message(mut msg: Message) -> Message {
    if msg.role != Role::Assistant {
        return msg;
    }
    msg.content = msg
        .content
        .into_iter()
        .fold(Vec::new(), |mut content, item| {
            match item {
                MessageContentBlock::Text(text) => match content.last_mut() {
                    Some(MessageContentBlock::Text(last))
                        if last.annotations.as_ref().and_then(|a| a.audience.as_ref())
                            == text.annotations.as_ref().and_then(|a| a.audience.as_ref()) =>
                    {
                        last.text.push_str(&text.text);
                    }
                    _ => content.push(MessageContentBlock::Text(text)),
                },
                other => content.push(other),
            }
            content
        });
    msg
}

fn merge_text_content_items(messages: Vec<Message>) -> (Vec<Message>, Vec<String>) {
    messages.into_iter().fold(
        (Vec::new(), Vec::new()),
        |(mut messages, mut issues), message| {
            let content_len = message.content.len();
            let message = merge_text_content_in_message(message);
            if content_len != message.content.len() {
                issues.push(String::from("Merged text content"))
            }
            messages.push(message);
            (messages, issues)
        },
    )
}

fn trim_assistant_text_whitespace(messages: Vec<Message>) -> (Vec<Message>, Vec<String>) {
    let mut issues = Vec::new();

    let fixed_messages = messages
        .into_iter()
        .map(|mut message| {
            if message.role == Role::Assistant {
                for content in &mut message.content {
                    if let MessageContentBlock::Text(text) = content {
                        let trimmed = text.text.trim_end();
                        if trimmed.len() != text.text.len() {
                            issues.push(
                                "Trimmed trailing whitespace from assistant message".to_string(),
                            );
                            text.text = trimmed.to_string();
                        }
                    }
                }
            }
            message
        })
        .collect();

    (fixed_messages, issues)
}

fn remove_empty_messages(messages: Vec<Message>) -> (Vec<Message>, Vec<String>) {
    let mut issues = Vec::new();
    let filtered_messages = messages
        .into_iter()
        .filter(|msg| {
            if msg
                .content
                .iter()
                .all(|c| c.as_text().is_some_and(str::is_empty))
            {
                issues.push("Removed empty message".to_string());
                false
            } else {
                true
            }
        })
        .collect();
    (filtered_messages, issues)
}

/// Checks whether tool result content has any meaningful payload.
/// Text and resources must contain non-empty strings; images are always meaningful.
fn has_tool_result_content(content: &[ContentBlock]) -> bool {
    content.iter().any(|c| {
        if let Some(t) = c.as_text() {
            return !t.text.is_empty();
        }
        if let Some(r) = c.as_resource() {
            return !extract_text_from_resource(&r.resource).is_empty();
        }
        c.as_image().is_some()
    })
}

/// Fix tool results that would be empty when formatted for LLM APIs.
/// Some APIs (like Anthropic) reject tool_result blocks with empty content.
/// This adds a placeholder message for tool results that have no extractable text.
fn fix_empty_tool_results(messages: Vec<Message>) -> (Vec<Message>, Vec<String>) {
    let mut issues = Vec::new();

    let fixed_messages = messages
        .into_iter()
        .map(|mut message| {
            for content in &mut message.content {
                if let MessageContentBlock::ToolResponse(ref mut tool_response) = content {
                    if let Ok(ref mut result) = tool_response.tool_result {
                        if !has_tool_result_content(&result.content) {
                            // Add a placeholder text content so the tool result isn't empty
                            result.content.push(ContentBlock::text("(empty result)"));
                            issues.push(format!(
                                "Added placeholder to empty tool result '{}'",
                                tool_response.id
                            ));
                        }
                    }
                }
            }
            message
        })
        .collect();

    (fixed_messages, issues)
}

fn fix_tool_calling(mut messages: Vec<Message>) -> (Vec<Message>, Vec<String>) {
    let mut issues = Vec::new();
    let mut pending_tool_requests: HashSet<String> = HashSet::new();

    for message in &mut messages {
        let mut content_to_remove = Vec::new();

        match message.role {
            Role::User => {
                for (idx, content) in message.content.iter().enumerate() {
                    match content {
                        MessageContentBlock::ToolRequest(req) => {
                            content_to_remove.push(idx);
                            issues.push(format!(
                                "Removed tool request '{}' from user message",
                                req.id
                            ));
                        }
                        MessageContentBlock::ToolConfirmationRequest(req) => {
                            content_to_remove.push(idx);
                            issues.push(format!(
                                "Removed tool confirmation request '{}' from user message",
                                req.id
                            ));
                        }
                        MessageContentBlock::Thinking(_)
                        | MessageContentBlock::RedactedThinking(_) => {
                            content_to_remove.push(idx);
                            issues.push("Removed thinking content from user message".to_string());
                        }
                        MessageContentBlock::ToolResponse(resp) => {
                            if pending_tool_requests.contains(&resp.id) {
                                pending_tool_requests.remove(&resp.id);
                            } else {
                                content_to_remove.push(idx);
                                issues
                                    .push(format!("Removed orphaned tool response '{}'", resp.id));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Role::Assistant => {
                for (idx, content) in message.content.iter().enumerate() {
                    match content {
                        MessageContentBlock::ToolResponse(resp) => {
                            content_to_remove.push(idx);
                            issues.push(format!(
                                "Removed tool response '{}' from assistant message",
                                resp.id
                            ));
                        }
                        MessageContentBlock::ToolRequest(req) => {
                            pending_tool_requests.insert(req.id.clone());
                        }
                        _ => {}
                    }
                }
            }
        }

        for &idx in content_to_remove.iter().rev() {
            message.content.remove(idx);
        }
    }

    for message in &mut messages {
        if message.role == Role::Assistant {
            let mut content_to_remove = Vec::new();
            for (idx, content) in message.content.iter().enumerate() {
                if let MessageContentBlock::ToolRequest(req) = content {
                    if pending_tool_requests.contains(&req.id) {
                        content_to_remove.push(idx);
                        issues.push(format!("Removed orphaned tool request '{}'", req.id));
                    }
                }
            }
            for &idx in content_to_remove.iter().rev() {
                message.content.remove(idx);
            }
        }
    }
    let (messages, empty_removed) = remove_empty_messages(messages);
    issues.extend(empty_removed);
    (messages, issues)
}

/// Never merges across visibility or turn-context boundaries, so the result
/// is safe to persist.
pub fn merge_consecutive_messages(messages: Vec<Message>) -> (Vec<Message>, Vec<String>) {
    merge_consecutive(messages, false)
}

/// Merges regardless of visibility, for providers that require strict role
/// alternation. Never persist the result.
pub fn merge_consecutive_messages_for_request(messages: Vec<Message>) -> Vec<Message> {
    merge_consecutive(messages, true).0
}

fn merge_consecutive(
    messages: Vec<Message>,
    across_visibility: bool,
) -> (Vec<Message>, Vec<String>) {
    let mut issues = Vec::new();
    let mut merged_messages: Vec<Message> = Vec::new();

    for message in messages {
        if let Some(last) = merged_messages.last_mut() {
            let effective = effective_role(&message);
            if effective_role(last) == effective
                && (across_visibility
                    || (last.metadata.user_visible == message.metadata.user_visible
                        && last.metadata.turn_context == message.metadata.turn_context))
            {
                last.content.extend(message.content);
                issues.push(format!("Merged consecutive {} messages", effective));
                continue;
            }
        }
        merged_messages.push(message);
    }

    (merged_messages, issues)
}

/// Signed thinking carries a signature; redacted thinking is always signed.
/// Signed blocks must be replayed exactly; unsigned reasoning summaries need not.
fn is_signed_thinking(content: &MessageContentBlock) -> bool {
    match content {
        MessageContentBlock::Thinking(t) => !t.signature.is_empty(),
        MessageContentBlock::RedactedThinking(_) => true,
        _ => false,
    }
}

/// Drops duplicate signed thinking blocks, keeping the first occurrence. Some
/// signed-replay APIs (like Anthropic) reject a request that repeats the same
/// signed block more than once.
///
/// Duplicates arise two ways, both handled here:
///   - Within one assistant message, when a standalone thinking message is
///     merged with a tool-call message that re-embedded the same thinking.
///   - Across assistant messages, when the agent splits one provider turn into
///     several tool-call messages (interleaved with tool results) that each
///     carry a copy of the turn's signed thinking.
///
/// The `seen` set spans the whole conversation. A signed block carries a
/// cryptographic signature unique to its generation, so an exact (text +
/// signature) match can only be the same turn's thinking copied onto split
/// messages — never two genuinely distinct thoughts. Unsigned reasoning
/// summaries are left untouched, since providers like Kimi/DeepSeek require
/// them echoed on every tool-call message.
///
/// This runs before any provider formatter, so it covers every Claude transport
/// (direct Anthropic, Bedrock, Databricks, Vertex) in one place.
fn dedupe_signed_thinking(messages: Vec<Message>) -> (Vec<Message>, Vec<String>) {
    let mut issues = Vec::new();
    let mut seen: Vec<MessageContentBlock> = Vec::new();

    let fixed_messages = messages
        .into_iter()
        .map(|mut message| {
            if message.role != Role::Assistant {
                return message;
            }

            let original_len = message.content.len();
            let mut deduped: Vec<MessageContentBlock> = Vec::with_capacity(original_len);
            for content in &message.content {
                let is_signed = is_signed_thinking(content);
                if is_signed && seen.contains(content) {
                    continue;
                }
                if is_signed {
                    seen.push(content.clone());
                }
                deduped.push(content.clone());
            }

            if deduped.len() != original_len {
                issues.push("Removed duplicate signed thinking block".to_string());
                message.content = deduped;
            }
            message
        })
        .collect();

    (fixed_messages, issues)
}

fn has_tool_response(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|content| matches!(content, MessageContentBlock::ToolResponse(_)))
}

pub const TURN_CONTEXT_TAG: &str = "turn-context";
pub const CURRENT_TIME_TAG: &str = "current-time";
pub const WORKING_DIRECTORY_TAG: &str = "working-directory";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveRole {
    User,
    Assistant,
    Tool,
}

impl std::fmt::Display for EffectiveRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User => write!(f, "user"),
            Self::Assistant => write!(f, "assistant"),
            Self::Tool => write!(f, "tool"),
        }
    }
}

pub fn effective_role(message: &Message) -> EffectiveRole {
    if message.role == Role::User && has_tool_response(message) {
        EffectiveRole::Tool
    } else {
        match message.role {
            Role::User => EffectiveRole::User,
            Role::Assistant => EffectiveRole::Assistant,
        }
    }
}

fn fix_lead_trail(mut messages: Vec<Message>) -> (Vec<Message>, Vec<String>) {
    let mut issues = Vec::new();

    if let Some(first) = messages.first() {
        if first.role == Role::Assistant {
            messages.remove(0);
            issues.push("Removed leading assistant message".to_string());
        }
    }

    if let Some(last) = messages.last() {
        if last.role == Role::Assistant {
            messages.pop();
            issues.push("Removed trailing assistant message".to_string());
        }
    }

    (messages, issues)
}

const PLACEHOLDER_USER_MESSAGE: &str = "Hello";

fn populate_if_empty(mut messages: Vec<Message>) -> (Vec<Message>, Vec<String>) {
    let mut issues = Vec::new();

    if messages.is_empty() {
        issues.push("Added placeholder user message to empty conversation".to_string());
        messages.push(Message::user().with_text(PLACEHOLDER_USER_MESSAGE));
    }
    (messages, issues)
}

pub fn debug_conversation_fix(
    messages: &[Message],
    fixed: &[Message],
    issues: &[String],
) -> String {
    let mut output = String::new();

    output.push_str("=== CONVERSATION FIX DEBUG ===\n\n");

    output.push_str("BEFORE:\n");
    for (i, msg) in messages.iter().enumerate() {
        output.push_str(&format!("  [{}] {}\n", i, msg.debug()));
    }

    output.push_str("\nISSUES FOUND:\n");
    if issues.is_empty() {
        output.push_str("  (none)\n");
    } else {
        for issue in issues {
            output.push_str(&format!("  - {}\n", issue));
        }
    }

    output.push_str("\nAFTER:\n");
    for (i, msg) in fixed.iter().enumerate() {
        output.push_str(&format!("  [{}] {}\n", i, msg.debug()));
    }

    output.push_str("\n==============================\n");
    output
}

#[cfg(test)]
mod tests {
    use crate::conversation::message::{
        InferenceMetadata, Message, MessageContentBlock, MessageMetadata,
    };
    use crate::conversation::{debug_conversation_fix, fix_conversation, Conversation};
    use rmcp::model::{CallToolRequestParams, Role};
    use rmcp::object;

    macro_rules! assert_has_issues_unordered {
        ($fixed:expr, $issues:expr, $($expected:expr),+ $(,)?) => {
            {
                let mut expected: Vec<&str> = vec![$($expected),+];
                let mut actual: Vec<&str> = $issues.iter().map(|s| s.as_str()).collect();
                expected.sort();
                actual.sort();

                if actual != expected {
                    panic!(
                        "assertion failed: issues don't match\nexpected: {:?}\n  actual: {:?}. Fixed conversation is:\n{:#?}",
                        expected, $issues, $fixed,
                    );
                }
            }
        };
    }

    fn run_verify(messages: Vec<Message>) -> (Vec<Message>, Vec<String>) {
        let (fixed, issues) = fix_conversation(Conversation::new_unvalidated(messages.clone()));

        // Uncomment the following line to print the debug report
        // let report = debug_conversation_fix(&messages, &fixed, &issues);
        // print!("\n{}", report);

        let (_fixed, issues_with_fixed) = fix_conversation(fixed.clone());
        assert_eq!(
            issues_with_fixed.len(),
            0,
            "Fixed conversation should have no issues, but found: {:?}\n\n{}",
            issues_with_fixed,
            debug_conversation_fix(&messages, fixed.messages(), &issues)
        );
        (fixed.messages().clone(), issues)
    }

    #[test]
    fn test_valid_conversation() {
        use rmcp::model::ContentBlock;

        let all_messages = [
            Message::user().with_text("Can you help me search for something?"),
            Message::assistant()
                .with_text("I'll help you search.")
                .with_tool_request(
                    "search_1",
                    Ok(CallToolRequestParams::new("web_search")
                        .with_arguments(object!({"query": "rust programming"}))),
                ),
            Message::user().with_tool_response(
                "search_1",
                Ok(rmcp::model::CallToolResult::success(vec![
                    ContentBlock::text("Search results here"),
                ])),
            ),
            Message::assistant().with_text("Based on the search results, here's what I found..."),
        ];

        for i in 1..=all_messages.len() {
            let messages = Conversation::new_unvalidated(all_messages[..i].to_vec());
            if messages.last().unwrap().role == Role::User {
                let (fixed, issues) = fix_conversation(messages.clone());
                assert_eq!(
                    fixed.len(),
                    messages.len(),
                    "Step {}: Length should match",
                    i
                );
                assert!(
                    issues.is_empty(),
                    "Step {}: Should have no issues, but found: {:?}",
                    i,
                    issues
                );
                assert_eq!(
                    fixed.messages(),
                    messages.messages(),
                    "Step {}: Messages should be unchanged",
                    i
                );
            }
        }
    }

    #[test]
    fn test_role_alternation_and_content_placement_issues() {
        use rmcp::model::ContentBlock;

        let messages = vec![
            Message::user().with_text("Hello"),
            Message::user().with_text("Another user message"),
            Message::assistant()
                .with_text("Response")
                .with_tool_response(
                    "orphan_1",
                    Ok(rmcp::model::CallToolResult::success(vec![
                        ContentBlock::text("result"),
                    ])),
                ), // Wrong role
            Message::assistant().with_thinking("Let me think", "sig"),
            Message::user()
                .with_tool_request(
                    "bad_req",
                    Ok(CallToolRequestParams::new("search").with_arguments(object!({}))),
                )
                .with_text("User with bad tool request"),
        ];

        let (fixed, issues) = run_verify(messages);

        assert_eq!(fixed.len(), 3);

        assert_has_issues_unordered!(
            fixed,
            issues,
            "Merged consecutive assistant messages",
            "Merged consecutive user messages",
            "Removed tool response 'orphan_1' from assistant message",
            "Removed tool request 'bad_req' from user message",
        );

        assert_eq!(fixed[0].role, Role::User);
        assert_eq!(fixed[1].role, Role::Assistant);
        assert_eq!(fixed[2].role, Role::User);

        assert_eq!(fixed[0].content.len(), 2);
    }

    #[test]
    fn merge_does_not_cross_user_visibility() {
        let visible = Message::user().with_text("what is in main.rs?");
        let hidden = Message::user()
            .with_text("<turn-context>frozen</turn-context>")
            .with_visibility(false, true);

        let (fixed, _) = fix_conversation(Conversation::new_unvalidated(vec![visible, hidden]));
        assert_eq!(
            fixed.messages().len(),
            2,
            "the persistable form keeps the agent-only event separate"
        );
        assert!(fixed.messages()[0].is_user_visible());
        assert!(!fixed.messages()[1].is_user_visible());

        let merged =
            crate::conversation::merge_consecutive_messages_for_request(fixed.messages().clone());
        assert_eq!(
            merged.len(),
            1,
            "the request form merges to satisfy role alternation"
        );
        assert_eq!(merged[0].content.len(), 2);
    }

    #[test]
    fn merge_does_not_erase_turn_context_marker() {
        let preserved_prompt = Message::user()
            .with_text("the preserved prompt")
            .with_metadata(MessageMetadata::agent_only());
        let carried_event = Message::user()
            .with_text("<turn-context>frozen</turn-context>")
            .with_metadata(MessageMetadata::agent_only().with_turn_context());

        let (fixed, _) = fix_conversation(Conversation::new_unvalidated(vec![
            preserved_prompt,
            carried_event,
        ]));
        assert_eq!(
            fixed.messages().len(),
            2,
            "the persistable form must not fold a turn-context event into the prompt"
        );
        assert!(!fixed.messages()[0].is_turn_context());
        assert!(fixed.messages()[1].is_turn_context());

        let merged =
            crate::conversation::merge_consecutive_messages_for_request(fixed.messages().clone());
        assert_eq!(
            merged.len(),
            1,
            "the request form still merges to satisfy role alternation"
        );
    }

    #[test]
    fn test_orphaned_tools_and_empty_messages() {
        use rmcp::model::ContentBlock;

        // This conversation completely collapses. the first user message is invalid
        // then we remove the empty user message and the wrong tool response
        // then we collapse the assistant messages
        // which we then remove because you can't end a conversation with an assistant message
        let messages = vec![
            Message::assistant()
                .with_text("I'll search for you")
                .with_tool_request(
                    "search_1",
                    Ok(CallToolRequestParams::new("search").with_arguments(object!({}))),
                ),
            Message::user(),
            Message::user().with_tool_response(
                "wrong_id",
                Ok(rmcp::model::CallToolResult::success(vec![
                    ContentBlock::text("result"),
                ])),
            ),
            Message::assistant().with_tool_request(
                "search_2",
                Ok(CallToolRequestParams::new("search").with_arguments(object!({}))),
            ),
        ];

        let (fixed, issues) = run_verify(messages);

        assert_eq!(fixed.len(), 1);

        assert_has_issues_unordered!(
            fixed,
            issues,
            "Removed empty message",
            "Removed orphaned tool response 'wrong_id'",
            "Removed orphaned tool request 'search_1'",
            "Removed orphaned tool request 'search_2'",
            "Removed empty message",
            "Removed empty message",
            "Removed leading assistant message",
            "Added placeholder user message to empty conversation",
        );

        assert_eq!(fixed[0].role, Role::User);
        assert_eq!(fixed[0].as_concat_text(), "Hello");
    }

    #[test]
    fn test_real_world_consecutive_assistant_messages() {
        use rmcp::model::ContentBlock;

        let conversation = Conversation::new_unvalidated(vec![
            Message::user().with_text("run ls in the current directory and then run a word count on the smallest file"),

            Message::assistant()
                .with_text("I'll help you run `ls` in the current directory and then perform a word count on the smallest file. Let me start by listing the directory contents.")
                .with_tool_request("toolu_bdrk_018adWbP4X26CfoJU5hkhu3i", Ok(CallToolRequestParams::new("developer__shell").with_arguments(object!({"command": "ls -la"})))),

            Message::assistant()
                .with_text("Now I'll identify the smallest file by size. Looking at the output, I can see that both `slack.yaml` and `subrecipes.yaml` have a size of 0 bytes, making them the smallest files. I'll run a word count on one of them:")
                .with_tool_request("toolu_bdrk_01KgDYHs4fAodi22NqxRzmwx", Ok(CallToolRequestParams::new("developer__shell").with_arguments(object!({"command": "wc slack.yaml"})))),

            Message::user()
                .with_tool_response("toolu_bdrk_01KgDYHs4fAodi22NqxRzmwx", Ok(rmcp::model::CallToolResult::success(vec![ContentBlock::text("0 0 0 slack.yaml")]))),

            Message::assistant()
                .with_text("I ran `ls -la` in the current directory and found several files. Looking at the file sizes, I can see that both `slack.yaml` and `subrecipes.yaml` are 0 bytes (the smallest files). I ran a word count on `slack.yaml` which shows: **0 lines**, **0 words**, **0 characters**"),
            Message::user().with_text("thanks!"),
        ]);

        let (fixed, issues) = fix_conversation(conversation);

        assert_eq!(fixed.len(), 5);
        assert_has_issues_unordered!(
            fixed,
            issues,
            "Removed orphaned tool request 'toolu_bdrk_018adWbP4X26CfoJU5hkhu3i'",
            "Merged consecutive assistant messages"
        )
    }

    #[test]
    fn test_tool_response_effective_role() {
        use rmcp::model::ContentBlock;

        let messages = vec![
            Message::user().with_text("Search for something"),
            Message::assistant()
                .with_text("I'll search for you")
                .with_tool_request(
                    "search_1",
                    Ok(CallToolRequestParams::new("search").with_arguments(object!({}))),
                ),
            Message::user().with_tool_response(
                "search_1",
                Ok(rmcp::model::CallToolResult::success(vec![
                    ContentBlock::text("search results"),
                ])),
            ),
            Message::user().with_text("Thanks!"),
        ];

        let (_fixed, issues) = run_verify(messages);
        assert!(issues.is_empty());
    }

    #[test]
    fn test_merge_text_content_items() {
        use crate::conversation::message::MessageContentBlock;
        use rmcp::model::TextContent;

        let mut message = Message::assistant().with_text("Hello");

        message
            .content
            .push(MessageContentBlock::Text(TextContent::new(" world")));
        message
            .content
            .push(MessageContentBlock::Text(TextContent::new("!")));

        let messages = vec![
            Message::user().with_text("hello"),
            message,
            Message::user().with_text("thanks"),
        ];

        let (fixed, issues) = run_verify(messages);

        assert_eq!(fixed.len(), 3);
        assert_has_issues_unordered!(fixed, issues, "Merged text content");

        let fixed_msg = &fixed[1];
        assert_eq!(fixed_msg.content.len(), 1);

        if let MessageContentBlock::Text(text_content) = &fixed_msg.content[0] {
            assert_eq!(text_content.text, "Hello world!");
        } else {
            panic!("Expected text content");
        }
    }

    #[test]
    fn test_merge_text_content_items_with_mixed_content() {
        use crate::conversation::message::MessageContentBlock;
        use rmcp::model::TextContent;

        let mut image_message = Message::assistant().with_text("Look at");

        image_message
            .content
            .push(MessageContentBlock::Text(TextContent::new(" this image:")));

        image_message = image_message.with_image("", "");

        let messages = vec![
            Message::user().with_text("hello"),
            image_message,
            Message::user().with_text("thanks"),
        ];

        let (fixed, issues) = run_verify(messages);

        assert_eq!(fixed.len(), 3);
        assert_has_issues_unordered!(fixed, issues, "Merged text content");
        let fixed_msg = &fixed[1];

        assert_eq!(fixed_msg.content.len(), 2);
        if let MessageContentBlock::Text(text_content) = &fixed_msg.content[0] {
            assert_eq!(text_content.text, "Look at this image:");
        } else {
            panic!("Expected first item to be text content");
        }

        if let MessageContentBlock::Image(_) = &fixed_msg.content[1] {
            // Good
        } else {
            panic!("Expected second item to be an image");
        }
    }

    #[test]
    fn test_streamed_text_with_different_audiences_is_not_merged() {
        use rmcp::model::{Annotations, Role, TextContent};

        let text = |value: &str, audience| {
            MessageContentBlock::Text(
                TextContent::new(value)
                    .with_annotations(Annotations::default().with_audience(vec![audience])),
            )
        };

        for (first, second) in [(Role::User, Role::Assistant), (Role::Assistant, Role::User)] {
            let mut conversation = Conversation::empty();
            conversation.push(
                Message::assistant()
                    .with_id("stream-1")
                    .with_content(text("first", first.clone())),
            );
            conversation.push(
                Message::assistant()
                    .with_id("stream-1")
                    .with_content(text("second", second)),
            );

            let message = conversation.last().unwrap();
            assert_eq!(message.content.len(), 2);
            assert_eq!(
                message
                    .user_visible_content()
                    .content
                    .iter()
                    .filter_map(|content| match content {
                        MessageContentBlock::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                if first == Role::User {
                    vec!["first"]
                } else {
                    vec!["second"]
                }
            );
            assert_eq!(
                message
                    .agent_visible_content()
                    .content
                    .iter()
                    .filter_map(|content| match content {
                        MessageContentBlock::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                if first == Role::Assistant {
                    vec!["first"]
                } else {
                    vec!["second"]
                }
            );
        }
    }

    #[test]
    fn test_user_visible_messages_projects_content_and_drops_hidden_rows() {
        use rmcp::model::{Annotations, Role, TextContent};

        let assistant_only = |value: &str| {
            MessageContentBlock::Text(
                TextContent::new(value)
                    .with_annotations(Annotations::default().with_audience(vec![Role::Assistant])),
            )
        };
        let conversation = Conversation::new_unvalidated([
            Message::assistant()
                .with_content(assistant_only("content hidden by audience"))
                .agent_only(),
            Message::assistant()
                .with_content(assistant_only("private"))
                .with_text("public"),
        ]);

        let projected = conversation.user_visible_messages();

        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].as_concat_text(), "public");
    }

    #[test]
    fn test_agent_visible_messages_projects_content_and_drops_hidden_rows() {
        use rmcp::model::{Annotations, Role, TextContent};

        let user_only = |value: &str| {
            MessageContentBlock::Text(
                TextContent::new(value)
                    .with_annotations(Annotations::default().with_audience(vec![Role::User])),
            )
        };
        let conversation = Conversation::new_unvalidated([
            Message::assistant()
                .with_content(user_only("content hidden from agent"))
                .user_only(),
            Message::assistant()
                .with_content(user_only("private from agent"))
                .with_text("shared with agent"),
        ]);

        let projected = conversation.agent_visible_messages();

        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].as_concat_text(), "shared with agent");
    }

    #[test]
    fn test_agent_visible_non_visible_message_ordering_with_fixes() {
        // Test that non-visible messages maintain their position relative to visible messages
        // even when visible messages are fixed (merged, removed, etc.)

        // Create messages with mixed visibility where visible ones need fixing
        let mut msg1_user = Message::user().with_text("First user message");
        msg1_user.metadata.agent_visible = true;

        let mut msg2_non_visible = Message::user().with_text("Non-visible note 1");
        msg2_non_visible.metadata.agent_visible = false;

        // These two consecutive user messages should be merged (triggering a fix)
        let mut msg3_user = Message::user().with_text("Second user message");
        msg3_user.metadata.agent_visible = true;

        let mut msg4_user = Message::user().with_text("Third user message");
        msg4_user.metadata.agent_visible = true;

        let mut msg5_non_visible = Message::user().with_text("Non-visible note 2");
        msg5_non_visible.metadata.agent_visible = false;

        let mut msg6_assistant = Message::assistant().with_text("Assistant response");
        msg6_assistant.metadata.agent_visible = true;

        let mut msg7_non_visible = Message::user().with_text("Non-visible note 3");
        msg7_non_visible.metadata.agent_visible = false;

        let mut msg8_user = Message::user().with_text("Final user message");
        msg8_user.metadata.agent_visible = true;

        let messages = vec![
            msg1_user.clone(),
            msg2_non_visible.clone(),
            msg3_user.clone(),
            msg4_user.clone(),
            msg5_non_visible.clone(),
            msg6_assistant.clone(),
            msg7_non_visible.clone(),
            msg8_user.clone(),
        ];

        let (fixed, issues) = fix_conversation(Conversation::new_unvalidated(messages.clone()));

        // Should have merged consecutive user messages
        assert!(!issues.is_empty());
        assert!(issues.iter().any(|i| i.contains("Merged consecutive")));

        let fixed_messages = fixed.messages();

        // Verify non-visible messages are still present
        let non_visible_texts: Vec<String> = fixed_messages
            .iter()
            .filter(|m| !m.metadata.agent_visible)
            .map(|m| m.as_concat_text())
            .collect();

        assert_eq!(non_visible_texts.len(), 3);
        assert_eq!(non_visible_texts[0], "Non-visible note 1");
        assert_eq!(non_visible_texts[1], "Non-visible note 2");
        assert_eq!(non_visible_texts[2], "Non-visible note 3");

        // Verify visible messages were processed
        let visible_texts: Vec<String> = fixed_messages
            .iter()
            .filter(|m| m.metadata.agent_visible)
            .map(|m| m.as_concat_text())
            .collect();

        // Should have 3 visible messages: first user, merged user messages, assistant, final user
        // But after merging consecutive users and fixing lead/trail, we get fewer
        assert!(!visible_texts.is_empty());

        // The key assertion: non-visible messages should be preserved and not reordered
        // relative to each other
        let mut found_note1 = false;
        let mut found_note2 = false;

        for msg in fixed_messages {
            let text = msg.as_concat_text();
            if text == "Non-visible note 1" {
                assert!(!found_note2 && !found_note1);
                found_note1 = true;
            } else if text == "Non-visible note 2" {
                assert!(found_note1 && !found_note2);
                found_note2 = true;
            } else if text == "Non-visible note 3" {
                assert!(found_note1 && found_note2);
            }
        }
    }

    #[test]
    fn test_shadow_map_with_multiple_consecutive_merges() {
        // Test the shadow map handles multiple consecutive visible messages that all merge
        let mut msg1 = Message::user().with_text("User 1");
        msg1.metadata.agent_visible = true;

        let mut msg2_non_vis = Message::user().with_text("Non-visible A");
        msg2_non_vis.metadata.agent_visible = false;

        let mut msg3 = Message::user().with_text("User 2");
        msg3.metadata.agent_visible = true;

        let mut msg4 = Message::user().with_text("User 3");
        msg4.metadata.agent_visible = true;

        let mut msg5 = Message::user().with_text("User 4");
        msg5.metadata.agent_visible = true;

        let mut msg6_non_vis = Message::user().with_text("Non-visible B");
        msg6_non_vis.metadata.agent_visible = false;

        let messages = vec![
            msg1,
            msg2_non_vis.clone(),
            msg3,
            msg4,
            msg5,
            msg6_non_vis.clone(),
        ];

        let (fixed, issues) = fix_conversation(Conversation::new_unvalidated(messages));

        // Should have merged the consecutive user messages
        assert!(issues.iter().any(|i| i.contains("Merged consecutive")));

        let fixed_messages = fixed.messages();

        // Non-visible messages should still be present and in order
        let non_visible: Vec<String> = fixed_messages
            .iter()
            .filter(|m| !m.metadata.agent_visible)
            .map(|m| m.as_concat_text())
            .collect();

        assert_eq!(non_visible.len(), 2);
        assert_eq!(non_visible[0], "Non-visible A");
        assert_eq!(non_visible[1], "Non-visible B");

        // The merged message should contain all the user texts
        let visible: Vec<String> = fixed_messages
            .iter()
            .filter(|m| m.metadata.agent_visible)
            .map(|m| m.as_concat_text())
            .collect();

        assert_eq!(visible.len(), 1);
        assert!(visible[0].contains("User 1"));
        assert!(visible[0].contains("User 2"));
        assert!(visible[0].contains("User 3"));
        assert!(visible[0].contains("User 4"));
    }

    #[test]
    fn test_shadow_map_with_leading_trailing_removal() {
        // Test that shadow map handles removal of leading/trailing assistant messages
        let mut msg1_assistant = Message::assistant().with_text("Leading assistant");
        msg1_assistant.metadata.agent_visible = true;

        let mut msg2_non_vis = Message::user().with_text("Non-visible note");
        msg2_non_vis.metadata.agent_visible = false;

        let mut msg3_user = Message::user().with_text("User message");
        msg3_user.metadata.agent_visible = true;

        let mut msg4_assistant = Message::assistant().with_text("Assistant response");
        msg4_assistant.metadata.agent_visible = true;

        let mut msg5_assistant = Message::assistant().with_text("Trailing assistant");
        msg5_assistant.metadata.agent_visible = true;

        let messages = vec![
            msg1_assistant,
            msg2_non_vis.clone(),
            msg3_user,
            msg4_assistant,
            msg5_assistant,
        ];

        let (fixed, issues) = fix_conversation(Conversation::new_unvalidated(messages));

        // Should have merged consecutive assistants, removed leading, and removed trailing
        assert!(issues
            .iter()
            .any(|i| i.contains("Merged consecutive assistant")));
        assert!(issues
            .iter()
            .any(|i| i.contains("Removed leading assistant")));
        assert!(issues
            .iter()
            .any(|i| i.contains("Removed trailing assistant")));

        let fixed_messages = fixed.messages();

        // Non-visible message should still be present
        let non_visible: Vec<String> = fixed_messages
            .iter()
            .filter(|m| !m.metadata.agent_visible)
            .map(|m| m.as_concat_text())
            .collect();

        assert_eq!(non_visible.len(), 1);
        assert_eq!(non_visible[0], "Non-visible note");

        // The two consecutive assistant messages get merged, then the merged message
        // is removed as trailing, leaving only the user message
        let visible: Vec<String> = fixed_messages
            .iter()
            .filter(|m| m.metadata.agent_visible)
            .map(|m| m.as_concat_text())
            .collect();

        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0], "User message");
    }

    #[test]
    fn test_shadow_map_all_visible_messages_removed() {
        // Edge case: all visible messages are removed, only non-visible remain
        let mut msg1_assistant = Message::assistant().with_text("Only assistant");
        msg1_assistant.metadata.agent_visible = true;

        let mut msg2_non_vis = Message::user().with_text("Non-visible note 1");
        msg2_non_vis.metadata.agent_visible = false;

        let mut msg3_non_vis = Message::user().with_text("Non-visible note 2");
        msg3_non_vis.metadata.agent_visible = false;

        let messages = vec![msg1_assistant, msg2_non_vis, msg3_non_vis];

        let (fixed, issues) = fix_conversation(Conversation::new_unvalidated(messages));

        // Should have removed the assistant and added placeholder
        assert!(issues
            .iter()
            .any(|i| i.contains("Removed leading assistant")));
        assert!(issues.iter().any(|i| i.contains("Added placeholder")));

        let fixed_messages = fixed.messages();

        // Non-visible messages should still be present
        let non_visible: Vec<String> = fixed_messages
            .iter()
            .filter(|m| !m.metadata.agent_visible)
            .map(|m| m.as_concat_text())
            .collect();

        assert_eq!(non_visible.len(), 2);
        assert_eq!(non_visible[0], "Non-visible note 1");
        assert_eq!(non_visible[1], "Non-visible note 2");

        // Should have placeholder user message
        let visible: Vec<String> = fixed_messages
            .iter()
            .filter(|m| m.metadata.agent_visible)
            .map(|m| m.as_concat_text())
            .collect();

        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0], "Hello");
    }

    #[test]
    fn test_empty_tool_result_gets_placeholder() {
        // Test that tool results with empty content get a placeholder added
        let messages = vec![
            Message::user().with_text("Search for something"),
            Message::assistant()
                .with_text("I'll search for you")
                .with_tool_request(
                    "search_1",
                    Ok(CallToolRequestParams::new("search").with_arguments(object!({}))),
                ),
            Message::user().with_tool_response(
                "search_1",
                Ok(rmcp::model::CallToolResult::success(vec![])), // Empty content - this should get a placeholder
            ),
            Message::user().with_text("Thanks!"),
        ];

        let (fixed, issues) = fix_conversation(Conversation::new_unvalidated(messages));

        // Should have added a placeholder
        assert!(issues
            .iter()
            .any(|i| i.contains("Added placeholder to empty tool result")));

        // Find the tool response and verify it has content now
        let tool_response_msg = fixed
            .messages()
            .iter()
            .find(|m| {
                m.content.iter().any(|c| {
                    matches!(
                        c,
                        crate::conversation::message::MessageContentBlock::ToolResponse(_)
                    )
                })
            })
            .expect("Should have a tool response message");

        if let crate::conversation::message::MessageContentBlock::ToolResponse(resp) =
            &tool_response_msg.content[0]
        {
            if let Ok(result) = &resp.tool_result {
                assert!(!result.content.is_empty(), "Content should not be empty");
                // Verify the placeholder text
                let text = result.content[0]
                    .as_text()
                    .expect("Should be text content")
                    .text
                    .clone();
                assert_eq!(text, "(empty result)");
            } else {
                panic!("Tool result should be Ok");
            }
        } else {
            panic!("First content should be ToolResponse");
        }
    }

    #[test]
    fn test_shadow_map_preserves_interleaving_pattern() {
        // Test that complex interleaving patterns are preserved
        let mut msg1_user = Message::user().with_text("User 1");
        msg1_user.metadata.agent_visible = true;

        let mut msg2_non_vis = Message::user().with_text("Non-vis A");
        msg2_non_vis.metadata.agent_visible = false;

        let mut msg3_assistant = Message::assistant().with_text("Assistant 1");
        msg3_assistant.metadata.agent_visible = true;

        let mut msg4_non_vis = Message::user().with_text("Non-vis B");
        msg4_non_vis.metadata.agent_visible = false;

        let mut msg5_user = Message::user().with_text("User 2");
        msg5_user.metadata.agent_visible = true;

        let mut msg6_non_vis = Message::user().with_text("Non-vis C");
        msg6_non_vis.metadata.agent_visible = false;

        let messages = vec![
            msg1_user,
            msg2_non_vis,
            msg3_assistant,
            msg4_non_vis,
            msg5_user,
            msg6_non_vis,
        ];

        let (fixed, issues) = fix_conversation(Conversation::new_unvalidated(messages));

        // Should have no issues for this valid conversation
        assert!(issues.is_empty());

        let fixed_messages = fixed.messages();

        // Verify the interleaving pattern is preserved
        assert_eq!(fixed_messages.len(), 6);

        assert_eq!(fixed_messages[0].as_concat_text(), "User 1");
        assert!(fixed_messages[0].metadata.agent_visible);

        assert_eq!(fixed_messages[1].as_concat_text(), "Non-vis A");
        assert!(!fixed_messages[1].metadata.agent_visible);

        assert_eq!(fixed_messages[2].as_concat_text(), "Assistant 1");
        assert!(fixed_messages[2].metadata.agent_visible);

        assert_eq!(fixed_messages[3].as_concat_text(), "Non-vis B");
        assert!(!fixed_messages[3].metadata.agent_visible);

        assert_eq!(fixed_messages[4].as_concat_text(), "User 2");
        assert!(fixed_messages[4].metadata.agent_visible);

        assert_eq!(fixed_messages[5].as_concat_text(), "Non-vis C");
        assert!(!fixed_messages[5].metadata.agent_visible);
    }

    #[test]
    fn test_dedupes_duplicate_signed_thinking_around_tool_call() {
        use crate::conversation::message::MessageContentBlock;
        use rmcp::model::ContentBlock;

        // Reproduces the Anthropic 400 scenario: a standalone signed thinking
        // message immediately followed by an assistant message that repeats the
        // same signed thinking plus a tool_use. After merging consecutive
        // assistant messages these become two adjacent identical thinking blocks.
        let messages = vec![
            Message::user().with_text("Do the thing"),
            Message::assistant().with_thinking("Let me think about this", "sig-1"),
            Message::assistant()
                .with_thinking("Let me think about this", "sig-1")
                .with_tool_request(
                    "tool_1",
                    Ok(CallToolRequestParams::new("do_thing").with_arguments(object!({"x": 1}))),
                ),
            Message::user().with_tool_response(
                "tool_1",
                Ok(rmcp::model::CallToolResult::success(vec![
                    ContentBlock::text("done"),
                ])),
            ),
            Message::user().with_text("Now continue"),
        ];

        let (fixed, issues) = fix_conversation(Conversation::new_unvalidated(messages));

        assert!(
            issues
                .iter()
                .any(|i| i == "Removed duplicate signed thinking block"),
            "expected dedupe issue, got: {:?}",
            issues
        );

        let fixed_messages = fixed.messages();
        let assistant = fixed_messages
            .iter()
            .find(|m| {
                m.role == Role::Assistant
                    && m.content
                        .iter()
                        .any(|c| matches!(c, MessageContentBlock::ToolRequest(_)))
            })
            .expect("assistant tool-call message should exist");

        let thinking_count = assistant
            .content
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    MessageContentBlock::Thinking(_) | MessageContentBlock::RedactedThinking(_)
                )
            })
            .count();
        assert_eq!(
            thinking_count, 1,
            "duplicate signed thinking should be collapsed to one block"
        );
    }

    #[test]
    fn test_keeps_distinct_signed_thinking_blocks_in_assistant_message() {
        use crate::conversation::message::MessageContentBlock;
        use rmcp::model::ContentBlock;

        // Distinct signed thinking blocks (different signatures) must be preserved.
        let messages = vec![
            Message::user().with_text("Do the thing"),
            Message::assistant()
                .with_thinking("First thought", "sig-A")
                .with_thinking("Second thought", "sig-B")
                .with_tool_request(
                    "tool_1",
                    Ok(CallToolRequestParams::new("do_thing").with_arguments(object!({"x": 1}))),
                ),
            Message::user().with_tool_response(
                "tool_1",
                Ok(rmcp::model::CallToolResult::success(vec![
                    ContentBlock::text("done"),
                ])),
            ),
            Message::user().with_text("Now continue"),
        ];

        let (fixed, issues) = fix_conversation(Conversation::new_unvalidated(messages));

        assert!(
            !issues
                .iter()
                .any(|i| i == "Removed duplicate signed thinking block"),
            "should not dedupe distinct thinking blocks, got: {:?}",
            issues
        );

        let fixed_messages = fixed.messages();
        let thinking_count = fixed_messages[1]
            .content
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    MessageContentBlock::Thinking(_) | MessageContentBlock::RedactedThinking(_)
                )
            })
            .count();
        assert_eq!(thinking_count, 2, "distinct thinking blocks must be kept");
    }

    #[test]
    fn test_keeps_duplicate_unsigned_thinking_blocks() {
        use crate::conversation::message::MessageContentBlock;

        // Unsigned thinking (reasoning summaries from non-Anthropic providers)
        // can legitimately repeat and must not be dropped, since only signed
        // blocks trigger the Anthropic exact-replay 400.
        let messages = vec![
            Message::user().with_text("Do the thing"),
            Message::assistant()
                .with_thinking("same reasoning", "")
                .with_thinking("same reasoning", ""),
            Message::user().with_text("Now continue"),
        ];

        let (fixed, issues) = fix_conversation(Conversation::new_unvalidated(messages));

        assert!(
            !issues
                .iter()
                .any(|i| i == "Removed duplicate signed thinking block"),
            "unsigned thinking must not be deduped, got: {:?}",
            issues
        );

        let fixed_messages = fixed.messages();
        let thinking_count = fixed_messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|c| matches!(c, MessageContentBlock::Thinking(_)))
            .count();
        assert_eq!(
            thinking_count, 2,
            "duplicate unsigned thinking blocks must be kept"
        );
    }

    #[test]
    fn test_dedupes_signed_thinking_across_split_tool_messages() {
        use crate::conversation::message::MessageContentBlock;
        use rmcp::model::ContentBlock;

        // The agent splits one provider turn with multiple tool calls into one
        // assistant message per call (interleaved with tool results), each
        // carrying the same signed thinking. merge_consecutive_messages cannot
        // merge them because tool results sit between, so the dedupe must span
        // messages and keep the signed block only on the first.
        let messages = vec![
            Message::user().with_text("Use both tools"),
            Message::assistant()
                .with_thinking("multi-tool reasoning", "sig-1")
                .with_tool_request(
                    "call_1",
                    Ok(CallToolRequestParams::new("tool_a").with_arguments(object!({"p": 1}))),
                ),
            Message::user().with_tool_response(
                "call_1",
                Ok(rmcp::model::CallToolResult::success(vec![
                    ContentBlock::text("ok"),
                ])),
            ),
            Message::assistant()
                .with_thinking("multi-tool reasoning", "sig-1")
                .with_tool_request(
                    "call_2",
                    Ok(CallToolRequestParams::new("tool_b").with_arguments(object!({"p": 2}))),
                ),
            Message::user().with_tool_response(
                "call_2",
                Ok(rmcp::model::CallToolResult::success(vec![
                    ContentBlock::text("ok"),
                ])),
            ),
            Message::user().with_text("Now continue"),
        ];

        let (fixed, issues) = fix_conversation(Conversation::new_unvalidated(messages));

        assert!(
            issues
                .iter()
                .any(|i| i == "Removed duplicate signed thinking block"),
            "expected cross-message dedupe issue, got: {:?}",
            issues
        );

        let fixed_messages = fixed.messages();
        let total_thinking = fixed_messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|c| {
                matches!(
                    c,
                    MessageContentBlock::Thinking(_) | MessageContentBlock::RedactedThinking(_)
                )
            })
            .count();
        assert_eq!(
            total_thinking, 1,
            "the repeated signed block must survive only once across the turn"
        );

        let total_tool_requests = fixed_messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|c| matches!(c, MessageContentBlock::ToolRequest(_)))
            .count();
        assert_eq!(total_tool_requests, 2, "both tool calls must be preserved");

        // The first split message keeps the thinking; the second loses it.
        assert!(
            fixed_messages[1]
                .content
                .iter()
                .any(|c| matches!(c, MessageContentBlock::Thinking(_))),
            "first split message keeps signed thinking"
        );
    }

    #[test]
    fn test_push_coalesces_thinking_deltas() {
        use crate::conversation::message::MessageContentBlock;

        let mut conv = Conversation::empty();
        for fragment in ["I ", "should ", "think ", "about ", "this."] {
            conv.push(
                Message::assistant()
                    .with_thinking(fragment, "")
                    .with_id("turn-1"),
            );
        }

        assert_eq!(conv.messages().len(), 1);
        let content = &conv.messages()[0].content;
        assert_eq!(content.len(), 1);
        match &content[0] {
            MessageContentBlock::Thinking(t) => {
                assert_eq!(t.thinking, "I should think about this.");
                assert_eq!(t.signature, "");
            }
            other => panic!("expected single Thinking block, got {:?}", other),
        }
    }

    #[test]
    fn test_push_merges_empty_output_token_limit_update_by_id() {
        let mut conv = Conversation::empty();
        conv.push(Message::assistant().with_text("first").with_id("turn-1"));

        let inference = InferenceMetadata {
            provider: "test-provider".to_string(),
            requested_model: "test-model".to_string(),
            resolved_model: None,
            provider_session_id: None,
        };
        let mut limited = Message::assistant()
            .with_id("turn-1")
            .with_inference(inference.clone());
        limited.metadata.output_token_limit_reached = true;
        conv.push(limited);

        assert_eq!(conv.messages().len(), 1);
        assert!(conv.messages()[0].metadata.output_token_limit_reached);
        assert_eq!(conv.messages()[0].metadata.inference, Some(inference));
    }

    #[test]
    fn test_push_retains_unmatched_output_token_limit_update_for_user_only() {
        let mut conv = Conversation::empty();
        let mut limited = Message::assistant().with_id("turn-1");
        limited.metadata.output_token_limit_reached = true;

        conv.push(limited);

        assert_eq!(conv.messages().len(), 1);
        let persisted = &conv.messages()[0];
        assert!(persisted.content.is_empty());
        assert!(persisted.metadata.user_visible);
        assert!(!persisted.metadata.agent_visible);
        assert!(persisted.metadata.output_token_limit_reached);
        assert!(conv.agent_visible_messages().is_empty());
    }

    #[test]
    fn test_push_thinking_adopts_signature_on_closing_delta() {
        use crate::conversation::message::MessageContentBlock;

        let mut conv = Conversation::empty();
        // Streamed shape for one signed block: text deltas accumulate while
        // unsigned; the closing delta carries the signature.
        conv.push(
            Message::assistant()
                .with_thinking("a", "")
                .with_id("turn-1"),
        );
        conv.push(
            Message::assistant()
                .with_thinking("b", "sig1")
                .with_id("turn-1"),
        );

        let content = &conv.messages()[0].content;
        assert_eq!(content.len(), 1);
        match &content[0] {
            MessageContentBlock::Thinking(t) => {
                assert_eq!(t.thinking, "ab");
                assert_eq!(t.signature, "sig1");
            }
            other => panic!("expected Thinking, got {:?}", other),
        }
    }

    #[test]
    fn test_push_unsigned_thinking_after_signed_starts_new_block() {
        use crate::conversation::message::MessageContentBlock;

        let mut conv = Conversation::empty();
        conv.push(
            Message::assistant()
                .with_thinking("first body", "sig1")
                .with_id("turn-1"),
        );
        // A second thinking block begins; in signature-at-end streams the
        // first text arrives before the block's signature, so the new
        // unsigned delta must NOT be appended to the already-signed block —
        // otherwise the closing signature later would replay text under
        // the wrong signature.
        conv.push(
            Message::assistant()
                .with_thinking("second body start", "")
                .with_id("turn-1"),
        );

        let content = &conv.messages()[0].content;
        assert_eq!(
            content.len(),
            2,
            "unsigned delta must not merge into a signed block: {:?}",
            content
        );
        match (&content[0], &content[1]) {
            (MessageContentBlock::Thinking(a), MessageContentBlock::Thinking(b)) => {
                assert_eq!(a.thinking, "first body");
                assert_eq!(a.signature, "sig1");
                assert_eq!(b.thinking, "second body start");
                assert_eq!(b.signature, "");
            }
            other => panic!("unexpected content shape: {:?}", other),
        }
    }

    #[test]
    fn test_push_keeps_distinct_signed_thinking_blocks_separate() {
        use crate::conversation::message::MessageContentBlock;

        let mut conv = Conversation::empty();
        conv.push(
            Message::assistant()
                .with_thinking("block A", "sig-A")
                .with_id("turn-1"),
        );
        conv.push(
            Message::assistant()
                .with_thinking("block B", "sig-B")
                .with_id("turn-1"),
        );

        let content = &conv.messages()[0].content;
        assert_eq!(
            content.len(),
            2,
            "two distinct signed blocks must not coalesce: {:?}",
            content
        );
        match (&content[0], &content[1]) {
            (MessageContentBlock::Thinking(a), MessageContentBlock::Thinking(b)) => {
                assert_eq!(a.thinking, "block A");
                assert_eq!(a.signature, "sig-A");
                assert_eq!(b.thinking, "block B");
                assert_eq!(b.signature, "sig-B");
            }
            other => panic!("unexpected content shape: {:?}", other),
        }
    }

    #[test]
    fn test_push_does_not_coalesce_multi_block_thinking_message() {
        use crate::conversation::message::MessageContentBlock;

        let mut conv = Conversation::empty();
        conv.push(
            Message::assistant()
                .with_thinking("first", "")
                .with_id("turn-1"),
        );

        // Multi-block message must NOT coalesce into the existing thinking
        // block — the merge arm requires `message.content.len() == 1`.
        let mut multi = Message::assistant().with_thinking("second", "");
        multi = multi.with_text("and now text").with_id("turn-1");
        conv.push(multi);

        let content = &conv.messages()[0].content;
        assert_eq!(content.len(), 3);
        match (&content[0], &content[1], &content[2]) {
            (
                MessageContentBlock::Thinking(a),
                MessageContentBlock::Thinking(b),
                MessageContentBlock::Text(c),
            ) => {
                assert_eq!(a.thinking, "first");
                assert_eq!(b.thinking, "second");
                assert_eq!(c.text, "and now text");
            }
            other => panic!("unexpected content shape: {:?}", other),
        }
    }
}
