//! Runs an ordered, re-entrant pipeline over persisted conversation state.
//!
//! Callers persist incoming messages, construct `Step`s from their own operations,
//! and choose whether to call `StateMachine::step`, `StateMachine::apply`, or
//! `StateMachine::run`. Goose's concrete operations remain internal because their
//! configuration is part of `Agent::reply`, not the state-machine protocol.

mod effects;
mod inference_preparation;
mod ops_bang_shell;
mod ops_compaction;
mod ops_doctor;
mod ops_entry_hook;
mod ops_exit_on_error;
mod ops_llm;
mod ops_maxturns;
mod ops_project;
mod ops_recipe;
mod ops_retry;
mod ops_skills;
mod ops_slash_command;
mod ops_status;
mod ops_steer;
mod ops_stop_hook;
mod ops_tool_approval;
mod ops_tool_pair_compaction;
mod ops_toolcalling;
mod ops_unknown_tool;
mod session;
pub(crate) use session::run as run_goose;
mod tool_confirmation;
mod usage;

#[cfg(test)]
mod tests;

pub use effects::GooseEffect;
pub use goose_agent::machine::{
    EffectHandler, EffectUsage, MachineSession, SessionLoader, StateMachine, Step,
};
pub use goose_agent::operation::{
    applied, assistant_turn_count, ends_turn, last_effective_role, messages_since_kickoff,
    not_applicable, trailing_error, yielded, yielded_with, ConversationEffect, Emitter, Inference,
    InferenceInput, MachineEffect, Operation, OperationResult, SlashCommand, StepResult,
};
pub(crate) use tool_confirmation::{
    has_unapplied_tool_confirmation_response, pending_tool_confirmations,
    persist_tool_confirmation_decision,
};

pub(super) use inference_preparation::GooseInferenceRequestPreparer;
pub(super) use ops_bang_shell::BangShellOperation;
pub(super) use ops_compaction::CompactionOperation;
pub(super) use ops_doctor::DoctorOperation;
pub(super) use ops_entry_hook::EntryHookOperation;
pub(super) use ops_exit_on_error::ExitOnErrorOperation;
pub(super) use ops_llm::{GooseInferenceProvider, InferenceRunner};
pub(super) use ops_maxturns::{MaxTurnsOperation, MAX_TURNS_MESSAGE};
pub(super) use ops_project::ProjectOperation;
pub(super) use ops_recipe::RecipeOperation;
pub(super) use ops_retry::RetryOperation;
pub(super) use ops_skills::SkillOperation;
pub(super) use ops_slash_command::SlashCommandOperation;
pub(super) use ops_status::StatusOperation;
pub(super) use ops_steer::{SteerOperation, SteerQueue};
pub(super) use ops_stop_hook::StopHookOperation;
pub(super) use ops_tool_approval::ToolApprovalOperation;
pub(super) use ops_tool_pair_compaction::ToolPairCompactionOperation;
pub(super) use ops_toolcalling::ToolExecutionOperation;
pub(super) use ops_unknown_tool::UnknownToolOperation;

pub fn enabled() -> bool {
    std::env::var("GOOSE_STATE_MACHINE")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
        .unwrap_or(false)
}
