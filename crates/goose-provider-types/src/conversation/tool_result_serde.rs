use crate::conversation::message::ToolResult;
use crate::utils::strip_unicode_tags;
use rmcp::model::{CallToolRequestParams, ErrorCode, ErrorData, JsonObject};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;

pub fn serialize<T, S>(value: &ToolResult<T>, serializer: S) -> Result<S::Ok, S::Error>
where
    T: Serialize,
    S: Serializer,
{
    match value {
        Ok(val) => {
            let mut state = serializer.serialize_struct("ToolResult", 2)?;
            state.serialize_field("status", "success")?;
            state.serialize_field("value", val)?;
            state.end()
        }
        Err(err) => {
            let mut state = serializer.serialize_struct("ToolResult", 2)?;
            state.serialize_field("status", "error")?;
            state.serialize_field("error", &err.to_string())?;
            state.end()
        }
    }
}

#[derive(Deserialize)]
struct ToolCallWithValueArguments {
    name: String,
    arguments: serde_json::Value,
}

impl ToolCallWithValueArguments {
    fn into_call_tool_request_param(self) -> CallToolRequestParams {
        let arguments = match self.arguments {
            serde_json::Value::Object(map) => Some(map),
            serde_json::Value::Null => None,
            other => {
                let mut map = JsonObject::new();
                map.insert("value".to_string(), other);
                Some(map)
            }
        };
        {
            let mut params = CallToolRequestParams::new(self.name);
            if let Some(args) = arguments {
                params = params.with_arguments(args);
            }
            params
        }
    }
}

fn sanitize_json_strings(value: serde_json::Value) -> Result<serde_json::Value, &'static str> {
    match value {
        serde_json::Value::String(value) => {
            Ok(serde_json::Value::String(strip_unicode_tags(&value)))
        }
        serde_json::Value::Array(values) => Ok(serde_json::Value::Array(
            values
                .into_iter()
                .map(sanitize_json_strings)
                .collect::<Result<_, _>>()?,
        )),
        serde_json::Value::Object(values) => {
            let mut sanitized = JsonObject::new();
            for (key, value) in values {
                let key = strip_unicode_tags(&key);
                if sanitized.contains_key(&key) {
                    return Err(
                        "tool arguments contain duplicate key after Unicode tag sanitization",
                    );
                }
                sanitized.insert(key, sanitize_json_strings(value)?);
            }
            Ok(serde_json::Value::Object(sanitized))
        }
        value => Ok(value),
    }
}

fn sanitize_tool_request(
    mut request: CallToolRequestParams,
) -> Result<CallToolRequestParams, &'static str> {
    if let Some(arguments) = request.arguments.take() {
        let serde_json::Value::Object(arguments) =
            sanitize_json_strings(serde_json::Value::Object(arguments))?
        else {
            unreachable!("tool arguments remain an object after sanitization");
        };
        request.arguments = Some(arguments);
    }
    Ok(request)
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<ToolResult<CallToolRequestParams>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ResultFormat {
        SuccessWithCallToolRequestParams {
            status: String,
            value: CallToolRequestParams,
        },
        SuccessWithToolCallValueArguments {
            status: String,
            value: ToolCallWithValueArguments,
        },
        Error {
            status: String,
            error: String,
        },
    }

    let format = ResultFormat::deserialize(deserializer)?;

    match format {
        ResultFormat::SuccessWithCallToolRequestParams { status, value } => {
            if status == "success" {
                sanitize_tool_request(value)
                    .map(Ok)
                    .map_err(serde::de::Error::custom)
            } else {
                Err(serde::de::Error::custom(format!(
                    "Expected status 'success', got '{}'",
                    status
                )))
            }
        }
        ResultFormat::SuccessWithToolCallValueArguments { status, value } => {
            if status == "success" {
                sanitize_tool_request(value.into_call_tool_request_param())
                    .map(Ok)
                    .map_err(serde::de::Error::custom)
            } else {
                Err(serde::de::Error::custom(format!(
                    "Expected status 'success', got '{}'",
                    status
                )))
            }
        }
        ResultFormat::Error { status, error } => {
            if status == "error" {
                Ok(Err(ErrorData {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(error),
                    data: None,
                }))
            } else {
                Err(serde::de::Error::custom(format!(
                    "Expected status 'error', got '{}'",
                    status
                )))
            }
        }
    }
}

pub mod call_tool_result {
    use super::*;
    use crate::conversation::message::sanitize_tool_result_in_place;
    use rmcp::model::{CallToolResult, ContentBlock};

    pub fn serialize<S>(
        value: &ToolResult<CallToolResult>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        super::serialize(value, serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<ToolResult<CallToolResult>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ResultFormat {
            SuccessWithCallToolResult {
                status: String,
                value: CallToolResult,
            },
            SuccessWithContentVec {
                status: String,
                value: Vec<ContentBlock>,
            },
            Error {
                status: String,
                error: String,
            },
        }

        let format = ResultFormat::deserialize(deserializer)?;

        let mut result = match format {
            ResultFormat::SuccessWithCallToolResult { status, value } => {
                if status == "success" {
                    Ok(value)
                } else {
                    return Err(serde::de::Error::custom(format!(
                        "Expected status 'success', got '{}'",
                        status
                    )));
                }
            }
            ResultFormat::SuccessWithContentVec { status, value } => {
                if status == "success" {
                    Ok(CallToolResult::success(value))
                } else {
                    return Err(serde::de::Error::custom(format!(
                        "Expected status 'success', got '{}'",
                        status
                    )));
                }
            }
            ResultFormat::Error { status, error } => {
                if status == "error" {
                    Err(ErrorData {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(error),
                        data: None,
                    })
                } else {
                    return Err(serde::de::Error::custom(format!(
                        "Expected status 'error', got '{}'",
                        status
                    )));
                }
            }
        };

        sanitize_tool_result_in_place(&mut result);
        Ok(result)
    }
}
