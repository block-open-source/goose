use crate::providers::base::Provider;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

// We use double Arc here to allow easy provider swaps while sharing concurrent access
pub type SharedProvider = Arc<Mutex<Option<Arc<dyn Provider>>>>;

/// Default timeout for retry operations (5 minutes)
pub const DEFAULT_RETRY_TIMEOUT_SECONDS: u64 = 300;

/// Default timeout for on_failure operations (10 minutes - longer for on_failure tasks)
pub const DEFAULT_ON_FAILURE_TIMEOUT_SECONDS: u64 = 600;

/// Configuration for retry logic in recipe execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Maximum number of retry attempts before giving up
    pub max_retries: u32,
    /// List of success checks to validate recipe completion
    pub checks: Vec<SuccessCheck>,
    /// Optional shell command to run on failure for cleanup
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<String>,
    /// Timeout in seconds for individual shell commands (default: 300 seconds)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// Timeout in seconds for on_failure commands (default: 600 seconds)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_failure_timeout_seconds: Option<u64>,
}

impl RetryConfig {
    /// Validates the retry configuration values
    pub fn validate(&self) -> Result<(), String> {
        if self.max_retries == 0 {
            return Err("max_retries must be greater than 0".to_string());
        }

        if let Some(timeout) = self.timeout_seconds {
            if timeout == 0 {
                return Err("timeout_seconds must be greater than 0 if specified".to_string());
            }
        }

        if let Some(on_failure_timeout) = self.on_failure_timeout_seconds {
            if on_failure_timeout == 0 {
                return Err(
                    "on_failure_timeout_seconds must be greater than 0 if specified".to_string(),
                );
            }
        }

        Ok(())
    }
}

/// A single success check to validate recipe completion
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SuccessCheck {
    /// Execute a shell command and check its exit status
    #[serde(alias = "shell")]
    Shell {
        /// The shell command to execute
        command: String,
    },
}

/// Session configuration for an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    /// Identifier of the underlying Session
    pub id: String,
    /// ID of the schedule that triggered this session, if any
    pub schedule_id: Option<String>,
    /// Maximum number of turns (iterations) allowed without user input
    pub max_turns: Option<u32>,
    /// Retry configuration for automated validation and recovery
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_config: Option<RetryConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_config_validate_success() {
        let config = RetryConfig {
            max_retries: 3,
            checks: vec![],
            on_failure: None,
            timeout_seconds: Some(60),
            on_failure_timeout_seconds: Some(120),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_retry_config_validate_max_retries_zero() {
        let config = RetryConfig {
            max_retries: 0,
            checks: vec![],
            on_failure: None,
            timeout_seconds: None,
            on_failure_timeout_seconds: None,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "max_retries must be greater than 0");
    }

    #[test]
    fn test_retry_config_validate_timeout_zero() {
        let config = RetryConfig {
            max_retries: 3,
            checks: vec![],
            on_failure: None,
            timeout_seconds: Some(0),
            on_failure_timeout_seconds: None,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "timeout_seconds must be greater than 0 if specified"
        );
    }

    #[test]
    fn test_retry_config_validate_on_failure_timeout_zero() {
        let config = RetryConfig {
            max_retries: 3,
            checks: vec![],
            on_failure: None,
            timeout_seconds: None,
            on_failure_timeout_seconds: Some(0),
        };
        let result = config.validate();
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "on_failure_timeout_seconds must be greater than 0 if specified"
        );
    }
}
