//! Windows multi-line paste capture for the interactive prompt.
//!
//! rustyline handles pastes natively via bracketed paste off Windows, but the
//! Windows console has no such mechanism, so a pasted block arrives as an
//! ordinary key-event burst. This module detects that burst, drains it from the
//! console buffer, and collapses it into a `[Pasted N lines]` chip that
//! [`expand_pastes`] restores on submit. The seam exposed to `session::input`
//! is deliberately narrow: [`PasteState`], the two handlers, and
//! [`read_paste_aware_input`]; everything else stays private.

use super::completion::GooseCompleter;
use rustyline::Editor;
use std::sync::Arc;

/// Minimum number of events already queued in the console input buffer for a
/// keystroke to be treated as the start of a paste rather than fast typing /
/// type-ahead. A single keypress leaves at most its own key-up event queued.
const PASTE_QUEUE_THRESHOLD: u32 = 2;
/// Collapse a paste into a chip once it spans at least this many lines.
const PASTE_CHIP_MIN_LINES: usize = 2;
/// ...or once a single-line paste is at least this many characters.
const PASTE_CHIP_MIN_CHARS: usize = 400;

struct Paste {
    marker: String,
    content: String,
}

#[derive(Default)]
pub(super) struct PasteState {
    pastes: Vec<Paste>,
    next_id: usize,
}

/// Number of events still queued in the console input buffer, i.e. keystrokes
/// the terminal has delivered but rustyline has not yet read. During a paste the
/// whole payload is queued at once, so a non-empty queue right after a keystroke
/// reliably distinguishes pasted input from real typing. Always 0 off Windows,
/// where rustyline handles pastes natively via bracketed paste.
#[cfg(windows)]
fn console_pending_events() -> u32 {
    use winapi::um::consoleapi::GetNumberOfConsoleInputEvents;
    use winapi::um::processenv::GetStdHandle;
    use winapi::um::winbase::STD_INPUT_HANDLE;

    unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        let mut count: u32 = 0;
        if GetNumberOfConsoleInputEvents(handle, &mut count) != 0 {
            count
        } else {
            0
        }
    }
}

#[cfg(not(windows))]
fn console_pending_events() -> u32 {
    0
}

/// The Unicode code unit carried by a console key event, or `None` if the record
/// carries no character. Characters normally arrive on key-down, but Unicode the
/// active keyboard layout cannot synthesize — emoji, supplementary-plane chars —
/// is injected by Windows as an Alt+numpad sequence whose composed character
/// lands on the Alt (`VK_MENU`) key-up. Accepting that one key-up, and no other,
/// mirrors rustyline's own console read loop and avoids double-counting the
/// key-up of ordinary typed characters.
#[cfg(windows)]
fn key_event_char(record: &winapi::um::wincon::INPUT_RECORD) -> Option<u16> {
    use winapi::um::wincon::KEY_EVENT;
    const VK_MENU: u16 = 0x12;

    if record.EventType != KEY_EVENT {
        return None;
    }
    let key = unsafe { record.Event.KeyEvent() };
    let ch = unsafe { *key.uChar.UnicodeChar() };
    if ch != 0 && (key.bKeyDown != 0 || key.wVirtualKeyCode == VK_MENU) {
        Some(ch)
    } else {
        None
    }
}

/// Drain the remainder of a paste burst directly from the console input buffer,
/// starting from `first` (the character that triggered detection). rustyline
/// reads events one at a time, so the rest of the payload is still queued here;
/// consuming it ourselves keeps it from being echoed line by line and lets us
/// finalize the chip in a single keystroke. Key-up and non-key records — which
/// rustyline discards but [`console_pending_events`] counts — are skipped.
#[cfg(windows)]
fn drain_console_paste(first: char) -> String {
    use winapi::um::consoleapi::ReadConsoleInputW;
    use winapi::um::processenv::GetStdHandle;
    use winapi::um::winbase::STD_INPUT_HANDLE;
    use winapi::um::wincon::INPUT_RECORD;

    let mut units: Vec<u16> = Vec::new();
    let mut buf = [0u16; 2];
    units.extend_from_slice(first.encode_utf16(&mut buf));

    unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        loop {
            if console_pending_events() == 0 {
                // The queue can briefly empty between chunks of a large paste;
                // poll a little before concluding the burst is over.
                let mut more = false;
                for _ in 0..4 {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    if console_pending_events() > 0 {
                        more = true;
                        break;
                    }
                }
                if !more {
                    break;
                }
            }

            let mut record: INPUT_RECORD = std::mem::zeroed();
            let mut read: u32 = 0;
            if ReadConsoleInputW(handle, &mut record, 1, &mut read) == 0 || read == 0 {
                break;
            }
            if let Some(ch) = key_event_char(&record) {
                units.push(ch);
            }
        }
    }

    normalize_paste_text(&String::from_utf16_lossy(&units))
}

#[cfg(not(windows))]
fn drain_console_paste(_first: char) -> String {
    String::new()
}

/// A non-destructive look at the characters queued behind the current event.
#[cfg(windows)]
enum PeekedBurst {
    /// Larger than we bother to scan — unambiguously a paste.
    TooLarge,
    /// The key-down characters currently queued (key-up and non-key records,
    /// which rustyline discards, are omitted). May be empty.
    Chars(Vec<u16>),
}

/// Peek the queued console input without consuming it. rustyline reads one event
/// at a time, so the remainder of a paste burst is still buffered here.
#[cfg(windows)]
fn peek_pending_chars() -> Option<PeekedBurst> {
    use winapi::um::processenv::GetStdHandle;
    use winapi::um::winbase::STD_INPUT_HANDLE;
    use winapi::um::wincon::{PeekConsoleInputW, INPUT_RECORD};

    const PEEK_CAP: u32 = 512;
    let pending = console_pending_events();
    if pending == 0 {
        return None;
    }
    if pending > PEEK_CAP {
        return Some(PeekedBurst::TooLarge);
    }

    unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        let mut records: Vec<INPUT_RECORD> = vec![std::mem::zeroed(); pending as usize];
        let mut read: u32 = 0;
        if PeekConsoleInputW(handle, records.as_mut_ptr(), pending, &mut read) == 0 {
            return None;
        }

        let mut chars: Vec<u16> = Vec::new();
        for record in records.iter().take(read as usize) {
            if let Some(ch) = key_event_char(record) {
                chars.push(ch);
            }
        }
        Some(PeekedBurst::Chars(chars))
    }
}

/// Distinguish a genuine multi-line paste from fast type-ahead. We treat the
/// queued burst as a paste only when a newline is followed by more input. A
/// single typed line terminated by Enter has its newline last, so it is not a
/// paste and must still submit normally. A burst too large to scan is
/// unambiguously a paste. Always `false` off Windows.
#[cfg(windows)]
fn console_burst_is_paste() -> bool {
    match peek_pending_chars() {
        None => false,
        Some(PeekedBurst::TooLarge) => true,
        Some(PeekedBurst::Chars(chars)) => chars
            .iter()
            .enumerate()
            .any(|(i, &c)| (c == 0x0D || c == 0x0A) && i + 1 < chars.len()),
    }
}

/// Whether any printable character is queued behind the current event. Used for
/// a paste that begins with a newline: the leading newline arrives as the Enter
/// event and is consumed before we peek, so the "more input after the newline"
/// that marks a paste is whatever remains queued. Always `false` off Windows.
#[cfg(windows)]
fn console_has_pending_char() -> bool {
    match peek_pending_chars() {
        None => false,
        Some(PeekedBurst::TooLarge) => true,
        Some(PeekedBurst::Chars(chars)) => !chars.is_empty(),
    }
}

#[cfg(not(windows))]
fn console_burst_is_paste() -> bool {
    false
}

#[cfg(not(windows))]
fn console_has_pending_char() -> bool {
    false
}

/// If `first` begins a multi-line paste burst, capture the whole burst and
/// return the command that renders it — a `[Pasted N lines]` chip, or the literal
/// text when it is too small to collapse. Returns `None` for ordinary keystrokes
/// and type-ahead (and always off Windows, where rustyline handles pastes via
/// bracketed paste). When `first` is itself the newline (the Enter path), it is
/// the paste's leading newline, so any queued printable text makes it a paste.
fn capture_paste(
    state: &Arc<std::sync::RwLock<PasteState>>,
    first: char,
    editor_line: &str,
    cursor_position: usize,
) -> Option<rustyline::Cmd> {
    if console_pending_events() < PASTE_QUEUE_THRESHOLD {
        return None;
    }
    let is_paste = if first == '\n' || first == '\r' {
        console_has_pending_char()
    } else {
        console_burst_is_paste()
    };
    if !is_paste {
        return None;
    }

    let content = drain_console_paste(first);
    let mut state = state.write().ok()?;
    let id = state.next_id + 1;
    Some(
        match paste_marker(&content, id, editor_line, cursor_position) {
            Some(marker) => {
                state.next_id = id;
                let cmd = rustyline::Cmd::Insert(1, marker.clone());
                state.pastes.push(Paste { marker, content });
                cmd
            }
            None => rustyline::Cmd::Insert(1, content),
        },
    )
}

/// Intercepts printable characters so a pasted burst is captured into
/// [`PasteState`] instead of being echoed line by line.
pub(super) struct PasteCaptureHandler {
    paste_state: Arc<std::sync::RwLock<PasteState>>,
}

impl PasteCaptureHandler {
    pub(super) fn new(paste_state: Arc<std::sync::RwLock<PasteState>>) -> Self {
        Self { paste_state }
    }
}

impl rustyline::ConditionalEventHandler for PasteCaptureHandler {
    fn handle(
        &self,
        event: &rustyline::Event,
        _n: u16,
        _positive: bool,
        ctx: &rustyline::EventContext,
    ) -> Option<rustyline::Cmd> {
        let ch = match event.get(0)? {
            rustyline::KeyEvent(rustyline::KeyCode::Char(c), m)
                if *m == rustyline::Modifiers::NONE || *m == rustyline::Modifiers::SHIFT =>
            {
                *c
            }
            rustyline::KeyEvent(rustyline::KeyCode::Tab, rustyline::Modifiers::NONE) => '\t',
            _ => return None,
        };
        capture_paste(&self.paste_state, ch, ctx.line(), ctx.pos())
    }
}

/// Handles Enter: a newline that begins a paste burst is folded into the pasted
/// block; a genuine keystroke accepts the line.
pub(super) struct PasteAwareEnterHandler {
    paste_state: Arc<std::sync::RwLock<PasteState>>,
}

impl PasteAwareEnterHandler {
    pub(super) fn new(paste_state: Arc<std::sync::RwLock<PasteState>>) -> Self {
        Self { paste_state }
    }
}

impl rustyline::ConditionalEventHandler for PasteAwareEnterHandler {
    fn handle(
        &self,
        _event: &rustyline::Event,
        _n: u16,
        _positive: bool,
        ctx: &rustyline::EventContext,
    ) -> Option<rustyline::Cmd> {
        Some(
            capture_paste(&self.paste_state, '\n', ctx.line(), ctx.pos())
                .unwrap_or(rustyline::Cmd::AcceptLine),
        )
    }
}

#[cfg(windows)]
fn normalize_paste_text(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Build the chip shown in place of a pasted block, or `None` when inserting the
/// paste would make the full editor buffer a slash command. `id` makes the
/// marker unique to this paste instance so a cleared chip can never be expanded
/// into a later, identical-looking one (see [`expand_pastes`]).
fn paste_marker(
    content: &str,
    id: usize,
    editor_line: &str,
    cursor_position: usize,
) -> Option<String> {
    let (before, after) = editor_line.split_at(cursor_position);
    let first_after_insert = before
        .chars()
        .next()
        .or_else(|| content.chars().next())
        .or_else(|| after.chars().next());
    if first_after_insert == Some('/') {
        return None;
    }

    let lines = content.trim_end_matches('\n').matches('\n').count() + 1;
    if lines >= PASTE_CHIP_MIN_LINES {
        Some(format!("[Pasted {lines} lines #{id}]"))
    } else if content.chars().count() >= PASTE_CHIP_MIN_CHARS {
        Some(format!("[Pasted {} chars #{id}]", content.chars().count()))
    } else {
        None
    }
}

/// Expand chip markers in the submitted line back to their pasted content, in
/// the order the chips appear in the line, so `> summarize [Pasted 50 lines]`
/// submits the full text. Chips are matched by position rather than by capture
/// order, so reordering them in the prompt still expands every block.
fn expand_pastes(line: &str, pastes: &[Paste]) -> String {
    if pastes.is_empty() {
        return line.to_string();
    }

    let mut result = String::with_capacity(line.len());
    let mut rest = line;
    while let Some((idx, paste)) = pastes
        .iter()
        .filter_map(|paste| rest.find(&paste.marker).map(|idx| (idx, paste)))
        .min_by_key(|(idx, _)| *idx)
    {
        let (before, after) = rest.split_at(idx);
        result.push_str(before);
        result.push_str(&paste.content);
        rest = after.strip_prefix(paste.marker.as_str()).unwrap_or(after);
    }
    result.push_str(rest);
    result
}

enum PasteSubmission {
    Ready(String),
    NeedsReview(String),
}

fn prepare_paste_submission(line: &str, pastes: &[Paste]) -> PasteSubmission {
    let contained_chip = pastes.iter().any(|paste| line.contains(&paste.marker));
    let expanded = expand_pastes(line, pastes);
    if contained_chip && expanded.starts_with('/') {
        PasteSubmission::NeedsReview(expanded)
    } else {
        PasteSubmission::Ready(expanded)
    }
}

pub(super) fn read_paste_aware_input(
    editor: &mut Editor<GooseCompleter, rustyline::history::DefaultHistory>,
    paste_state: Arc<std::sync::RwLock<PasteState>>,
) -> rustyline::Result<String> {
    let mut prefill: Option<String> = None;
    loop {
        let input = match prefill.take() {
            Some(text) => editor.readline_with_initial("> ", (&text, ""))?,
            None => editor.readline("> ")?,
        };
        let submission = paste_state
            .read()
            .ok()
            .map(|state| prepare_paste_submission(&input, &state.pastes))
            .unwrap_or(PasteSubmission::Ready(input));
        match submission {
            PasteSubmission::Ready(expanded) => return Ok(expanded),
            PasteSubmission::NeedsReview(expanded) => {
                if let Ok(mut state) = paste_state.write() {
                    state.pastes.clear();
                }
                prefill = Some(expanded);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(content: &str, id: usize) -> Option<String> {
        paste_marker(content, id, "", 0)
    }

    #[test]
    fn test_paste_marker() {
        assert_eq!(marker("single line", 1), None);
        assert_eq!(
            marker("line one\nline two", 1),
            Some("[Pasted 2 lines #1]".to_string())
        );
        // Trailing newline is not counted as an extra line.
        assert_eq!(
            marker("line one\nline two\n", 2),
            Some("[Pasted 2 lines #2]".to_string())
        );
        let long = "x".repeat(PASTE_CHIP_MIN_CHARS);
        assert_eq!(
            marker(&long, 3),
            Some(format!("[Pasted {PASTE_CHIP_MIN_CHARS} chars #3]"))
        );
    }

    #[test]
    fn test_paste_marker_keeps_direct_and_split_slash_commands_visible() {
        assert_eq!(marker("/extension example\nignored", 4), None);

        let long = format!("/mode auto {}", "x".repeat(PASTE_CHIP_MIN_CHARS));
        assert_eq!(marker(&long, 5), None);

        assert_eq!(paste_marker("extension example\nignored", 6, "/", 1), None);
        assert_eq!(
            paste_marker(
                &format!("mode auto {}", "x".repeat(PASTE_CHIP_MIN_CHARS)),
                7,
                "/",
                1,
            ),
            None
        );
    }

    #[test]
    fn test_paste_marker_preserves_non_command_chips() {
        assert_eq!(
            marker("explain /extension\ncontinued", 8),
            Some("[Pasted 2 lines #8]".to_string())
        );
        assert_eq!(
            paste_marker("/extension example\ncontinued", 9, "explain ", 8),
            Some("[Pasted 2 lines #9]".to_string())
        );
        assert_eq!(
            paste_marker("extension example\ncontinued", 10, "explain /", 9),
            Some("[Pasted 2 lines #10]".to_string())
        );
    }

    #[test]
    fn test_expand_pastes() {
        assert_eq!(expand_pastes("no chips here", &[]), "no chips here");

        let pastes = vec![Paste {
            marker: "[Pasted 2 lines #1]".to_string(),
            content: "a\nb".to_string(),
        }];
        assert_eq!(
            expand_pastes("summarize [Pasted 2 lines #1] please", &pastes),
            "summarize a\nb please"
        );

        // Deleting the chip drops its content rather than corrupting the message.
        assert_eq!(
            expand_pastes("summarize please", &pastes),
            "summarize please"
        );
    }

    #[test]
    fn test_expand_pastes_multiple_in_order() {
        let pastes = vec![
            Paste {
                marker: "[Pasted 2 lines #1]".to_string(),
                content: "FIRST".to_string(),
            },
            Paste {
                marker: "[Pasted 3 lines #2]".to_string(),
                content: "SECOND".to_string(),
            },
        ];
        assert_eq!(
            expand_pastes("[Pasted 2 lines #1] and [Pasted 3 lines #2]", &pastes),
            "FIRST and SECOND"
        );
    }

    #[test]
    fn test_expand_pastes_reordered() {
        // The chips are moved so a later paste appears before an earlier one.
        // Matching by position (not capture order) still expands both.
        let pastes = vec![
            Paste {
                marker: "[Pasted 2 lines #1]".to_string(),
                content: "FIRST".to_string(),
            },
            Paste {
                marker: "[Pasted 3 lines #2]".to_string(),
                content: "SECOND".to_string(),
            },
        ];
        assert_eq!(
            expand_pastes("[Pasted 3 lines #2] then [Pasted 2 lines #1]", &pastes),
            "SECOND then FIRST"
        );
    }

    #[test]
    fn test_expand_pastes_skips_cleared_paste() {
        // A paste was made, cleared (Ctrl+C), then another paste with the same
        // line count was made. Unique ids keep the stale entry from hijacking the
        // visible chip: only the paste actually shown is expanded.
        let pastes = vec![
            Paste {
                marker: "[Pasted 2 lines #1]".to_string(),
                content: "CLEARED".to_string(),
            },
            Paste {
                marker: "[Pasted 2 lines #2]".to_string(),
                content: "CURRENT".to_string(),
            },
        ];
        assert_eq!(expand_pastes("[Pasted 2 lines #2]", &pastes), "CURRENT");
    }

    #[test]
    fn test_prepare_paste_submission_reviews_hidden_slash_after_edit() {
        let pasted_slash = vec![Paste {
            marker: "[Pasted 2 lines #1]".to_string(),
            content: "/extension example\ncontinued".to_string(),
        }];
        assert!(matches!(
            prepare_paste_submission("[Pasted 2 lines #1]", &pasted_slash),
            PasteSubmission::NeedsReview(input)
                if input == "/extension example\ncontinued"
        ));

        let pasted_command = vec![Paste {
            marker: "[Pasted 2 lines #2]".to_string(),
            content: "extension example\ncontinued".to_string(),
        }];
        assert!(matches!(
            prepare_paste_submission("/[Pasted 2 lines #2]", &pasted_command),
            PasteSubmission::NeedsReview(input)
                if input == "/extension example\ncontinued"
        ));
    }

    #[test]
    fn test_prepare_paste_submission_preserves_legitimate_chips() {
        let pastes = vec![Paste {
            marker: "[Pasted 2 lines #1]".to_string(),
            content: "/extension example\ncontinued".to_string(),
        }];
        assert!(matches!(
            prepare_paste_submission("explain [Pasted 2 lines #1]", &pastes),
            PasteSubmission::Ready(input)
                if input == "explain /extension example\ncontinued"
        ));
    }

    #[test]
    fn test_capture_paste_ignored_without_burst() {
        // Off Windows (and on Windows with no queued burst) there is nothing to
        // drain, so ordinary keystrokes are never captured as a paste.
        let state = Arc::new(std::sync::RwLock::new(PasteState::default()));
        assert!(capture_paste(&state, 'a', "", 0).is_none());
        assert!(state.read().unwrap().pastes.is_empty());
    }
}
