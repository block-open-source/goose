use base64::{engine::general_purpose::STANDARD, Engine};

use crate::session::session_manager::Session;

const TEMPLATE_HTML: &str = include_str!("template.html");
const TEMPLATE_CSS: &str = include_str!("template.css");
const TEMPLATE_JS: &str = include_str!("template.js");
const MARKED_JS: &str = include_str!("vendor/marked.min.js");
const HIGHLIGHT_JS: &str = include_str!("vendor/highlight.min.js");

/// Render a session to a self-contained HTML file.
///
/// The session is serialized to JSON, base64-encoded, and embedded in the
/// template. Rendering happens client-side in the browser using the bundled
/// `template.js`, `marked` (markdown), and `highlight.js` (syntax highlighting).
pub fn export_session_to_html(session: &Session) -> anyhow::Result<String> {
    let session_json = serde_json::to_string(session)?;
    let session_base64 = STANDARD.encode(session_json.as_bytes());

    let html = TEMPLATE_HTML
        .replace("{{CSS}}", TEMPLATE_CSS)
        .replace("{{MARKED_JS}}", MARKED_JS)
        .replace("{{HIGHLIGHT_JS}}", HIGHLIGHT_JS)
        .replace("{{JS}}", TEMPLATE_JS)
        .replace("{{SESSION_DATA}}", &session_base64);

    Ok(html)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::message::Message;
    use crate::conversation::Conversation;
    use crate::session::session_manager::SessionType;
    use base64::Engine;
    use chrono::Utc;
    use std::path::PathBuf;

    fn sample_session() -> Session {
        let conversation = Conversation::new_unvalidated(vec![
            Message::user().with_text("hello"),
            Message::assistant().with_text("hi there"),
        ]);
        Session {
            id: "20260910_1".to_string(),
            working_dir: PathBuf::from("/tmp/proj"),
            name: "Greeting".to_string(),
            user_set_name: false,
            session_type: SessionType::User,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            extension_data: Default::default(),
            usage: Default::default(),
            accumulated_usage: Default::default(),
            accumulated_cost: Some(0.01),
            schedule_id: None,
            recipe: None,
            user_recipe_values: None,
            conversation: Some(conversation),
            message_count: 2,
            last_message_at: None,
            provider_name: Some("databricks".to_string()),
            model_config: None,
            goose_mode: Default::default(),
            archived_at: None,
            project_id: None,
            parent_session_id: None,
            last_message_snippet: None,
        }
    }

    #[test]
    fn embeds_session_json_as_base64() {
        let session = sample_session();
        let html = export_session_to_html(&session).unwrap();

        assert!(html.contains("<script id=\"session-data\""));
        assert!(!html.contains("{{SESSION_DATA}}"));
        assert!(!html.contains("{{CSS}}"));
        assert!(!html.contains("{{JS}}"));

        let start = html.find("application/json\">").unwrap() + "application/json\">".len();
        let end = html[start..].find("</script>").unwrap() + start;
        let decoded = STANDARD.decode(html[start..end].as_bytes()).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(value["id"], "20260910_1");
        assert_eq!(value["conversation"].as_array().unwrap().len(), 2);
    }
}
