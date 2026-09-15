use super::message::{
    ToolChainSummary, ToolNameParts, ToolRequest, TOOL_META_CHAIN_SUMMARY_KEY,
    TOOL_META_EXTERNAL_DISPATCH_KEY, TOOL_META_PROVIDER_INDEX_KEY, TOOL_META_TITLE_KEY,
};

impl<'a> From<&'a str> for ToolNameParts<'a> {
    fn from(name: &'a str) -> Self {
        match name.split_once("__") {
            Some((extension_name, tool_name)) => Self {
                extension_name: Some(extension_name),
                tool_name,
            },
            None => Self {
                extension_name: None,
                tool_name: name,
            },
        }
    }
}

impl ToolRequest {
    pub fn tool_name_parts(&self) -> Option<ToolNameParts<'_>> {
        let tool_call = self.tool_call.as_ref().ok()?;
        let name = tool_call.name.as_ref();
        let mut parts = ToolNameParts::from(name);
        if let Some(extension_name) = self
            .tool_meta
            .as_ref()
            .and_then(|meta| meta.get("goose_extension"))
            .and_then(serde_json::Value::as_str)
        {
            parts.extension_name = Some(extension_name);
            parts.tool_name = name
                .strip_prefix(extension_name)
                .and_then(|name| name.strip_prefix("__"))
                .unwrap_or(parts.tool_name);
        }
        Some(parts)
    }

    pub fn to_readable_string(&self) -> String {
        match &self.tool_call {
            Ok(tool_call) => {
                format!(
                    "Tool: {}, Args: {}",
                    tool_call.name,
                    serde_json::to_string_pretty(&tool_call.arguments)
                        .unwrap_or_else(|_| "<<invalid json>>".to_string())
                )
            }
            Err(e) => format!("Invalid tool call: {}", e),
        }
    }

    /// Returns true if this tool request was already executed externally
    /// (e.g. by an ACP provider's underlying SDK) and the agent loop must
    /// not redispatch it. See [`TOOL_META_EXTERNAL_DISPATCH_KEY`].
    pub fn was_executed_externally(&self) -> bool {
        self.tool_meta
            .as_ref()
            .and_then(|v| v.get(TOOL_META_EXTERNAL_DISPATCH_KEY))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    pub fn generated_title(&self) -> Option<&str> {
        self.tool_meta
            .as_ref()
            .and_then(|v| v.get(TOOL_META_TITLE_KEY))
            .and_then(|v| v.as_str())
    }

    /// Provider-reported index of this tool call within the streamed response.
    /// See [`TOOL_META_PROVIDER_INDEX_KEY`].
    pub fn provider_index(&self) -> Option<i32> {
        self.tool_meta
            .as_ref()
            .and_then(|v| v.get(TOOL_META_PROVIDER_INDEX_KEY))
            .and_then(|v| v.as_i64())
            .map(|index| index as i32)
    }

    pub fn generated_chain_summary(&self) -> Option<ToolChainSummary> {
        let obj = self
            .tool_meta
            .as_ref()
            .and_then(|v| v.get(TOOL_META_CHAIN_SUMMARY_KEY))?;
        let summary = obj.get("summary").and_then(|v| v.as_str())?.to_string();
        let count = obj.get("count").and_then(|v| v.as_u64())?;
        if count == 0 {
            return None;
        }
        Some(ToolChainSummary {
            summary,
            count: count as usize,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::CallToolRequestParams;

    fn make_tool_request(meta: Option<serde_json::Value>) -> ToolRequest {
        ToolRequest {
            id: "id-1".to_string(),
            tool_call: Ok(CallToolRequestParams::new("test_tool")),
            metadata: None,
            tool_meta: meta,
        }
    }

    mod tool_name_parts {
        use super::*;
        use rmcp::model::ErrorData;

        fn make_named_tool_request(name: &str) -> ToolRequest {
            ToolRequest {
                id: "id-1".to_string(),
                tool_call: Ok(CallToolRequestParams::new(name.to_string())),
                metadata: None,
                tool_meta: None,
            }
        }

        #[test]
        fn splits_prefixed_name() {
            let request = make_named_tool_request("developer__shell");

            assert_eq!(
                request.tool_name_parts(),
                Some(ToolNameParts {
                    extension_name: Some("developer"),
                    tool_name: "shell",
                })
            );
        }

        #[test]
        fn preserves_unprefixed_name() {
            let request = make_named_tool_request("read");

            assert_eq!(
                request.tool_name_parts(),
                Some(ToolNameParts {
                    extension_name: None,
                    tool_name: "read",
                })
            );
        }

        #[test]
        fn resolves_unprefixed_extension_from_metadata() {
            let mut request = make_named_tool_request("write");
            request.tool_meta = Some(serde_json::json!({"goose_extension": "developer"}));

            assert_eq!(
                request.tool_name_parts(),
                Some(ToolNameParts {
                    extension_name: Some("developer"),
                    tool_name: "write",
                })
            );
        }

        #[test]
        fn strips_exact_extension_prefix_from_metadata() {
            let mut request = make_named_tool_request("__cli__ent____tool");
            request.tool_meta = Some(serde_json::json!({"goose_extension": "__cli__ent__"}));

            assert_eq!(
                request.tool_name_parts(),
                Some(ToolNameParts {
                    extension_name: Some("__cli__ent__"),
                    tool_name: "tool",
                })
            );
        }

        #[test]
        fn splits_at_first_separator() {
            let request = make_named_tool_request("calendar__events__list");

            assert_eq!(
                request.tool_name_parts(),
                Some(ToolNameParts {
                    extension_name: Some("calendar"),
                    tool_name: "events__list",
                })
            );
        }

        #[test]
        fn returns_none_for_invalid_call() {
            let request = ToolRequest {
                id: "id-1".to_string(),
                tool_call: Err(ErrorData::invalid_request("invalid tool call", None)),
                metadata: None,
                tool_meta: None,
            };

            assert_eq!(request.tool_name_parts(), None);
        }
    }

    mod generated_title {
        use super::*;

        #[test]
        fn returns_none_when_meta_missing() {
            let req = make_tool_request(None);
            assert_eq!(req.generated_title(), None);
        }

        #[test]
        fn returns_value_when_present() {
            let meta = serde_json::json!({
                TOOL_META_TITLE_KEY: "reading project configuration",
            });
            let req = make_tool_request(Some(meta));
            assert_eq!(req.generated_title(), Some("reading project configuration"));
        }

        #[test]
        fn returns_none_for_non_string_value() {
            let meta = serde_json::json!({ TOOL_META_TITLE_KEY: 42 });
            let req = make_tool_request(Some(meta));
            assert_eq!(req.generated_title(), None);
        }

        #[test]
        fn does_not_collide_with_external_dispatch() {
            let meta = serde_json::json!({
                TOOL_META_EXTERNAL_DISPATCH_KEY: true,
                TOOL_META_TITLE_KEY: "running commands",
            });
            let req = make_tool_request(Some(meta));
            assert!(req.was_executed_externally());
            assert_eq!(req.generated_title(), Some("running commands"));
        }
    }

    mod generated_chain_summary {
        use super::*;

        #[test]
        fn round_trips() {
            let meta = serde_json::json!({
                TOOL_META_CHAIN_SUMMARY_KEY: {
                    "summary": "applied dark mode polish",
                    "count": 4,
                },
            });
            let req = make_tool_request(Some(meta));
            let summary = req.generated_chain_summary().expect("summary present");
            assert_eq!(summary.summary, "applied dark mode polish");
            assert_eq!(summary.count, 4);
        }

        #[test]
        fn returns_none_for_missing_or_zero_count() {
            let req = make_tool_request(None);
            assert!(req.generated_chain_summary().is_none());

            let meta_zero = serde_json::json!({
                TOOL_META_CHAIN_SUMMARY_KEY: { "summary": "x", "count": 0 },
            });
            let req_zero = make_tool_request(Some(meta_zero));
            assert!(req_zero.generated_chain_summary().is_none());

            let meta_no_summary = serde_json::json!({
                TOOL_META_CHAIN_SUMMARY_KEY: { "count": 3 },
            });
            let req_no_summary = make_tool_request(Some(meta_no_summary));
            assert!(req_no_summary.generated_chain_summary().is_none());
        }
    }
}
