use agent_client_protocol::schema::v1::SessionInfo;
use agent_client_protocol::{JsonRpcRequest, JsonRpcResponse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{EmptyResponse, RecipeDto};

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledJobDto {
    pub id: String,
    pub source: String,
    pub cron: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
    pub currently_running: bool,
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_start_time: Option<String>,
}

/// List scheduled recipe jobs.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/schedules/list",
    response = ListSchedulesResponse
)]
pub struct ListSchedulesRequest {}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct ListSchedulesResponse {
    pub jobs: Vec<ScheduledJobDto>,
}

/// Create a scheduled recipe job.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/schedules/create",
    response = CreateScheduleResponse
)]
pub struct CreateScheduleRequest {
    pub id: String,
    pub recipe: RecipeDto,
    pub cron: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct CreateScheduleResponse {
    pub job: ScheduledJobDto,
}

/// Delete a scheduled recipe job.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/schedules/delete", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct DeleteScheduleRequest {
    pub schedule_id: String,
}

/// Update the cron expression for a scheduled recipe job.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/schedules/update",
    response = UpdateScheduleResponse
)]
#[serde(rename_all = "camelCase")]
pub struct UpdateScheduleRequest {
    pub schedule_id: String,
    pub cron: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct UpdateScheduleResponse {
    pub job: ScheduledJobDto,
}

/// Run a scheduled recipe job immediately.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/schedules/run-now",
    response = RunScheduleNowResponse
)]
#[serde(rename_all = "camelCase")]
pub struct RunScheduleNowRequest {
    pub schedule_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct RunScheduleNowResponse {
    pub status: RunScheduleNowStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RunScheduleNowStatus {
    #[default]
    Completed,
    Cancelled,
}

/// List recent sessions created by a scheduled recipe job.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/schedules/sessions/list",
    response = ListScheduleSessionsResponse
)]
#[serde(rename_all = "camelCase")]
pub struct ListScheduleSessionsRequest {
    pub schedule_id: String,
    pub limit: usize,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct ListScheduleSessionsResponse {
    pub sessions: Vec<SessionInfo>,
}

/// Pause a scheduled recipe job.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/schedules/pause", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct PauseScheduleRequest {
    pub schedule_id: String,
}

/// Resume a paused scheduled recipe job.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/schedules/unpause",
    response = EmptyResponse
)]
#[serde(rename_all = "camelCase")]
pub struct UnpauseScheduleRequest {
    pub schedule_id: String,
}

/// Stop a currently running scheduled job.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/schedules/running-job/kill",
    response = KillRunningJobResponse
)]
#[serde(rename_all = "camelCase")]
pub struct KillRunningJobRequest {
    pub job_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct KillRunningJobResponse {
    pub message: String,
}

/// Inspect the current state of a running scheduled job.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/schedules/running-job/inspect",
    response = InspectRunningJobResponse
)]
#[serde(rename_all = "camelCase")]
pub struct InspectRunningJobRequest {
    pub job_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct InspectRunningJobResponse {
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_start_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_duration_seconds: Option<i64>,
}
