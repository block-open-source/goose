//! Last routed request header per session, recorded so the compaction call
//! can replay the same cacheable prefix.

use rmcp::model::Tool;
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

/// Evicted (or never-recorded) sessions summarize without a header.
const MAX_SESSIONS: usize = 64;

#[derive(Clone, Default)]
pub struct RequestHeader {
    pub system_prompt: String,
    pub tools: Vec<Tool>,
    /// Real tool definitions when toolshim is active (`tools` is empty then):
    /// the summarizer's response must be interpreted the same way as the
    /// session's own responses.
    pub toolshim_tools: Vec<Tool>,
}

#[derive(Default)]
struct Registry {
    headers: HashMap<String, RequestHeader>,
    insertion_order: VecDeque<String>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

pub fn record(session_id: &str, header: RequestHeader) {
    let mut registry = registry().lock().unwrap();
    if !registry.headers.contains_key(session_id) {
        while registry.insertion_order.len() >= MAX_SESSIONS {
            if let Some(oldest) = registry.insertion_order.pop_front() {
                registry.headers.remove(&oldest);
            }
        }
        registry.insertion_order.push_back(session_id.to_string());
    }
    registry.headers.insert(session_id.to_string(), header);
}

pub fn last_for_session(session_id: &str) -> Option<RequestHeader> {
    registry().lock().unwrap().headers.get(session_id).cloned()
}
