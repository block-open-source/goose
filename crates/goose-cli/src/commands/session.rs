use anyhow::{Context, Result};

use cliclack::{confirm, multiselect, select};
use etcetera::home_dir;
#[cfg(feature = "nostr")]
use goose::config::Config;
#[cfg(feature = "nostr")]
use goose::session::nostr_share;
use goose::session::{
    export_session_to_markdown, generate_diagnostics, DiagnosticsLevel, Session, SessionManager,
    SessionType,
};
use goose::utils::safe_truncate;
use regex::Regex;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::path::PathBuf;

const TRUNCATED_DESC_LENGTH: usize = 60;

fn display_path_with_tilde(path: &Path) -> String {
    #[cfg(not(target_os = "windows"))]
    if let Ok(home) = home_dir() {
        if let Ok(stripped) = path.strip_prefix(&home) {
            return format!("~/{}", stripped.display());
        }
    }
    path.display().to_string()
}

async fn remove_sessions(session_manager: &SessionManager, sessions: Vec<Session>) -> Result<()> {
    println!("The following sessions will be removed:");
    for session in &sessions {
        println!("- {} {}", session.id, session.name);
    }

    let should_delete = confirm("Are you sure you want to delete these sessions?")
        .initial_value(false)
        .interact()?;

    if should_delete {
        for session in sessions {
            session_manager.delete_session(&session.id).await?;
            println!("Session `{}` removed.", session.id);
        }
    } else {
        println!("Skipping deletion of the sessions.");
    }

    Ok(())
}

fn prompt_interactive_session_removal(sessions: &[Session]) -> Result<Vec<Session>> {
    if sessions.is_empty() {
        println!("No sessions to delete.");
        return Ok(vec![]);
    }

    let mut selector = multiselect(
        "Select sessions to delete (use spacebar, Enter to confirm, Ctrl+C to cancel):",
    );

    let display_map: std::collections::HashMap<String, Session> = sessions
        .iter()
        .map(|s| {
            let desc = if s.name.is_empty() {
                "(no name)"
            } else {
                &s.name
            };
            let truncated_desc = safe_truncate(desc, TRUNCATED_DESC_LENGTH);
            let display_text =
                format!("{} - {} ({})", session_activity_at(s), truncated_desc, s.id);
            (display_text, s.clone())
        })
        .collect();

    for display_text in display_map.keys() {
        selector = selector.item(display_text.clone(), display_text.clone(), "");
    }

    let selected_display_texts: Vec<String> = selector.interact()?;

    let selected_sessions: Vec<Session> = selected_display_texts
        .into_iter()
        .filter_map(|text| display_map.get(&text).cloned())
        .collect();

    Ok(selected_sessions)
}

pub async fn handle_session_remove(
    session_id: Option<String>,
    name: Option<String>,
    regex_string: Option<String>,
) -> Result<()> {
    let session_manager = SessionManager::instance();

    let matched_sessions: Vec<Session>;

    if let Some(id_val) = session_id {
        match session_manager.get_session(&id_val, false).await {
            Ok(session) => matched_sessions = vec![session],
            Err(_) => return Err(anyhow::anyhow!("Session ID '{}' not found.", id_val)),
        }
    } else if let Some(name_val) = name {
        let all_sessions = session_manager.list_all_sessions().await?;
        if let Some(session) = all_sessions.into_iter().find(|s| s.name == name_val) {
            matched_sessions = vec![session];
        } else {
            return Err(anyhow::anyhow!(
                "Session with name '{}' not found.",
                name_val
            ));
        }
    } else if let Some(regex_val) = regex_string {
        let session_regex = Regex::new(&regex_val)
            .with_context(|| format!("Invalid regex pattern '{}'", regex_val))?;

        let visible_sessions = session_manager.list_sessions().await?;
        matched_sessions = visible_sessions
            .into_iter()
            .filter(|session| session_regex.is_match(&session.id))
            .collect();

        if matched_sessions.is_empty() {
            println!("Regex string '{}' does not match any sessions", regex_val);
            return Ok(());
        }
    } else {
        let visible_sessions = session_manager.list_sessions().await?;
        if visible_sessions.is_empty() {
            return Err(anyhow::anyhow!("No sessions found."));
        }
        matched_sessions = prompt_interactive_session_removal(&visible_sessions)?;
    }

    if matched_sessions.is_empty() {
        return Ok(());
    }

    remove_sessions(&session_manager, matched_sessions).await
}

fn write_line_or_broken_pipe_ok<W: Write>(out: &mut W, line: &str) -> Result<bool> {
    match writeln!(out, "{line}") {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn session_activity_at(session: &Session) -> chrono::DateTime<chrono::Utc> {
    session.last_message_at.unwrap_or(session.updated_at)
}

fn session_list_limit_if_safe_to_push_down(
    ascending: bool,
    working_dir: Option<&Path>,
    limit: Option<usize>,
) -> Option<usize> {
    if !ascending && working_dir.is_none() {
        limit
    } else {
        None
    }
}

pub async fn handle_session_list(
    format: String,
    ascending: bool,
    working_dir: Option<PathBuf>,
    limit: Option<usize>,
) -> Result<()> {
    let session_manager = SessionManager::instance();
    let mut sessions = if let Some(limit) =
        session_list_limit_if_safe_to_push_down(ascending, working_dir.as_deref(), limit)
    {
        session_manager.list_sessions_with_limit(limit).await?
    } else {
        session_manager.list_sessions().await?
    };

    if let Some(ref pat) = working_dir {
        let pat_lower = pat.to_string_lossy().to_lowercase();
        sessions.retain(|s| {
            s.working_dir
                .to_string_lossy()
                .to_lowercase()
                .contains(&pat_lower)
        });
    }

    if ascending {
        sessions.sort_by_key(session_activity_at);
    } else {
        sessions.sort_by_key(|b| std::cmp::Reverse(session_activity_at(b)));
    }

    if let Some(n) = limit {
        sessions.truncate(n);
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();

    match format.as_str() {
        "json" => {
            let payload = serde_json::to_string(&sessions)?;
            if !write_line_or_broken_pipe_ok(&mut out, &payload)? {
                return Ok(());
            }
        }
        _ => {
            if sessions.is_empty() {
                if !write_line_or_broken_pipe_ok(&mut out, "No sessions found")? {
                    return Ok(());
                }
                return Ok(());
            }

            if !write_line_or_broken_pipe_ok(&mut out, "Available sessions:")? {
                return Ok(());
            }

            for session in sessions {
                let output = format!(
                    "{} - {} - {} - {}",
                    session.id,
                    session.name,
                    session_activity_at(&session),
                    display_path_with_tilde(&session.working_dir)
                );
                if !write_line_or_broken_pipe_ok(&mut out, &output)? {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod session_list_tests {
    use super::*;

    #[test]
    fn pushes_down_only_safe_session_list_limits() {
        assert_eq!(
            session_list_limit_if_safe_to_push_down(false, None, Some(50)),
            Some(50)
        );
        assert_eq!(
            session_list_limit_if_safe_to_push_down(true, None, Some(50)),
            None
        );
        assert_eq!(
            session_list_limit_if_safe_to_push_down(false, Some(Path::new("/tmp")), Some(50)),
            None
        );
        assert_eq!(
            session_list_limit_if_safe_to_push_down(false, None, None),
            None
        );
    }
}

pub async fn handle_session_export(
    session_id: String,
    output_path: Option<PathBuf>,
    format: String,
    nostr: bool,
    #[cfg_attr(not(feature = "nostr"), allow(unused_variables))] relays: Vec<String>,
) -> Result<()> {
    let session_manager = SessionManager::instance();
    let session = match session_manager.get_session(&session_id, true).await {
        Ok(session) => session,
        Err(e) => {
            return Err(anyhow::anyhow!(
                "Session '{}' not found or failed to read: {}",
                session_id,
                e
            ));
        }
    };

    let output = match format.as_str() {
        "json" => serde_json::to_string_pretty(&session)?,
        "yaml" => serde_yaml::to_string(&session)?,
        "markdown" => {
            let conversation = session
                .conversation
                .ok_or_else(|| anyhow::anyhow!("Session has no messages"))?;
            export_session_to_markdown(conversation.user_visible_messages(), &session.name)
        }
        _ => return Err(anyhow::anyhow!("Unsupported format: {}", format)),
    };

    #[cfg(feature = "nostr")]
    if nostr {
        if format != "json" {
            return Err(anyhow::anyhow!(
                "Nostr session sharing only supports --format json"
            ));
        }
        if output_path.is_some() {
            return Err(anyhow::anyhow!(
                "Nostr session sharing cannot be combined with --output"
            ));
        }

        let relays = nostr_share::resolve_relays(relays, Config::global());
        let share = nostr_share::publish_session_json(&output, relays).await?;
        println!("Session published to Nostr relays:");
        for relay in &share.relays {
            println!("- {}", relay);
        }
        println!("\nShare link:");
        println!("{}", share.deeplink);
        return Ok(());
    }
    #[cfg(not(feature = "nostr"))]
    if nostr {
        return Err(anyhow::anyhow!("goose was not built with nostr support"));
    }

    if let Some(output_path) = output_path {
        fs::write(&output_path, output).with_context(|| {
            format!("Failed to write to output file: {}", output_path.display())
        })?;
        println!("Session exported to {}", output_path.display());
    } else {
        println!("{}", output);
    }

    Ok(())
}

pub async fn handle_session_import(input: String, nostr: bool) -> Result<()> {
    let json = if nostr || input.starts_with("goose://sessions/nostr") {
        #[cfg(feature = "nostr")]
        {
            nostr_share::import_session_json_from_deeplink(&input).await?
        }
        #[cfg(not(feature = "nostr"))]
        return Err(anyhow::anyhow!("goose was not built with nostr support"));
    } else {
        fs::read_to_string(&input)
            .with_context(|| format!("Failed to read session import file: {input}"))?
    };

    let format = goose::session::import_formats::detect_format(&json);
    let label = match format {
        goose::session::import_formats::ImportFormat::Goose => "goose",
        goose::session::import_formats::ImportFormat::ClaudeCode => "Claude Code",
        goose::session::import_formats::ImportFormat::Codex => "Codex",
        goose::session::import_formats::ImportFormat::Pi => "Pi",
    };
    println!("Detected format: {}", label);

    let session_manager = SessionManager::instance();
    let session = session_manager
        .import_session(&json, Some(SessionType::User))
        .await?;

    println!("Session imported:");
    println!("{} - {}", session.id, session.name);

    Ok(())
}

#[cfg(unix)]
fn open_diagnostics_output(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(windows)]
fn open_diagnostics_output(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use winapi::um::winbase::FILE_FLAG_OPEN_REPARSE_POINT;
    use winapi::um::winnt::FILE_ATTRIBUTE_REPARSE_POINT;

    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "diagnostics output must be a regular file",
        ));
    }
    file.set_len(0)?;
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
fn open_diagnostics_output(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

pub async fn handle_diagnostics(session_id: &str, output_path: Option<PathBuf>) -> Result<()> {
    println!(
        "Generating diagnostics report for session '{}'...",
        session_id
    );

    let session_manager = SessionManager::instance();
    let diagnostics_report =
        generate_diagnostics(&session_manager, session_id, DiagnosticsLevel::Full)
            .await
            .with_context(|| {
                format!(
                    "Failed to generate diagnostics report for session '{}'",
                    session_id
                )
            })?;
    let diagnostics_data = serde_json::to_vec_pretty(&diagnostics_report)
        .context("Failed to serialize diagnostics report")?;

    let output_file = if let Some(path) = output_path {
        path.clone()
    } else {
        PathBuf::from(format!("diagnostics_{}.json", session_id))
    };

    let mut file = open_diagnostics_output(&output_file).context(format!(
        "Failed to create output file: {}",
        output_file.display()
    ))?;

    file.write_all(&diagnostics_data)
        .context("Failed to write diagnostics data")?;

    println!("Diagnostics report saved to: {}", output_file.display());

    Ok(())
}

/// Prompt the user to interactively select a session
///
/// Shows a list of available sessions and lets the user select one
pub async fn prompt_interactive_session_selection(
    session_manager: &SessionManager,
) -> Result<String> {
    let sessions = session_manager.list_sessions().await?;

    if sessions.is_empty() {
        return Err(anyhow::anyhow!("No sessions found"));
    }

    // Build the selection prompt
    let mut selector = select("Select a session to export:");

    // Map to display text
    let display_map: std::collections::HashMap<String, Session> = sessions
        .iter()
        .map(|s| {
            let desc = if s.name.is_empty() {
                "(no name)"
            } else {
                &s.name
            };
            let truncated_desc = safe_truncate(desc, TRUNCATED_DESC_LENGTH);

            let display_text = format!("{} - {} ({})", s.updated_at, truncated_desc, s.id);
            (display_text, s.clone())
        })
        .collect();

    // Add each session as an option
    for display_text in display_map.keys() {
        selector = selector.item(display_text.clone(), display_text.clone(), "");
    }

    // Add a cancel option
    let cancel_value = String::from("cancel");
    selector = selector.item(cancel_value, "Cancel", "Cancel export");

    // Get user selection
    let selected_display_text: String = selector.interact()?;

    if selected_display_text == "cancel" {
        return Err(anyhow::anyhow!("Export canceled"));
    }

    // Retrieve the selected session
    if let Some(session) = display_map.get(&selected_display_text) {
        Ok(session.id.clone())
    } else {
        Err(anyhow::anyhow!("Invalid selection"))
    }
}

#[cfg(test)]
mod diagnostics_output_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn creates_new_output_file() {
        let temp_dir = TempDir::new().unwrap();
        let output = temp_dir.path().join("diagnostics.json");

        let mut file = open_diagnostics_output(&output).unwrap();
        file.write_all(b"diagnostics").unwrap();
        drop(file);

        assert_eq!(fs::read(&output).unwrap(), b"diagnostics");
    }

    #[test]
    fn truncates_existing_regular_output_file() {
        let temp_dir = TempDir::new().unwrap();
        let output = temp_dir.path().join("diagnostics.json");
        fs::write(&output, "old diagnostics").unwrap();

        let mut file = open_diagnostics_output(&output).unwrap();
        file.write_all(b"new").unwrap();
        drop(file);

        assert_eq!(fs::read(&output).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_existing_symlink_output() {
        use std::os::unix::fs::symlink;

        let temp_dir = TempDir::new().unwrap();
        let target = temp_dir.path().join("target.json");
        let output = temp_dir.path().join("diagnostics.json");
        fs::write(&target, "preserve").unwrap();
        symlink(&target, &output).unwrap();

        assert!(open_diagnostics_output(&output).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "preserve");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_dangling_symlink_output() {
        use std::os::unix::fs::symlink;

        let temp_dir = TempDir::new().unwrap();
        let target = temp_dir.path().join("missing.json");
        let output = temp_dir.path().join("diagnostics.json");
        symlink(&target, &output).unwrap();

        assert!(open_diagnostics_output(&output).is_err());
        assert!(!target.exists());
    }

    #[cfg(windows)]
    #[test]
    fn refuses_existing_symlink_output() {
        use std::os::windows::fs::symlink_file;

        let temp_dir = TempDir::new().unwrap();
        let target = temp_dir.path().join("target.json");
        let output = temp_dir.path().join("diagnostics.json");
        fs::write(&target, "preserve").unwrap();
        if symlink_file(&target, &output).is_err() {
            return;
        }

        assert!(open_diagnostics_output(&output).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "preserve");
    }
}
