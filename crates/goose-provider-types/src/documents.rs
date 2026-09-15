use serde_json::{json, Value};

use crate::conversation::message::DocumentContent;

#[derive(Debug, Copy, Clone)]
pub enum DocumentFormat {
    OpenAi,
    Anthropic,
}

/// Media types that providers accept as native document input. Anything else is
/// reported back to the caller rather than being sent as an unusable blob.
pub const SUPPORTED_DOCUMENT_MEDIA_TYPES: [&str; 1] = ["application/pdf"];

pub fn document_media_type_is_supported(mime_type: &str) -> bool {
    SUPPORTED_DOCUMENT_MEDIA_TYPES.contains(&mime_type)
}

pub fn convert_document(document: &DocumentContent, format: &DocumentFormat) -> Value {
    match format {
        DocumentFormat::OpenAi => json!({
            "type": "file",
            "file": {
                "filename": document.name.clone().unwrap_or_else(|| "document.pdf".to_string()),
                "file_data": format!("data:{};base64,{}", document.mime_type, document.data),
            }
        }),
        DocumentFormat::Anthropic => {
            let mut block = json!({
                "type": "document",
                "source": {
                    "type": "base64",
                    "media_type": document.mime_type,
                    "data": document.data,
                }
            });
            if let Some(name) = &document.name {
                block["title"] = json!(name);
            }
            block
        }
    }
}

/// Explains why a document was dropped so the model, and the caller reading the
/// request, can act on it instead of silently losing the attachment.
pub fn unsupported_document_text(document: &DocumentContent, reason: &str) -> String {
    match &document.name {
        Some(name) => format!(
            "[document \"{}\" ({}) not sent: {}]",
            name, document.mime_type, reason
        ),
        None => format!("[document ({}) not sent: {}]", document.mime_type, reason),
    }
}

pub const UNSUPPORTED_MEDIA_TYPE_REASON: &str =
    "only application/pdf documents can be sent to this provider";
pub const UNSUPPORTED_PROVIDER_REASON: &str = "this provider does not accept document input";
pub const ASSISTANT_ROLE_REASON: &str = "documents can only be sent in user messages";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_document_text_names_the_document_and_reason() {
        let named = DocumentContent::new("data", "text/csv").with_name("rows.csv");
        assert_eq!(
            unsupported_document_text(&named, UNSUPPORTED_MEDIA_TYPE_REASON),
            "[document \"rows.csv\" (text/csv) not sent: only application/pdf documents can be sent to this provider]"
        );

        let unnamed = DocumentContent::new("data", "text/csv");
        assert_eq!(
            unsupported_document_text(&unnamed, UNSUPPORTED_PROVIDER_REASON),
            "[document (text/csv) not sent: this provider does not accept document input]"
        );
    }
}
