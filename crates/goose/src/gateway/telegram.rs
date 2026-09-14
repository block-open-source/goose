use super::{
    Gateway, GatewayConfig, GatewayHandler, IncomingMessage, OutgoingMessage, PlatformUser,
};
use async_trait::async_trait;
use reqwest::{Client, RequestBuilder, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";
const POLL_TIMEOUT_SECS: u64 = 30;
const MAX_MESSAGE_LENGTH: usize = 4096;
const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(5);
/// Maximum voice file size we'll attempt to download (20 MB, Telegram's bot API limit).
const MAX_VOICE_FILE_SIZE: i64 = 20 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VoiceFileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume: u32,
    #[cfg(windows)]
    index_high: u32,
    #[cfg(windows)]
    index_low: u32,
}

struct VoiceTempFile {
    path: tempfile::TempPath,
    identity: VoiceFileIdentity,
    created_at: std::time::SystemTime,
    cleanup_on_drop: bool,
}

impl VoiceTempFile {
    fn remove(&mut self) -> io::Result<bool> {
        self.path.disable_cleanup(true);
        let removed = remove_voice_file_if_unchanged(&self.path, Some(self.identity), |_| {})?;
        self.cleanup_on_drop = false;
        Ok(removed)
    }
}

impl Drop for VoiceTempFile {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            let path = self.path.to_path_buf();
            self.path.disable_cleanup(true);
            let _ = remove_voice_file_if_unchanged(&path, Some(self.identity), |_| {});
        }
    }
}

struct VoiceTempFiles {
    parent: PathBuf,
    files: Mutex<Vec<VoiceTempFile>>,
}

impl VoiceTempFiles {
    fn new_in(parent: impl Into<PathBuf>) -> Self {
        Self {
            parent: parent.into(),
            files: Mutex::new(Vec::new()),
        }
    }

    fn save(&self, bytes: &[u8], extension: &str) -> io::Result<PathBuf> {
        let mut file = tempfile::Builder::new()
            .prefix("goose_voice_")
            .suffix(&format!(".{extension}"))
            .tempfile_in(&self.parent)?;
        file.write_all(bytes)?;
        let identity = voice_file_identity(file.as_file())?;
        let path = file.path().to_path_buf();
        let path_owner = file.into_temp_path();
        self.files
            .lock()
            .map_err(|_| io::Error::other("Telegram voice file registry is unavailable"))?
            .push(VoiceTempFile {
                path: path_owner,
                identity,
                created_at: std::time::SystemTime::now(),
                cleanup_on_drop: true,
            });
        Ok(path)
    }

    fn cleanup(&self, max_age: std::time::Duration) -> io::Result<u32> {
        self.cleanup_with_hook(max_age, |_| {})
    }

    fn cleanup_with_hook(
        &self,
        max_age: std::time::Duration,
        mut after_opened_candidate: impl FnMut(&std::path::Path),
    ) -> io::Result<u32> {
        let cutoff = std::time::SystemTime::now()
            .checked_sub(max_age)
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let mut files = self
            .files
            .lock()
            .map_err(|_| io::Error::other("Telegram voice file registry is unavailable"))?;
        let mut removed_tracked = 0;
        let mut retained = Vec::with_capacity(files.len());
        for mut file in std::mem::take(&mut *files) {
            if file.created_at > cutoff {
                retained.push(file);
                continue;
            }
            match file.remove() {
                Ok(true) => removed_tracked += 1,
                Ok(false) => {}
                Err(_) => retained.push(file),
            }
        }
        *files = retained;
        let active_paths: std::collections::HashSet<PathBuf> =
            files.iter().map(|file| file.path.to_path_buf()).collect();
        drop(files);

        let removed_orphans = cleanup_orphaned_voice_files(
            &self.parent,
            cutoff,
            &active_paths,
            &mut after_opened_candidate,
        )?;
        let removed_legacy =
            cleanup_legacy_voice_files(&self.parent, cutoff, &mut after_opened_candidate)?;
        Ok(removed_tracked + removed_orphans + removed_legacy)
    }

    #[cfg(test)]
    fn parent(&self) -> &std::path::Path {
        &self.parent
    }
}

fn cleanup_orphaned_voice_files(
    parent: &std::path::Path,
    cutoff: std::time::SystemTime,
    active_paths: &std::collections::HashSet<PathBuf>,
    after_opened_candidate: &mut impl FnMut(&std::path::Path),
) -> io::Result<u32> {
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut removed = 0;
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        if !is_goose_voice_file_name(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        if active_paths.contains(&path) {
            continue;
        }
        let Ok(file) = open_owned_voice_file(&path) else {
            continue;
        };
        let Ok(metadata) = file.metadata() else {
            continue;
        };
        if metadata
            .modified()
            .map_or(true, |modified| modified > cutoff)
        {
            continue;
        }
        let Ok(identity) = voice_file_identity(&file) else {
            continue;
        };
        after_opened_candidate(&path);
        if remove_voice_file_if_unchanged(&path, Some(identity), |_| {})? {
            removed += 1;
        }
    }
    Ok(removed)
}

fn is_goose_voice_file_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let Some(rest) = name.strip_prefix("goose_voice_") else {
        return false;
    };
    let Some((random, extension)) = rest.split_once('.') else {
        return false;
    };
    random.len() == 6
        && random.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && is_voice_file_extension(extension)
}

fn is_legacy_voice_file_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let Some(rest) = name.strip_prefix("voice_") else {
        return false;
    };
    let Some((uuid, extension)) = rest.split_once('.') else {
        return false;
    };
    let uuid_bytes = uuid.as_bytes();
    uuid_bytes.len() == 36
        && uuid_bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23) && *byte == b'-'
                || !matches!(index, 8 | 13 | 18 | 23) && byte.is_ascii_hexdigit()
        })
        && is_voice_file_extension(extension)
}

fn is_voice_file_extension(extension: &str) -> bool {
    !extension.is_empty()
        && extension.len() <= 16
        && extension.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && matches!(byte, b'-' | b'_'))
        })
}

fn cleanup_legacy_voice_files(
    parent: &std::path::Path,
    cutoff: std::time::SystemTime,
    after_opened_candidate: &mut impl FnMut(&std::path::Path),
) -> io::Result<u32> {
    let root_path = parent.join("goose_voice");
    let Ok(root) = open_legacy_voice_root(&root_path) else {
        return Ok(0);
    };
    let entries = match std::fs::read_dir(&root_path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut removed = 0;
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name();
        if !is_legacy_voice_file_name(&name) {
            continue;
        }
        let Ok(file) = open_owned_voice_file_at(&root, &name) else {
            continue;
        };
        let Ok(metadata) = file.metadata() else {
            continue;
        };
        if metadata
            .modified()
            .map_or(true, |modified| modified > cutoff)
        {
            continue;
        }
        let Ok(identity) = voice_file_identity(&file) else {
            continue;
        };
        let path = root_path.join(&name);
        after_opened_candidate(&path);
        let Ok(current) = open_owned_voice_file_at(&root, &name) else {
            continue;
        };
        if voice_file_identity(&current)? != identity {
            continue;
        }
        delete_open_legacy_voice_file(&root, &name, current)?;
        removed += 1;
    }
    Ok(removed)
}

fn remove_voice_file_if_unchanged(
    path: &std::path::Path,
    expected_identity: Option<VoiceFileIdentity>,
    mut after_opened: impl FnMut(&std::path::Path),
) -> io::Result<bool> {
    let file = match open_owned_voice_file(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Ok(false),
    };
    let identity = voice_file_identity(&file)?;
    if expected_identity.is_some_and(|expected| expected != identity) {
        return Ok(false);
    }
    after_opened(path);
    let current = match open_owned_voice_file(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Ok(false),
    };
    if voice_file_identity(&current)? != identity {
        return Ok(false);
    }
    delete_open_voice_file(current, path)?;
    Ok(true)
}

#[cfg(unix)]
fn delete_open_voice_file(_file: std::fs::File, path: &std::path::Path) -> io::Result<()> {
    std::fs::remove_file(path)
}

#[cfg(unix)]
fn open_legacy_voice_root(path: &std::path::Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = directory.metadata()?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not an owned legacy Telegram voice directory",
        ));
    }
    Ok(directory)
}

#[cfg(unix)]
fn open_owned_voice_file_at(
    directory: &std::fs::File,
    name: &std::ffi::OsStr,
) -> io::Result<std::fs::File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let name = CString::new(name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Telegram voice filename contains a NUL byte",
        )
    })?;
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { std::fs::File::from_raw_fd(descriptor) };
    validate_owned_voice_file(&file)?;
    Ok(file)
}

#[cfg(unix)]
fn delete_open_legacy_voice_file(
    directory: &std::fs::File,
    name: &std::ffi::OsStr,
    _file: std::fs::File,
) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    let name = CString::new(name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Telegram voice filename contains a NUL byte",
        )
    })?;
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn open_owned_voice_file(path: &std::path::Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    validate_owned_voice_file(&file)?;
    Ok(file)
}

#[cfg(unix)]
fn validate_owned_voice_file(file: &std::fs::File) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not an owned Telegram voice tempfile",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn voice_file_identity(file: &std::fs::File) -> io::Result<VoiceFileIdentity> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    Ok(VoiceFileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(windows)]
fn open_owned_voice_file(path: &std::path::Path) -> io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use winapi::um::winbase::FILE_FLAG_OPEN_REPARSE_POINT;
    use winapi::um::winnt::{
        DELETE, FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_GENERIC_READ | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    validate_owned_voice_file(&file)?;
    Ok(file)
}

#[cfg(windows)]
fn validate_owned_voice_file(file: &std::fs::File) -> io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    use winapi::um::winnt::FILE_ATTRIBUTE_REPARSE_POINT;

    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not an owned Telegram voice tempfile",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn open_legacy_voice_root(path: &std::path::Path) -> io::Result<std::fs::File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use winapi::um::winbase::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT};
    use winapi::um::winnt::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, SYNCHRONIZE,
    };

    let directory = std::fs::OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not an owned legacy Telegram voice directory",
        ));
    }
    Ok(directory)
}

#[cfg(windows)]
fn open_owned_voice_file_at(
    directory: &std::fs::File,
    name: &std::ffi::OsStr,
) -> io::Result<std::fs::File> {
    use ntapi::ntioapi::{
        NtCreateFile, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
        IO_STATUS_BLOCK,
    };
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use winapi::shared::ntdef::{
        HANDLE, NT_SUCCESS, OBJECT_ATTRIBUTES, OBJ_CASE_INSENSITIVE, UNICODE_STRING,
    };
    use winapi::um::winnt::{
        DELETE, FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut name: Vec<u16> = name.encode_wide().collect();
    let name_bytes = name
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Telegram voice filename is too long",
            )
        })?;
    let mut unicode_name = UNICODE_STRING {
        Length: name_bytes,
        MaximumLength: name_bytes,
        Buffer: name.as_mut_ptr(),
    };
    let mut attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: directory.as_raw_handle() as HANDLE,
        ObjectName: &mut unicode_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: std::ptr::null_mut(),
        SecurityQualityOfService: std::ptr::null_mut(),
    };
    let mut handle: HANDLE = std::ptr::null_mut();
    let mut io_status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            FILE_GENERIC_READ | DELETE,
            &mut attributes,
            &mut io_status,
            std::ptr::null_mut(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            std::ptr::null_mut(),
            0,
        )
    };
    if !NT_SUCCESS(status) {
        let error = unsafe { ntapi::ntrtl::RtlNtStatusToDosError(status) };
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    let file = unsafe { std::fs::File::from_raw_handle(handle.cast()) };
    validate_owned_voice_file(&file)?;
    Ok(file)
}

#[cfg(windows)]
fn delete_open_legacy_voice_file(
    _directory: &std::fs::File,
    _name: &std::ffi::OsStr,
    file: std::fs::File,
) -> io::Result<()> {
    delete_open_voice_file(file, std::path::Path::new(""))
}

#[cfg(windows)]
fn delete_open_voice_file(file: std::fs::File, _path: &std::path::Path) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use winapi::um::fileapi::{SetFileInformationByHandle, FILE_DISPOSITION_INFO};
    use winapi::um::minwinbase::FileDispositionInfo;

    let mut disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    // SAFETY: the handle is live and the information buffer matches FileDispositionInfo.
    let result = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle().cast(),
            FileDispositionInfo,
            (&mut disposition as *mut FILE_DISPOSITION_INFO).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
fn voice_file_identity(file: &std::fs::File) -> io::Result<VoiceFileIdentity> {
    use std::os::windows::io::AsRawHandle;
    use winapi::um::fileapi::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};

    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let result =
        unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), &mut information) };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(VoiceFileIdentity {
        volume: information.dwVolumeSerialNumber,
        index_high: information.nFileIndexHigh,
        index_low: information.nFileIndexLow,
    })
}

pub struct TelegramGateway {
    bot_token: String,
    client: Client,
    api_base: String,
    voice_temp_files: Arc<VoiceTempFiles>,
}

#[derive(Debug, Serialize)]
struct SendRichMessageRequest<'a> {
    chat_id: i64,
    rich_message: InputRichMessage<'a>,
}

#[derive(Debug, Serialize)]
struct InputRichMessage<'a> {
    markdown: &'a str,
}

#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    message_id: i64,
    from: Option<TelegramUser>,
    chat: TelegramChat,
    text: Option<String>,
    voice: Option<TelegramVoice>,
    audio: Option<TelegramAudio>,
}

#[derive(Debug, Deserialize)]
struct TelegramVoice {
    file_id: String,
    #[allow(dead_code)]
    duration: Option<i32>,
    #[allow(dead_code)]
    mime_type: Option<String>,
    file_size: Option<i64>,
}

/// Audio files sent as documents (not inline voice notes).
#[derive(Debug, Deserialize)]
struct TelegramAudio {
    file_id: String,
    #[allow(dead_code)]
    duration: Option<i32>,
    #[allow(dead_code)]
    mime_type: Option<String>,
    file_size: Option<i64>,
}

/// Metadata extracted from a Telegram voice note or audio attachment.
struct VoiceInfo<'a> {
    file_id: &'a str,
    file_size: Option<i64>,
    duration: Option<i32>,
    mime_type: Option<&'a str>,
}

/// Response from the Telegram `getFile` API.
#[derive(Debug, Deserialize)]
struct TelegramFile {
    #[allow(dead_code)]
    file_id: String,
    file_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramUser {
    first_name: String,
    last_name: Option<String>,
    #[allow(dead_code)]
    username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramChat {
    id: i64,
    #[allow(dead_code)]
    #[serde(rename = "type")]
    chat_type: String,
}

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

impl TelegramGateway {
    pub fn new(config: &GatewayConfig) -> anyhow::Result<Self> {
        Self::new_with_voice_temp_parent(config, std::env::temp_dir())
    }

    fn new_with_voice_temp_parent(
        config: &GatewayConfig,
        voice_temp_parent: PathBuf,
    ) -> anyhow::Result<Self> {
        let bot_token = config.platform_config["bot_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing bot_token in platform_config"))?
            .to_string();

        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .http1_only()
            .build()?;
        Ok(Self {
            bot_token,
            client,
            api_base: TELEGRAM_API_BASE.to_string(),
            voice_temp_files: Arc::new(VoiceTempFiles::new_in(voice_temp_parent)),
        })
    }

    fn api_url(&self, method: &str) -> String {
        format!("{}/bot{}/{}", self.api_base, self.bot_token, method)
    }

    async fn send_request(request: RequestBuilder) -> reqwest::Result<Response> {
        request.send().await.map_err(reqwest::Error::without_url)
    }

    async fn response_json<T: DeserializeOwned>(response: Response) -> reqwest::Result<T> {
        response.json().await.map_err(reqwest::Error::without_url)
    }

    async fn response_bytes(response: Response) -> reqwest::Result<Vec<u8>> {
        response
            .bytes()
            .await
            .map(Vec::from)
            .map_err(reqwest::Error::without_url)
    }

    async fn get_updates(&self, offset: Option<i64>) -> anyhow::Result<Vec<TelegramUpdate>> {
        let mut params = serde_json::json!({
            "timeout": POLL_TIMEOUT_SECS,
            "allowed_updates": ["message"],
        });
        if let Some(offset) = offset {
            params["offset"] = serde_json::json!(offset);
        }

        let response = Self::send_request(
            self.client
                .post(self.api_url("getUpdates"))
                .json(&params)
                .timeout(std::time::Duration::from_secs(POLL_TIMEOUT_SECS + 10)),
        )
        .await?;
        let resp: TelegramResponse<Vec<TelegramUpdate>> = Self::response_json(response).await?;

        resp.result.ok_or_else(|| {
            anyhow::anyhow!(
                "Telegram API error: {}",
                resp.description.unwrap_or_default()
            )
        })
    }

    async fn send_text(&self, chat_id: i64, text: &str) -> anyhow::Result<()> {
        let chunks = split_message(text, MAX_MESSAGE_LENGTH);
        for (index, chunk) in chunks.iter().enumerate() {
            let resp = Self::send_request(self.client.post(self.api_url("sendRichMessage")).json(
                &SendRichMessageRequest {
                    chat_id,
                    rich_message: InputRichMessage { markdown: chunk },
                },
            ))
            .await?;

            if let Ok(body) = Self::response_json::<TelegramResponse<serde_json::Value>>(resp).await
            {
                if !body.ok {
                    tracing::warn!(
                        error = body.description.as_deref().unwrap_or("unknown"),
                        "Telegram rejected rich markdown, falling back to plain text"
                    );
                    for plain_chunk in &chunks[index..] {
                        let plain_response =
                            Self::send_request(self.client.post(self.api_url("sendMessage")).json(
                                &serde_json::json!({
                                    "chat_id": chat_id,
                                    "text": plain_chunk,
                                }),
                            ))
                            .await?;
                        let plain_resp: TelegramResponse<serde_json::Value> =
                            Self::response_json(plain_response).await?;
                        if !plain_resp.ok {
                            anyhow::bail!(
                                "Telegram sendMessage failed: {}",
                                plain_resp.description.unwrap_or_default()
                            );
                        }
                    }
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    async fn send_chat_action(&self, chat_id: i64, action: &str) -> anyhow::Result<()> {
        Self::send_request(self.client.post(self.api_url("sendChatAction")).json(
            &serde_json::json!({
                "chat_id": chat_id,
                "action": action,
            }),
        ))
        .await?;
        Ok(())
    }

    /// Download a file from Telegram by its `file_id`.
    ///
    /// This is a two-step process:
    /// 1. Call `getFile` to obtain the server-side `file_path`.
    /// 2. Fetch the raw bytes from `https://api.telegram.org/file/bot<TOKEN>/<file_path>`.
    async fn download_file(&self, file_id: &str) -> anyhow::Result<Vec<u8>> {
        // Step 1 – resolve file_id → file_path
        let response = Self::send_request(
            self.client
                .post(self.api_url("getFile"))
                .json(&serde_json::json!({ "file_id": file_id })),
        )
        .await?;
        let resp: TelegramResponse<TelegramFile> = Self::response_json(response).await?;

        let tg_file = resp.result.ok_or_else(|| {
            anyhow::anyhow!(
                "Telegram getFile error: {}",
                resp.description.unwrap_or_default()
            )
        })?;

        let file_path = tg_file
            .file_path
            .ok_or_else(|| anyhow::anyhow!("Telegram getFile returned no file_path"))?;

        // Step 2 – download raw bytes
        let download_url = format!(
            "{}/file/bot{}/{}",
            TELEGRAM_API_BASE, self.bot_token, file_path
        );
        let response = Self::send_request(self.client.get(&download_url)).await?;
        Ok(Self::response_bytes(response).await?)
    }

    /// Save voice bytes to a temporary file and return the path.
    ///
    /// Files are stored as protected, exclusively created temporary files so
    /// Goose can access them via its shell tools. The extension is derived from
    /// the MIME type when available, falling back to `.ogg` for voice notes.
    ///
    /// On Unix files are created with mode `0600` so other local users cannot
    /// read private voice content.
    fn save_voice_file(&self, bytes: &[u8], mime_type: Option<&str>) -> anyhow::Result<PathBuf> {
        let ext = Self::voice_file_extension(mime_type);
        Ok(self.voice_temp_files.save(bytes, &ext)?)
    }

    fn voice_file_extension(mime_type: Option<&str>) -> String {
        let media_type = mime_type
            .and_then(|mime| mime.split(';').next())
            .map(str::trim)
            .map(str::to_ascii_lowercase);
        let subtype = media_type
            .as_deref()
            .and_then(|mime| mime.strip_prefix("audio/"));

        let Some(subtype) = subtype else {
            return "ogg".to_string();
        };

        match subtype {
            "mpeg" => "mp3".to_string(),
            "mp4" | "x-m4a" => "m4a".to_string(),
            "ogg" => "ogg".to_string(),
            "wav" | "x-wav" | "vnd.wave" => "wav".to_string(),
            other
                if other.len() <= 16
                    && other.bytes().enumerate().all(|(index, byte)| {
                        byte.is_ascii_alphanumeric() || (index > 0 && matches!(byte, b'-' | b'_'))
                    }) =>
            {
                other.to_string()
            }
            _ => "ogg".to_string(),
        }
    }

    /// Build the text prompt that tells Goose about a voice message file.
    fn voice_prompt(
        path: &std::path::Path,
        duration: Option<i32>,
        mime_type: Option<&str>,
    ) -> String {
        let duration_hint = duration
            .map(|d| format!(" (duration: {d}s)"))
            .unwrap_or_default();
        let format_hint = mime_type
            .map(|m| format!(" The file format is {m}."))
            .unwrap_or_default();
        format!(
            "The user sent a voice message{duration_hint}. \
             The audio file is saved at: {}{format_hint}\n\n\
             Please transcribe this audio file using available command-line tools \
             (e.g. whisper, ffmpeg, sox, or any STT utility you can find on this system) \
             and then respond to what the user said. \
             If no transcription tool is available, let the user know and ask them to type their message instead.",
            path.display()
        )
    }

    /// Extract metadata from either a voice note or an audio attachment.
    /// Returns `None` when neither is present.
    fn voice_info(msg: &TelegramMessage) -> Option<VoiceInfo<'_>> {
        if let Some(ref v) = msg.voice {
            return Some(VoiceInfo {
                file_id: &v.file_id,
                file_size: v.file_size,
                duration: v.duration,
                mime_type: v.mime_type.as_deref(),
            });
        }
        if let Some(ref a) = msg.audio {
            return Some(VoiceInfo {
                file_id: &a.file_id,
                file_size: a.file_size,
                duration: a.duration,
                mime_type: a.mime_type.as_deref(),
            });
        }
        None
    }

    fn to_platform_user(tg_msg: &TelegramMessage) -> PlatformUser {
        PlatformUser {
            platform: "telegram".to_string(),
            user_id: tg_msg.chat.id.to_string(),
            display_name: tg_msg.from.as_ref().map(|u| {
                let mut name = u.first_name.clone();
                if let Some(ref last) = u.last_name {
                    name.push(' ');
                    name.push_str(last);
                }
                name
            }),
        }
    }
}

#[async_trait]
impl Gateway for TelegramGateway {
    fn gateway_type(&self) -> &str {
        "telegram"
    }

    async fn start(
        &self,
        handler: GatewayHandler,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        let mut offset: Option<i64> = None;

        tracing::info!("Telegram gateway starting long-poll loop");

        // Spawn a background task that periodically removes stale voice files
        // (older than 1 hour) so they don't accumulate on disk.
        let cleanup_cancel = cancel.clone();
        let voice_temp_files = Arc::clone(&self.voice_temp_files);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(600));
            loop {
                tokio::select! {
                    _ = cleanup_cancel.cancelled() => break,
                    _ = interval.tick() => {
                        if let Err(error) = voice_temp_files.cleanup(std::time::Duration::from_secs(3600)) {
                            tracing::warn!(%error, "failed to clean up Telegram voice files");
                        }
                    }
                }
            }
        });

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::info!("Telegram gateway shutting down");
                    break;
                }
                result = self.get_updates(offset) => {
                    match result {
                        Ok(updates) => {
                            for update in updates {
                                offset = Some(update.update_id + 1);

                                let Some(tg_msg) = update.message else {
                                    continue;
                                };

                                // Determine the text to send to the handler.
                                // Voice/audio messages are downloaded, saved to
                                // disk, and converted into a prompt that asks
                                // Goose to transcribe the file using CLI tools.
                                let text = if let Some(voice) = Self::voice_info(&tg_msg) {
                                    // Reject files that exceed the Telegram bot
                                    // download limit.
                                    if voice.file_size.unwrap_or(0) > MAX_VOICE_FILE_SIZE {
                                        tracing::warn!(
                                            file_size = voice.file_size,
                                            "voice file exceeds size limit, skipping"
                                        );
                                        continue;
                                    }

                                    match self.download_file(voice.file_id).await {
                                        Ok(bytes) => match self.save_voice_file(&bytes, voice.mime_type) {
                                            Ok(path) => Self::voice_prompt(&path, voice.duration, voice.mime_type),
                                            Err(e) => {
                                                tracing::error!(
                                                    error = %e,
                                                    "failed to save voice file"
                                                );
                                                continue;
                                            }
                                        },
                                        Err(e) => {
                                            tracing::error!(
                                                error = %e,
                                                "failed to download voice file from Telegram"
                                            );
                                            continue;
                                        }
                                    }
                                } else if let Some(ref t) = tg_msg.text {
                                    t.clone()
                                } else {
                                    // Neither text nor voice — skip.
                                    continue;
                                };

                                let user = Self::to_platform_user(&tg_msg);
                                let incoming = IncomingMessage {
                                    user,
                                    text,
                                    platform_message_id: Some(tg_msg.message_id.to_string()),
                                    attachments: vec![],
                                };

                                let handler = handler.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = handler.handle_message(incoming).await {
                                        tracing::error!(error = %e, "error handling Telegram message");
                                    }
                                });
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = ?e, "Telegram poll error");
                            tokio::time::sleep(RETRY_DELAY).await;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn send_message(
        &self,
        user: &PlatformUser,
        message: OutgoingMessage,
    ) -> anyhow::Result<()> {
        let chat_id: i64 = user
            .user_id
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid chat_id: {}", user.user_id))?;

        match message {
            OutgoingMessage::Text { body } => {
                self.send_text(chat_id, &body).await?;
            }
            OutgoingMessage::Typing => {
                self.send_chat_action(chat_id, "typing").await?;
            }
        }

        Ok(())
    }

    async fn validate_config(&self) -> anyhow::Result<()> {
        let response = Self::send_request(self.client.get(self.api_url("getMe"))).await?;
        let resp: TelegramResponse<serde_json::Value> = Self::response_json(response).await?;

        if !resp.ok {
            anyhow::bail!(
                "invalid Telegram bot token: {}",
                resp.description.unwrap_or_default()
            );
        }

        if let Some(result) = &resp.result {
            if let Some(username) = result.get("username").and_then(|v| v.as_str()) {
                tracing::info!(bot = %username, "Telegram bot verified");
            }
        }

        Ok(())
    }
}

#[allow(clippy::string_slice)]
fn split_message(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining.to_string());
            break;
        }

        let mut cut = max_len;
        while cut > 0 && !remaining.is_char_boundary(cut) {
            cut -= 1;
        }
        if cut == 0 {
            cut = remaining
                .char_indices()
                .nth(1)
                .map(|(i, _)| i)
                .unwrap_or(remaining.len());
        }

        let split_at = remaining[..cut]
            .rfind('\n')
            .or_else(|| remaining[..cut].rfind(' '))
            .map(|pos| pos + 1)
            .unwrap_or(cut);

        chunks.push(remaining[..split_at].to_string());
        remaining = &remaining[split_at..];
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SECRET_BOT_TOKEN: &str = "123456789:AASecret_Telegram_Token";

    fn test_gateway(api_base: String) -> TelegramGateway {
        TelegramGateway {
            bot_token: "test-token".to_string(),
            client: Client::builder().no_proxy().build().unwrap(),
            api_base,
            voice_temp_files: Arc::new(VoiceTempFiles::new_in(std::env::temp_dir())),
        }
    }

    fn secret_gateway(api_base: String) -> TelegramGateway {
        TelegramGateway {
            bot_token: SECRET_BOT_TOKEN.to_string(),
            client: Client::builder().no_proxy().build().unwrap(),
            api_base,
            voice_temp_files: Arc::new(VoiceTempFiles::new_in(std::env::temp_dir())),
        }
    }

    fn gateway_with_voice_temp_files(voice_temp_files: VoiceTempFiles) -> TelegramGateway {
        TelegramGateway {
            bot_token: "test-token".to_string(),
            client: Client::builder().no_proxy().build().unwrap(),
            api_base: TELEGRAM_API_BASE.to_string(),
            voice_temp_files: Arc::new(voice_temp_files),
        }
    }

    fn assert_log_fields_are_redacted(
        error: &(impl std::fmt::Display + std::fmt::Debug),
        diagnostic: &str,
    ) {
        let display_log_field = format!("{error}");
        let debug_log_field = format!("{error:?}");

        for rendered in [&display_log_field, &debug_log_field] {
            assert!(!rendered.contains(SECRET_BOT_TOKEN), "{rendered}");
        }
        assert!(
            display_log_field.contains(diagnostic),
            "{display_log_field}"
        );
    }

    #[tokio::test]
    async fn request_errors_remove_token_url_from_display_and_debug() {
        let server = MockServer::start().await;
        let request_path = format!("/bot{SECRET_BOT_TOKEN}/getMe");
        let redirect_url = format!("{}{request_path}", server.uri());

        Mock::given(method("GET"))
            .and(path(request_path))
            .respond_with(
                ResponseTemplate::new(302).append_header("Location", redirect_url.as_str()),
            )
            .mount(&server)
            .await;

        let error = secret_gateway(server.uri())
            .validate_config()
            .await
            .unwrap_err();

        assert_log_fields_are_redacted(&error, "redirect");
        assert!(error
            .downcast_ref::<reqwest::Error>()
            .unwrap()
            .is_redirect());
        assert!(error
            .downcast_ref::<reqwest::Error>()
            .unwrap()
            .url()
            .is_none());
    }

    #[tokio::test]
    async fn response_errors_remove_token_url_from_display_and_debug() {
        let server = MockServer::start().await;
        let request_path = format!("/bot{SECRET_BOT_TOKEN}/getUpdates");

        Mock::given(method("POST"))
            .and(path(request_path))
            .respond_with(ResponseTemplate::new(200).set_body_raw("{", "application/json"))
            .mount(&server)
            .await;

        let error = secret_gateway(server.uri())
            .get_updates(None)
            .await
            .unwrap_err();

        assert_log_fields_are_redacted(&error, "decoding response body");
        assert!(error.downcast_ref::<reqwest::Error>().unwrap().is_decode());
        assert!(error
            .downcast_ref::<reqwest::Error>()
            .unwrap()
            .url()
            .is_none());
    }

    #[tokio::test]
    async fn timeout_errors_remove_token_url_from_display_and_debug() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _connection = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });

        let gateway = secret_gateway(format!("http://{addr}"));
        let url = gateway.api_url("getMe");
        let error = TelegramGateway::send_request(
            gateway
                .client
                .get(url)
                .timeout(std::time::Duration::from_millis(20)),
        )
        .await
        .unwrap_err();

        assert_log_fields_are_redacted(&error, "error sending request");
        assert!(error.is_timeout());
        assert!(error.url().is_none());
    }

    #[tokio::test]
    async fn body_errors_remove_token_url_from_display_and_debug() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            let _ = socket.read(&mut request).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 32\r\nconnection: close\r\n\r\nshort",
                )
                .await
                .unwrap();
        });

        let gateway = secret_gateway(format!("http://{addr}"));
        let url = format!("http://{addr}/file/bot{SECRET_BOT_TOKEN}/voice.ogg");
        let response = TelegramGateway::send_request(gateway.client.get(url))
            .await
            .unwrap();
        let error = TelegramGateway::response_bytes(response).await.unwrap_err();

        assert_log_fields_are_redacted(&error, "error decoding response body");
        assert!(error.is_decode());
        assert!(error.url().is_none());
    }

    #[tokio::test]
    async fn send_text_uses_rich_markdown() {
        let server = MockServer::start().await;
        let markdown = "| Tool | Status |\n|---|---|\n| **MCP** | `ready` |";

        Mock::given(method("POST"))
            .and(path("/bottest-token/sendRichMessage"))
            .and(body_json(serde_json::json!({
                "chat_id": 123,
                "rich_message": { "markdown": markdown },
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {},
            })))
            .expect(1)
            .mount(&server)
            .await;

        test_gateway(server.uri())
            .send_text(123, markdown)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn send_text_falls_back_from_rejected_rich_markdown_chunk() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/bottest-token/sendRichMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false,
                "description": "invalid rich markdown",
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/bottest-token/sendMessage"))
            .and(body_json(serde_json::json!({
                "chat_id": 123,
                "text": "broken **markdown",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {},
            })))
            .expect(1)
            .mount(&server)
            .await;

        test_gateway(server.uri())
            .send_text(123, "broken **markdown")
            .await
            .unwrap();
    }

    #[test]
    fn split_short_message() {
        let chunks = split_message("hello world", 4096);
        assert_eq!(chunks, vec!["hello world"]);
    }

    #[test]
    fn split_at_newline() {
        let text = format!("{}\n{}", "a".repeat(4000), "b".repeat(200));
        let chunks = split_message(&text, 4096);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 4001);
        assert_eq!(chunks[1].len(), 200);
    }

    #[test]
    fn split_at_space() {
        let text = format!("{} {}", "a".repeat(4000), "b".repeat(200));
        let chunks = split_message(&text, 4096);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 4001);
        assert_eq!(chunks[1].len(), 200);
    }

    #[test]
    fn split_no_boundary() {
        let text = "a".repeat(5000);
        let chunks = split_message(&text, 4096);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 4096);
        assert_eq!(chunks[1].len(), 904);
    }

    #[test]
    fn split_exact_boundary() {
        let text = "a".repeat(4096);
        let chunks = split_message(&text, 4096);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn split_empty() {
        let chunks = split_message("", 4096);
        assert_eq!(chunks, vec![""]);
    }

    #[test]
    fn split_multiple_chunks() {
        let text = format!(
            "{}\n{}\n{}",
            "a".repeat(4000),
            "b".repeat(4000),
            "c".repeat(4000)
        );
        let chunks = split_message(&text, 4096);
        assert_eq!(chunks.len(), 3);
    }

    #[test]
    fn split_multibyte_chars() {
        let text = "🦆".repeat(1025); // 4100 bytes
        let chunks = split_message(&text, 4096);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chars().count(), 1024);
        assert_eq!(chunks[1].chars().count(), 1);
    }

    #[test]
    fn voice_info_from_voice_message() {
        let msg = TelegramMessage {
            message_id: 1,
            from: None,
            chat: TelegramChat {
                id: 123,
                chat_type: "private".into(),
            },
            text: None,
            voice: Some(TelegramVoice {
                file_id: "voice_file_123".into(),
                duration: Some(5),
                mime_type: Some("audio/ogg".into()),
                file_size: Some(10000),
            }),
            audio: None,
        };
        let info = TelegramGateway::voice_info(&msg);
        assert!(info.is_some());
        let v = info.unwrap();
        assert_eq!(v.file_id, "voice_file_123");
        assert_eq!(v.file_size, Some(10000));
        assert_eq!(v.duration, Some(5));
        assert_eq!(v.mime_type, Some("audio/ogg"));
    }

    #[test]
    fn voice_info_from_audio_message() {
        let msg = TelegramMessage {
            message_id: 1,
            from: None,
            chat: TelegramChat {
                id: 123,
                chat_type: "private".into(),
            },
            text: None,
            voice: None,
            audio: Some(TelegramAudio {
                file_id: "audio_file_456".into(),
                duration: Some(120),
                mime_type: Some("audio/mpeg".into()),
                file_size: Some(500_000),
            }),
        };
        let info = TelegramGateway::voice_info(&msg);
        assert!(info.is_some());
        let v = info.unwrap();
        assert_eq!(v.file_id, "audio_file_456");
        assert_eq!(v.duration, Some(120));
        assert_eq!(v.mime_type, Some("audio/mpeg"));
    }

    #[test]
    fn voice_info_none_for_text() {
        let msg = TelegramMessage {
            message_id: 1,
            from: None,
            chat: TelegramChat {
                id: 123,
                chat_type: "private".into(),
            },
            text: Some("hello".into()),
            voice: None,
            audio: None,
        };
        assert!(TelegramGateway::voice_info(&msg).is_none());
    }

    #[test]
    fn voice_prefers_voice_over_audio() {
        let msg = TelegramMessage {
            message_id: 1,
            from: None,
            chat: TelegramChat {
                id: 123,
                chat_type: "private".into(),
            },
            text: None,
            voice: Some(TelegramVoice {
                file_id: "voice_wins".into(),
                duration: Some(3),
                mime_type: None,
                file_size: None,
            }),
            audio: Some(TelegramAudio {
                file_id: "audio_loses".into(),
                duration: Some(60),
                mime_type: None,
                file_size: None,
            }),
        };
        let v = TelegramGateway::voice_info(&msg).unwrap();
        assert_eq!(v.file_id, "voice_wins");
    }

    #[test]
    fn voice_prompt_includes_path_and_duration() {
        let path = std::path::PathBuf::from("/tmp/goose_voice/voice_test.ogg");
        let prompt = TelegramGateway::voice_prompt(&path, Some(10), Some("audio/ogg"));
        assert!(prompt.contains("/tmp/goose_voice/voice_test.ogg"));
        assert!(prompt.contains("(duration: 10s)"));
        assert!(prompt.contains("audio/ogg"));
        assert!(prompt.contains("transcribe"));
    }

    #[test]
    fn voice_prompt_without_duration() {
        let path = std::path::PathBuf::from("/tmp/goose_voice/voice_test.ogg");
        let prompt = TelegramGateway::voice_prompt(&path, None, None);
        assert!(!prompt.contains("duration"));
        assert!(prompt.contains("/tmp/goose_voice/voice_test.ogg"));
    }

    #[test]
    fn voice_prompt_with_mp3_mime() {
        let path = std::path::PathBuf::from("/tmp/goose_voice/voice_test.mp3");
        let prompt = TelegramGateway::voice_prompt(&path, Some(60), Some("audio/mpeg"));
        assert!(prompt.contains("audio/mpeg"));
        assert!(!prompt.contains("OGG"));
    }

    #[test]
    fn save_voice_file_creates_file_ogg() {
        let gateway = test_gateway(TELEGRAM_API_BASE.to_string());
        let bytes = b"fake ogg data";
        let path = gateway.save_voice_file(bytes, Some("audio/ogg")).unwrap();
        assert!(path.exists());
        assert!(path.to_str().unwrap().ends_with(".ogg"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn save_voice_file_creates_file_mp3() {
        let gateway = test_gateway(TELEGRAM_API_BASE.to_string());
        let bytes = b"fake mp3 data";
        let path = gateway.save_voice_file(bytes, Some("audio/mpeg")).unwrap();
        assert!(path.exists());
        assert!(path.to_str().unwrap().ends_with(".mp3"));
    }

    #[test]
    fn save_voice_file_defaults_to_ogg() {
        let gateway = test_gateway(TELEGRAM_API_BASE.to_string());
        let bytes = b"unknown format";
        let path = gateway.save_voice_file(bytes, None).unwrap();
        assert!(path.to_str().unwrap().ends_with(".ogg"));
    }

    #[test]
    fn voice_file_extension_preserves_safe_audio_formats() {
        let cases = [
            (Some("audio/mpeg"), "mp3"),
            (Some("audio/mp4"), "m4a"),
            (Some("audio/x-m4a"), "m4a"),
            (Some("audio/ogg; codecs=opus"), "ogg"),
            (Some("audio/x-wav"), "wav"),
            (Some("audio/vnd.wave"), "wav"),
            (Some("audio/flac"), "flac"),
            (Some("audio/WEBM"), "webm"),
            (Some("Audio/MPEG"), "mp3"),
            (Some("AUDIO/WEBM"), "webm"),
        ];

        for (mime_type, expected) in cases {
            assert_eq!(TelegramGateway::voice_file_extension(mime_type), expected);
        }
    }

    #[test]
    fn voice_file_extension_rejects_filename_syntax() {
        let invalid = [
            None,
            Some("application/ogg"),
            Some("audio/..\\..\\outside"),
            Some("audio/../../outside"),
            Some("audio/ogg:stream"),
            Some("audio/ogg.stream"),
            Some("audio/ogg\nnext"),
            Some("audio/ogg; touch=outside"),
            Some("audio/ogg$HOME"),
            Some("audio/åudio"),
            Some("audio/this-subtype-is-too-long"),
        ];

        for mime_type in invalid {
            assert_eq!(TelegramGateway::voice_file_extension(mime_type), "ogg");
        }
    }

    #[test]
    fn save_voice_file_contains_untrusted_mime_before_pairing() {
        let temp = tempfile::tempdir().unwrap();
        let gateway = gateway_with_voice_temp_files(VoiceTempFiles::new_in(temp.path()));
        let bytes = b"unpaired voice data";
        let path = gateway
            .save_voice_file(bytes, Some("audio/..\\..\\outside"))
            .unwrap();

        assert_eq!(path.parent(), Some(gateway.voice_temp_files.parent()));
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(filename.starts_with("goose_voice_"));
        assert!(filename.ends_with(".ogg"));
        assert!(!filename.chars().any(|c| matches!(c, '/' | '\\' | ':')));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn cleanup_handles_legitimate_voice_files() {
        let temp = tempfile::tempdir().unwrap();
        let gateway = gateway_with_voice_temp_files(VoiceTempFiles::new_in(temp.path()));
        let recent_file = gateway
            .save_voice_file(b"recent", Some("audio/ogg"))
            .unwrap();
        assert_eq!(
            gateway
                .voice_temp_files
                .cleanup(std::time::Duration::from_secs(3600))
                .unwrap(),
            0
        );
        assert!(recent_file.exists());

        std::thread::sleep(std::time::Duration::from_millis(10));
        assert_eq!(
            gateway
                .voice_temp_files
                .cleanup(std::time::Duration::ZERO)
                .unwrap(),
            1
        );
        assert!(!recent_file.exists());
    }

    #[cfg(unix)]
    #[test]
    fn saved_voice_files_do_not_retain_open_descriptors() {
        let temp = tempfile::tempdir().unwrap();
        let gateway = gateway_with_voice_temp_files(VoiceTempFiles::new_in(temp.path()));
        let paths: std::collections::HashSet<PathBuf> = (0..32)
            .map(|_| gateway.save_voice_file(b"voice", None).unwrap())
            .collect();
        let descriptor_dir = if std::path::Path::new("/proc/self/fd").exists() {
            std::path::Path::new("/proc/self/fd")
        } else {
            std::path::Path::new("/dev/fd")
        };

        let retained = std::fs::read_dir(descriptor_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read_link(entry.path()).ok())
            .any(|target| paths.contains(&target));

        assert!(!retained);
        assert_eq!(gateway.voice_temp_files.files.lock().unwrap().len(), 32);
    }

    #[cfg(windows)]
    #[test]
    fn saved_voice_files_do_not_retain_open_descriptors() {
        let temp = tempfile::tempdir().unwrap();
        let gateway = gateway_with_voice_temp_files(VoiceTempFiles::new_in(temp.path()));
        let path = gateway.save_voice_file(b"voice", None).unwrap();

        std::fs::remove_file(&path).expect("closed voice files must be removable on Windows");
    }

    #[test]
    fn cleanup_reclaims_stale_voice_file_from_previous_process() {
        let temp = tempfile::tempdir().unwrap();
        let producer = VoiceTempFiles::new_in(temp.path());
        let path = producer.save(b"orphaned voice", "ogg").unwrap();
        std::mem::forget(producer);
        let cleaner = VoiceTempFiles::new_in(temp.path());
        assert_eq!(
            cleaner
                .cleanup(std::time::Duration::from_secs(3600))
                .unwrap(),
            0
        );
        assert!(path.exists());

        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();

        assert_eq!(
            cleaner
                .cleanup(std::time::Duration::from_secs(3600))
                .unwrap(),
            1
        );
        assert!(!path.exists());
    }

    #[test]
    fn cleanup_reclaims_only_stale_legacy_voice_files() {
        let temp = tempfile::tempdir().unwrap();
        let legacy_root = temp.path().join("goose_voice");
        std::fs::create_dir(&legacy_root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(&legacy_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let stale = legacy_root.join("voice_01234567-89ab-cdef-0123-456789abcdef.ogg");
        let recent = legacy_root.join("voice_abcdef01-2345-6789-abcd-ef0123456789.mp3");
        let unrelated = legacy_root.join("notes.txt");
        std::fs::write(&stale, b"stale voice").unwrap();
        std::fs::write(&recent, b"recent voice").unwrap();
        std::fs::write(&unrelated, b"unrelated").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(&stale, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::fs::set_permissions(&recent, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();

        let cleaner = VoiceTempFiles::new_in(temp.path());
        assert_eq!(
            cleaner
                .cleanup(std::time::Duration::from_secs(3600))
                .unwrap(),
            1
        );
        assert!(!stale.exists());
        assert_eq!(std::fs::read(&recent).unwrap(), b"recent voice");
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"unrelated");
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_legacy_root_replacement_stays_anchored() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let legacy_root = temp.path().join("goose_voice");
        let moved_root = temp.path().join("moved-goose-voice");
        let filename = "voice_01234567-89ab-cdef-0123-456789abcdef.ogg";
        std::fs::create_dir(&legacy_root).unwrap();
        std::fs::set_permissions(&legacy_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let original = legacy_root.join(filename);
        std::fs::write(&original, b"legacy voice").unwrap();
        std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o600)).unwrap();
        let cleaner = VoiceTempFiles::new_in(temp.path());
        let mut replaced = false;

        let removed = cleaner
            .cleanup_with_hook(std::time::Duration::ZERO, |candidate| {
                if candidate == original && !replaced {
                    std::fs::rename(&legacy_root, &moved_root).unwrap();
                    std::fs::create_dir(&legacy_root).unwrap();
                    std::fs::set_permissions(&legacy_root, std::fs::Permissions::from_mode(0o700))
                        .unwrap();
                    let replacement = legacy_root.join(filename);
                    std::fs::write(&replacement, b"replacement").unwrap();
                    std::fs::set_permissions(&replacement, std::fs::Permissions::from_mode(0o600))
                        .unwrap();
                    replaced = true;
                }
            })
            .unwrap();

        assert_eq!(removed, 1);
        assert!(replaced);
        assert!(!moved_root.join(filename).exists());
        assert_eq!(
            std::fs::read(legacy_root.join(filename)).unwrap(),
            b"replacement"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_preserves_symlinks_and_unrelated_files() {
        let temp = tempfile::tempdir().unwrap();
        let unrelated = temp.path().join("unrelated.txt");
        let victim = temp.path().join("victim.txt");
        let disguised_symlink = temp.path().join("goose_voice_ABC123.ogg");
        std::fs::write(&unrelated, b"unrelated").unwrap();
        std::fs::write(&victim, b"victim").unwrap();
        std::os::unix::fs::symlink(&victim, &disguised_symlink).unwrap();

        let cleaner = VoiceTempFiles::new_in(temp.path());
        assert_eq!(cleaner.cleanup(std::time::Duration::ZERO).unwrap(), 0);
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"unrelated");
        assert_eq!(std::fs::read(&victim).unwrap(), b"victim");
        assert!(disguised_symlink.symlink_metadata().is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_preserves_replacement_after_candidate_open() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let producer = VoiceTempFiles::new_in(temp.path());
        let path = producer.save(b"original voice", "ogg").unwrap();
        let moved = temp.path().join("moved-original.ogg");
        std::mem::forget(producer);
        let cleaner = VoiceTempFiles::new_in(temp.path());
        let mut replaced = false;

        let removed = cleaner
            .cleanup_with_hook(std::time::Duration::ZERO, |candidate| {
                if candidate == path && !replaced {
                    std::fs::rename(&path, &moved).unwrap();
                    std::fs::write(&path, b"replacement").unwrap();
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                        .unwrap();
                    replaced = true;
                }
            })
            .unwrap();

        assert_eq!(removed, 0);
        assert!(replaced);
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
        assert_eq!(std::fs::read(&moved).unwrap(), b"original voice");
    }

    #[cfg(unix)]
    #[test]
    fn precreated_legacy_root_cannot_redirect_voice_save_or_cleanup() {
        use std::os::unix::fs::PermissionsExt;

        let sandbox = tempfile::tempdir().unwrap();
        let fake_tmp = sandbox.path().join("tmp");
        let victim_dir = sandbox.path().join("victim");
        std::fs::create_dir(&fake_tmp).unwrap();
        std::fs::create_dir(&victim_dir).unwrap();
        let victim_file = victim_dir.join("voice_01234567-89ab-cdef-0123-456789abcdef.ogg");
        std::fs::write(&victim_file, b"keep me").unwrap();
        std::fs::set_permissions(&victim_file, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&victim_dir, fake_tmp.join("goose_voice")).unwrap();
        let gateway = gateway_with_voice_temp_files(VoiceTempFiles::new_in(&fake_tmp));

        let saved = gateway
            .save_voice_file(b"voice", Some("audio/ogg"))
            .unwrap();
        assert_eq!(saved.parent(), Some(fake_tmp.as_path()));
        assert_ne!(saved.parent(), Some(victim_dir.as_path()));
        gateway
            .voice_temp_files
            .cleanup(std::time::Duration::ZERO)
            .unwrap();

        assert!(victim_file.exists());
    }

    #[cfg(windows)]
    #[test]
    fn replaced_legacy_root_cannot_redirect_voice_save_or_cleanup() {
        let sandbox = tempfile::tempdir().unwrap();
        let fake_tmp = sandbox.path().join("tmp");
        let victim_dir = sandbox.path().join("victim");
        std::fs::create_dir(&fake_tmp).unwrap();
        std::fs::create_dir(&victim_dir).unwrap();
        let victim_file = victim_dir.join("unrelated.txt");
        std::fs::write(&victim_file, b"keep me").unwrap();
        let replaced_root = fake_tmp.join("goose_voice");
        std::fs::create_dir(&replaced_root).unwrap();
        std::fs::write(replaced_root.join("attacker-controlled.txt"), b"keep me").unwrap();
        let gateway = gateway_with_voice_temp_files(VoiceTempFiles::new_in(&fake_tmp));

        let saved = gateway
            .save_voice_file(b"voice", Some("audio/ogg"))
            .unwrap();
        assert_eq!(saved.parent(), Some(fake_tmp.as_path()));
        gateway
            .voice_temp_files
            .cleanup(std::time::Duration::ZERO)
            .unwrap();

        assert_eq!(std::fs::read(&victim_file).unwrap(), b"keep me");
        assert_eq!(
            std::fs::read(replaced_root.join("attacker-controlled.txt")).unwrap(),
            b"keep me"
        );
    }

    #[test]
    fn text_gateway_starts_when_voice_storage_is_unavailable() {
        let sandbox = tempfile::tempdir().unwrap();
        let missing_parent = sandbox.path().join("missing");
        let config = GatewayConfig {
            gateway_type: "telegram".to_string(),
            platform_config: serde_json::json!({"bot_token": "test-token"}),
            max_sessions: 1,
        };

        let gateway = TelegramGateway::new_with_voice_temp_parent(&config, missing_parent.clone())
            .expect("text-only startup must not access voice storage");
        assert!(!missing_parent.exists());
        assert!(gateway
            .save_voice_file(b"voice", Some("audio/ogg"))
            .is_err());
    }

    #[test]
    fn split_preserves_content() {
        let text = format!(
            "{} {} {}",
            "a".repeat(3000),
            "b".repeat(3000),
            "c".repeat(3000)
        );
        let chunks = split_message(&text, 4096);
        let reassembled: String = chunks.join("");
        assert_eq!(reassembled, text);
    }
}
