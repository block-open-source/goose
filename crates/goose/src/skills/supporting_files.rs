use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

const LOADED_FILE_PREFIX: &str = "# Loaded: ";
const LOADED_FILE_SEPARATOR: &str = "\n\n";
const LOADED_FILE_SUFFIX: &str = "\n\n---\nFile loaded into context.";
const MAX_SOURCE_FILE_BYTES: usize = crate::scheduler::MAX_SCHEDULE_RECIPE_BYTES as usize;

#[derive(Clone, Copy)]
enum ReadLimit {
    Characters(usize),
    Bytes(usize),
}

#[derive(Clone, Copy)]
enum RootLinkPolicy {
    Reject,
    FollowFinal,
}

#[derive(Clone, Copy)]
struct WalkPolicy {
    allow_linked_skill_roots: bool,
    inside_linked_skill_root: bool,
}

pub(super) struct OpenedSkillDirectory(fs::File);

impl OpenedSkillDirectory {
    pub(super) fn from_opened(directory: &fs::File) -> io::Result<Self> {
        directory.try_clone().map(Self)
    }
}

pub(crate) fn load_supporting_file(
    skill_dir: &Path,
    relative: &Path,
    skill_name: &str,
    linked_skill_root: bool,
) -> io::Result<String> {
    load_supporting_file_with_limit(
        skill_dir,
        relative,
        skill_name,
        crate::agents::max_tool_response_size(),
        linked_skill_root,
    )
}

pub(super) fn load_supporting_file_from_opened(
    skill_dir: &OpenedSkillDirectory,
    relative: &Path,
    skill_name: &str,
) -> io::Result<String> {
    let max_characters = crate::agents::max_tool_response_size();
    let wrapper_characters = LOADED_FILE_PREFIX.chars().count()
        + skill_name.chars().count()
        + LOADED_FILE_SEPARATOR.chars().count()
        + LOADED_FILE_SUFFIX.chars().count();
    let content_limit = max_characters
        .checked_sub(wrapper_characters)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "maximum tool response size of {max_characters} characters is too small to load '{skill_name}'"
                ),
            )
        })?;
    let content =
        read_confined_file_from_opened(skill_dir, relative, ReadLimit::Characters(content_limit))?;
    Ok(format!(
        "{LOADED_FILE_PREFIX}{skill_name}{LOADED_FILE_SEPARATOR}{content}{LOADED_FILE_SUFFIX}"
    ))
}

fn load_supporting_file_with_limit(
    skill_dir: &Path,
    relative: &Path,
    skill_name: &str,
    max_characters: usize,
    linked_skill_root: bool,
) -> io::Result<String> {
    let wrapper_characters = LOADED_FILE_PREFIX.chars().count()
        + skill_name.chars().count()
        + LOADED_FILE_SEPARATOR.chars().count()
        + LOADED_FILE_SUFFIX.chars().count();
    let content_limit = max_characters
        .checked_sub(wrapper_characters)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "maximum tool response size of {max_characters} characters is too small to load '{skill_name}'"
                ),
            )
        })?;
    let root_link_policy = if linked_skill_root {
        RootLinkPolicy::FollowFinal
    } else {
        RootLinkPolicy::Reject
    };
    let content =
        read_supporting_file_with_limit(skill_dir, relative, content_limit, root_link_policy)?;
    Ok(format!(
        "{LOADED_FILE_PREFIX}{skill_name}{LOADED_FILE_SEPARATOR}{content}{LOADED_FILE_SUFFIX}"
    ))
}

fn read_supporting_file_with_limit(
    skill_dir: &Path,
    relative: &Path,
    max_characters: usize,
    root_link_policy: RootLinkPolicy,
) -> io::Result<String> {
    read_supporting_file_with_hook(
        skill_dir,
        relative,
        max_characters,
        root_link_policy,
        |_| {},
    )
}

pub(crate) fn read_source_file(source_dir: &Path, relative: &Path) -> io::Result<String> {
    read_confined_file_with_hook(
        source_dir,
        relative,
        ReadLimit::Bytes(MAX_SOURCE_FILE_BYTES),
        RootLinkPolicy::Reject,
        |_| {},
    )
}

pub(crate) fn write_source_file(
    source_dir: &Path,
    relative: &Path,
    content: &[u8],
) -> io::Result<()> {
    write_confined_file_with_hook(source_dir, relative, content, false, |_| {})
}

pub(crate) fn create_source_file(
    source_dir: &Path,
    relative: &Path,
    content: &[u8],
) -> io::Result<()> {
    write_confined_file_with_hook(source_dir, relative, content, true, |_| {})
}

fn read_supporting_file_with_hook(
    skill_dir: &Path,
    relative: &Path,
    max_characters: usize,
    root_link_policy: RootLinkPolicy,
    after_opened_component: impl FnMut(&Path),
) -> io::Result<String> {
    read_confined_file_with_hook(
        skill_dir,
        relative,
        ReadLimit::Characters(max_characters),
        root_link_policy,
        after_opened_component,
    )
}

fn max_utf8_bytes(max_characters: usize) -> io::Result<usize> {
    max_characters.checked_mul(4).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "configured supporting file size limit is too large",
        )
    })
}

fn read_utf8_with_limit(mut reader: impl io::Read, max_characters: usize) -> io::Result<String> {
    let max_bytes = max_utf8_bytes(max_characters)?;
    let read_size = max_bytes.checked_add(1).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "configured supporting file size limit is too large",
        )
    })?;
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(read_size as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(file_encoding_too_large(max_bytes));
    }
    let content = String::from_utf8(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if content.chars().count() > max_characters {
        return Err(file_too_large(max_characters));
    }
    Ok(content)
}

fn read_utf8_with_byte_limit(mut reader: impl io::Read, max_bytes: usize) -> io::Result<String> {
    let read_size = max_bytes.checked_add(1).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "configured source file size limit is too large",
        )
    })?;
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(read_size as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(file_encoding_too_large(max_bytes));
    }
    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn file_too_large(max_characters: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("supporting file exceeds the maximum size of {max_characters} characters"),
    )
}

fn file_encoding_too_large(max_bytes: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("supporting file exceeds the maximum encoded size of {max_bytes} bytes"),
    )
}

fn read_opened_file(file: fs::File, limit: ReadLimit) -> io::Result<String> {
    let max_bytes = match limit {
        ReadLimit::Characters(max_characters) => max_utf8_bytes(max_characters)?,
        ReadLimit::Bytes(max_bytes) => max_bytes,
    };
    if file.metadata()?.len() > max_bytes as u64 {
        return Err(file_encoding_too_large(max_bytes));
    }
    match limit {
        ReadLimit::Characters(max_characters) => read_utf8_with_limit(file, max_characters),
        ReadLimit::Bytes(max_bytes) => read_utf8_with_byte_limit(file, max_bytes),
    }
}

pub(super) fn walk_regular_files_no_follow_with_hook<F, G, H>(
    root: &Path,
    linked_skill_root: bool,
    should_descend: &mut G,
    visit_file: &mut F,
    after_read_dir: &mut H,
) -> io::Result<()>
where
    F: FnMut(&Path, bool, &fs::File, &mut dyn FnMut() -> io::Result<fs::File>),
    G: FnMut(&Path) -> bool,
    H: FnMut(&Path),
{
    let root_link_policy = if linked_skill_root {
        RootLinkPolicy::FollowFinal
    } else {
        RootLinkPolicy::Reject
    };
    walk_regular_files_no_follow_impl(
        root,
        root_link_policy,
        false,
        should_descend,
        visit_file,
        after_read_dir,
    )
}

pub(super) fn walk_regular_files_from_opened_with_hook<F, G, H>(
    root: &Path,
    directory: &OpenedSkillDirectory,
    should_descend: &mut G,
    visit_file: &mut F,
    after_read_dir: &mut H,
) -> io::Result<()>
where
    F: FnMut(&Path, bool, &fs::File, &mut dyn FnMut() -> io::Result<fs::File>),
    G: FnMut(&Path) -> bool,
    H: FnMut(&Path),
{
    walk_opened_directory(
        root,
        directory.0.try_clone()?,
        WalkPolicy {
            allow_linked_skill_roots: false,
            inside_linked_skill_root: true,
        },
        should_descend,
        visit_file,
        after_read_dir,
    );
    Ok(())
}

pub(super) fn walk_skill_files_no_follow_with_hook<F, G, H>(
    root: &Path,
    should_descend: &mut G,
    visit_file: &mut F,
    after_read_dir: &mut H,
) -> io::Result<()>
where
    F: FnMut(&Path, bool, &fs::File, &mut dyn FnMut() -> io::Result<fs::File>),
    G: FnMut(&Path) -> bool,
    H: FnMut(&Path),
{
    walk_regular_files_no_follow_impl(
        root,
        RootLinkPolicy::Reject,
        true,
        should_descend,
        visit_file,
        after_read_dir,
    )
}

fn walk_regular_files_no_follow_impl<F, G, H>(
    root: &Path,
    root_link_policy: RootLinkPolicy,
    allow_linked_skill_roots: bool,
    should_descend: &mut G,
    visit_file: &mut F,
    after_read_dir: &mut H,
) -> io::Result<()>
where
    F: FnMut(&Path, bool, &fs::File, &mut dyn FnMut() -> io::Result<fs::File>),
    G: FnMut(&Path) -> bool,
    H: FnMut(&Path),
{
    let root = normalize_absolute_path(root)?;
    let directory = open_skill_root(&root, root_link_policy, &mut |_| {})?;
    walk_opened_directory(
        &root,
        directory,
        WalkPolicy {
            allow_linked_skill_roots,
            inside_linked_skill_root: false,
        },
        should_descend,
        visit_file,
        after_read_dir,
    );
    Ok(())
}

fn normalize_absolute_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() || !normalized.has_root() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "skill path must stay within its filesystem root",
                    ));
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    if !normalized.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "skill path must resolve to an absolute path",
        ));
    }
    Ok(normalized)
}

fn walk_opened_directory<F, G, H>(
    logical_path: &Path,
    directory: fs::File,
    policy: WalkPolicy,
    should_descend: &mut G,
    visit_file: &mut F,
    after_read_dir: &mut H,
) where
    F: FnMut(&Path, bool, &fs::File, &mut dyn FnMut() -> io::Result<fs::File>),
    G: FnMut(&Path) -> bool,
    H: FnMut(&Path),
{
    let entries = match read_directory_names(&directory) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    after_read_dir(logical_path);

    for name in entries {
        let path = logical_path.join(&name);
        if should_descend(&path) {
            if let Ok(child) = open_child_directory(&directory, &name) {
                let child_is_skill_root =
                    is_child_regular_file(&child, std::ffi::OsStr::new("SKILL.md"))
                        .unwrap_or(false);
                walk_opened_directory(
                    &path,
                    child,
                    WalkPolicy {
                        allow_linked_skill_roots: policy.allow_linked_skill_roots
                            && !child_is_skill_root,
                        ..policy
                    },
                    should_descend,
                    visit_file,
                    after_read_dir,
                );
                continue;
            }
            if policy.allow_linked_skill_roots {
                if let Ok(child) = open_child_linked_skill_directory(&directory, &name) {
                    walk_opened_directory(
                        &path,
                        child,
                        WalkPolicy {
                            allow_linked_skill_roots: false,
                            inside_linked_skill_root: true,
                        },
                        should_descend,
                        visit_file,
                        after_read_dir,
                    );
                    continue;
                }
            }
        }
        if is_child_regular_file(&directory, &name).unwrap_or(false) {
            let mut open_for_read = || open_child_regular_file(&directory, &name);
            visit_file(
                &path,
                policy.inside_linked_skill_root,
                &directory,
                &mut open_for_read,
            );
        }
    }
}

fn read_confined_file_from_opened(
    skill_dir: &OpenedSkillDirectory,
    relative: &Path,
    limit: ReadLimit,
) -> io::Result<String> {
    let components = validated_relative_components(relative)?;
    let (file_name, ancestors) = components.split_last().unwrap();
    let mut directory = skill_dir.0.try_clone()?;
    for ancestor in ancestors {
        directory = open_child_directory(&directory, ancestor)?;
    }
    read_opened_file(open_child_regular_file(&directory, file_name)?, limit)
}

#[cfg(unix)]
fn read_directory_names(directory: &fs::File) -> io::Result<Vec<std::ffi::OsString>> {
    use std::ffi::{CStr, OsString};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStringExt;

    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            c".".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    let stream = unsafe { libc::fdopendir(descriptor) };
    if stream.is_null() {
        unsafe { libc::close(descriptor) };
        return Err(io::Error::last_os_error());
    }

    let mut names = Vec::new();
    loop {
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            names.push(OsString::from_vec(name.to_vec()));
        }
    }
    if unsafe { libc::closedir(stream) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(names)
}

#[cfg(windows)]
fn read_directory_names(directory: &fs::File) -> io::Result<Vec<std::ffi::OsString>> {
    use ntapi::ntioapi::{
        FileNamesInformation, NtQueryDirectoryFile, FILE_NAMES_INFORMATION, IO_STATUS_BLOCK,
    };
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use winapi::shared::ntdef::{HANDLE, NT_SUCCESS};
    use winapi::shared::ntstatus::STATUS_NO_MORE_FILES;

    let mut names = Vec::new();
    let mut buffer = vec![0_u64; 8 * 1024];
    let buffer_bytes = std::mem::size_of_val(buffer.as_slice());
    let mut restart_scan = 1;
    loop {
        let mut io_status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
        let status = unsafe {
            NtQueryDirectoryFile(
                directory.as_raw_handle() as HANDLE,
                std::ptr::null_mut(),
                None,
                std::ptr::null_mut(),
                &mut io_status,
                buffer.as_mut_ptr().cast(),
                buffer_bytes as u32,
                FileNamesInformation,
                0,
                std::ptr::null_mut(),
                restart_scan,
            )
        };
        if status == STATUS_NO_MORE_FILES {
            break;
        }
        if !NT_SUCCESS(status) {
            return Err(windows_nt_status_error(status));
        }
        restart_scan = 0;

        let mut offset = 0_usize;
        loop {
            let entry = unsafe {
                &*buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<FILE_NAMES_INFORMATION>()
            };
            let name_bytes = entry.FileNameLength as usize;
            if name_bytes % std::mem::size_of::<u16>() != 0
                || offset
                    .checked_add(std::mem::offset_of!(FILE_NAMES_INFORMATION, FileName))
                    .and_then(|start| start.checked_add(name_bytes))
                    .is_none_or(|end| end > buffer_bytes)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "directory query returned an invalid entry",
                ));
            }
            let name =
                unsafe { std::slice::from_raw_parts(entry.FileName.as_ptr(), name_bytes / 2) };
            if name != [b'.' as u16] && name != [b'.' as u16, b'.' as u16] {
                names.push(std::ffi::OsString::from_wide(name));
            }
            if entry.NextEntryOffset == 0 {
                break;
            }
            offset = offset
                .checked_add(entry.NextEntryOffset as usize)
                .filter(|offset| *offset < buffer_bytes)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "directory query returned an invalid entry offset",
                    )
                })?;
        }
    }
    Ok(names)
}

#[cfg(not(any(unix, windows)))]
fn read_directory_names(_directory: &fs::File) -> io::Result<Vec<std::ffi::OsString>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "secure skill discovery is not supported on this platform",
    ))
}

#[cfg(unix)]
fn is_child_regular_file(directory: &fs::File, name: &std::ffi::OsStr) -> io::Result<bool> {
    use std::os::fd::AsRawFd;

    let name = path_component_to_c_string(name)?;
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: metadata points to writable storage and fstatat does not retain the name pointer.
    let result = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatat initialized metadata on success.
    let metadata = unsafe { metadata.assume_init() };
    Ok(metadata.st_mode & libc::S_IFMT == libc::S_IFREG)
}

#[cfg(unix)]
fn open_child_directory(directory: &fs::File, name: &std::ffi::OsStr) -> io::Result<fs::File> {
    let child = open_at(directory, name, directory_traversal_flags())?;
    if !child.metadata()?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "skill entry is not a directory",
        ));
    }
    Ok(child)
}

#[cfg(unix)]
fn open_child_linked_skill_directory(
    directory: &fs::File,
    name: &std::ffi::OsStr,
) -> io::Result<fs::File> {
    use std::os::fd::AsRawFd;

    let encoded_name = path_component_to_c_string(name)?;
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: metadata points to writable storage and fstatat does not retain the name pointer.
    let result = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            encoded_name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatat initialized metadata on success.
    let metadata = unsafe { metadata.assume_init() };
    if metadata.st_mode & libc::S_IFMT != libc::S_IFLNK {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "skill entry is not a linked directory",
        ));
    }

    let child = open_at(directory, name, linked_directory_traversal_flags())?;
    if !child.metadata()?.is_dir()
        || !is_child_regular_file(&child, std::ffi::OsStr::new("SKILL.md"))?
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "linked skill entry is not a skill directory",
        ));
    }
    Ok(child)
}

#[cfg(unix)]
fn open_child_regular_file(directory: &fs::File, name: &std::ffi::OsStr) -> io::Result<fs::File> {
    let file = open_at(
        directory,
        name,
        libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
    )?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "skill entry is not a regular file",
        ));
    }
    Ok(file)
}

#[cfg(windows)]
fn is_child_regular_file(directory: &fs::File, name: &std::ffi::OsStr) -> io::Result<bool> {
    let file = windows_open_at(directory, name, false, false, false)?;
    let metadata = file.metadata()?;
    Ok(!windows_metadata_is_reparse_point(&metadata) && metadata.is_file())
}

#[cfg(windows)]
fn open_child_directory(directory: &fs::File, name: &std::ffi::OsStr) -> io::Result<fs::File> {
    let child = windows_open_at(directory, name, true, false, false)?;
    let metadata = child.metadata()?;
    if windows_metadata_is_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "skill entry is not a regular directory",
        ));
    }
    Ok(child)
}

#[cfg(windows)]
fn open_child_linked_skill_directory(
    directory: &fs::File,
    name: &std::ffi::OsStr,
) -> io::Result<fs::File> {
    let linked = windows_open_at(directory, name, true, false, false)?;
    if !windows_metadata_is_reparse_point(&linked.metadata()?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "skill entry is not a linked directory",
        ));
    }

    let child = windows_open_at(directory, name, true, false, true)?;
    if !child.metadata()?.is_dir()
        || !is_child_regular_file(&child, std::ffi::OsStr::new("SKILL.md"))?
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "linked skill entry is not a skill directory",
        ));
    }
    Ok(child)
}

#[cfg(windows)]
fn open_child_regular_file(directory: &fs::File, name: &std::ffi::OsStr) -> io::Result<fs::File> {
    let file = windows_open_at(directory, name, false, true, false)?;
    let metadata = file.metadata()?;
    if windows_metadata_is_reparse_point(&metadata) || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "skill entry is not a regular file",
        ));
    }
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
fn is_child_regular_file(_directory: &fs::File, _name: &std::ffi::OsStr) -> io::Result<bool> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "secure skill discovery is not supported on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn open_child_directory(_directory: &fs::File, _name: &std::ffi::OsStr) -> io::Result<fs::File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "secure skill discovery is not supported on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn open_child_linked_skill_directory(
    _directory: &fs::File,
    _name: &std::ffi::OsStr,
) -> io::Result<fs::File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "secure linked skill discovery is not supported on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn open_child_regular_file(_directory: &fs::File, _name: &std::ffi::OsStr) -> io::Result<fs::File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "secure skill discovery is not supported on this platform",
    ))
}

fn validated_relative_components(path: &Path) -> io::Result<Vec<&std::ffi::OsStr>> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(component) => components.push(component),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "supporting file path must stay within the skill directory",
                ));
            }
        }
    }
    if components.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "supporting file path must name a file",
        ));
    }
    Ok(components)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn directory_traversal_flags() -> libc::c_int {
    libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
}

#[cfg(all(
    unix,
    any(
        target_vendor = "apple",
        target_os = "aix",
        target_os = "freebsd",
        target_os = "illumos",
        target_os = "netbsd",
        target_os = "solaris"
    )
))]
fn directory_traversal_flags() -> libc::c_int {
    libc::O_SEARCH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
}

#[cfg(all(
    unix,
    not(any(
        target_vendor = "apple",
        target_os = "aix",
        target_os = "android",
        target_os = "freebsd",
        target_os = "illumos",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "solaris"
    ))
))]
fn directory_traversal_flags() -> libc::c_int {
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
}

#[cfg(unix)]
fn linked_directory_traversal_flags() -> libc::c_int {
    directory_traversal_flags() & !libc::O_NOFOLLOW
}

#[cfg(unix)]
fn open_skill_root(
    skill_dir: &Path,
    root_link_policy: RootLinkPolicy,
    after_opened_component: &mut impl FnMut(&Path),
) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = fs::OpenOptions::new();
    options.read(true).custom_flags(directory_traversal_flags());
    let mut directory = options.open(Path::new("/"))?;
    let mut opened_path = std::path::PathBuf::from("/");
    let mut saw_root = false;
    for component in skill_dir.components() {
        match component {
            Component::RootDir if !saw_root => saw_root = true,
            Component::Normal(component) if saw_root => {
                let next_path = opened_path.join(component);
                let flags = if matches!(root_link_policy, RootLinkPolicy::FollowFinal)
                    && next_path == skill_dir
                {
                    linked_directory_traversal_flags()
                } else {
                    directory_traversal_flags()
                };
                directory = open_at(&directory, component, flags)?;
                opened_path = next_path;
                after_opened_component(&opened_path);
            }
            Component::CurDir if saw_root => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "skill path must be an absolute normalized path",
                ));
            }
        }
    }
    if !saw_root || opened_path != skill_dir {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "skill path must be an absolute normalized path",
        ));
    }
    if !directory.metadata()?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "skill path is not a directory",
        ));
    }
    Ok(directory)
}

#[cfg(unix)]
fn read_confined_file_with_hook(
    skill_dir: &Path,
    relative: &Path,
    limit: ReadLimit,
    root_link_policy: RootLinkPolicy,
    mut after_opened_component: impl FnMut(&Path),
) -> io::Result<String> {
    let components = validated_relative_components(relative)?;
    let (file_name, ancestors) = components.split_last().unwrap();
    let mut directory = open_skill_root(skill_dir, root_link_policy, &mut after_opened_component)?;

    let mut opened_path = std::path::PathBuf::new();
    for ancestor in ancestors {
        directory = open_at(&directory, ancestor, directory_traversal_flags())?;
        opened_path.push(ancestor);
        after_opened_component(&opened_path);
    }

    let file = open_at(
        &directory,
        file_name,
        libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
    )?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "supporting file is not a regular file",
        ));
    }

    read_opened_file(file, limit)
}

#[cfg(unix)]
fn write_confined_file_with_hook(
    source_dir: &Path,
    relative: &Path,
    content: &[u8],
    create_new: bool,
    mut after_opened_component: impl FnMut(&Path),
) -> io::Result<()> {
    let components = validated_relative_components(relative)?;
    let (file_name, ancestors) = components.split_last().unwrap();
    let mut directory = open_skill_root(
        source_dir,
        RootLinkPolicy::Reject,
        &mut after_opened_component,
    )?;

    let mut opened_path = std::path::PathBuf::new();
    for ancestor in ancestors {
        directory = open_at(&directory, ancestor, directory_traversal_flags())?;
        opened_path.push(ancestor);
        after_opened_component(&opened_path);
    }

    let mut flags =
        libc::O_WRONLY | libc::O_CREAT | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    if create_new {
        flags |= libc::O_EXCL;
    }
    let mut file = open_at_with_mode(&directory, file_name, flags, 0o666)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source path is not a regular file",
        ));
    }
    ensure_source_file_has_single_link(&file, &metadata)?;
    if !create_new {
        file.set_len(0)?;
    }
    file.write_all(content)
}

#[cfg(unix)]
fn ensure_source_file_has_single_link(_file: &fs::File, metadata: &fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    if metadata.nlink() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source path must have exactly one hard link",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn open_at(
    directory: &fs::File,
    name: &std::ffi::OsStr,
    flags: libc::c_int,
) -> io::Result<fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let name = path_component_to_c_string(name)?;
    // SAFETY: openat does not retain the name pointer, and no creation flag requiring a mode is set.
    let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a new owned descriptor on success.
    Ok(unsafe { fs::File::from_raw_fd(descriptor) })
}

#[cfg(unix)]
fn open_at_with_mode(
    directory: &fs::File,
    name: &std::ffi::OsStr,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> io::Result<fs::File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let name = CString::new(name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "source file path contains a NUL byte",
        )
    })?;
    // SAFETY: openat does not retain the name pointer, and mode is supplied for O_CREAT.
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags,
            libc::c_uint::from(mode),
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a new owned descriptor on success.
    Ok(unsafe { fs::File::from_raw_fd(descriptor) })
}

#[cfg(unix)]
fn path_component_to_c_string(name: &std::ffi::OsStr) -> io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;

    std::ffi::CString::new(name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "supporting file path contains a NUL byte",
        )
    })
}

#[cfg(windows)]
fn open_skill_root(
    skill_dir: &Path,
    root_link_policy: RootLinkPolicy,
    after_opened_component: &mut impl FnMut(&Path),
) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use winapi::um::winbase::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT};
    use winapi::um::winnt::{
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_TRAVERSE, SYNCHRONIZE,
    };

    let root_anchor = skill_dir
        .ancestors()
        .last()
        .filter(|path| path.has_root())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "skill path must be an absolute normalized path",
            )
        })?;
    let relative = skill_dir.strip_prefix(root_anchor).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "skill path must be an absolute normalized path",
        )
    })?;
    let components = if relative.as_os_str().is_empty() {
        Vec::new()
    } else {
        validated_relative_components(relative)?
    };

    let mut options = fs::OpenOptions::new();
    options
        .access_mode(FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    let mut directory = options.open(root_anchor)?;
    let root_metadata = directory.metadata()?;
    if windows_metadata_is_reparse_point(&root_metadata) || !root_metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "skill path is not a directory",
        ));
    }
    let mut opened_path = root_anchor.to_path_buf();
    for component in components {
        let next_path = opened_path.join(component);
        let follow_reparse_point =
            matches!(root_link_policy, RootLinkPolicy::FollowFinal) && next_path == skill_dir;
        directory = windows_open_at(&directory, component, true, false, follow_reparse_point)?;
        let metadata = directory.metadata()?;
        if (!follow_reparse_point && windows_metadata_is_reparse_point(&metadata))
            || !metadata.is_dir()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "skill path ancestor is not a regular directory",
            ));
        }
        opened_path = next_path;
        after_opened_component(&opened_path);
    }
    if opened_path != skill_dir {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "skill path must be an absolute normalized path",
        ));
    }
    Ok(directory)
}

#[cfg(windows)]
fn read_confined_file_with_hook(
    skill_dir: &Path,
    relative: &Path,
    limit: ReadLimit,
    root_link_policy: RootLinkPolicy,
    mut after_opened_component: impl FnMut(&Path),
) -> io::Result<String> {
    let components = validated_relative_components(relative)?;
    let (file_name, ancestors) = components.split_last().unwrap();
    let mut directory = open_skill_root(skill_dir, root_link_policy, &mut after_opened_component)?;

    let mut opened_path = std::path::PathBuf::new();
    for ancestor in ancestors {
        directory = windows_open_at(&directory, ancestor, true, false, false)?;
        let metadata = directory.metadata()?;
        if windows_metadata_is_reparse_point(&metadata) || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "supporting file path ancestor is not a regular directory",
            ));
        }
        opened_path.push(ancestor);
        after_opened_component(&opened_path);
    }

    let file = windows_open_at(&directory, file_name, false, true, false)?;
    let metadata = file.metadata()?;
    if windows_metadata_is_reparse_point(&metadata) || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "supporting file is not a regular file",
        ));
    }

    read_opened_file(file, limit)
}

#[cfg(windows)]
fn write_confined_file_with_hook(
    source_dir: &Path,
    relative: &Path,
    content: &[u8],
    create_new: bool,
    mut after_opened_component: impl FnMut(&Path),
) -> io::Result<()> {
    let components = validated_relative_components(relative)?;
    let (file_name, ancestors) = components.split_last().unwrap();
    let mut directory = open_skill_root(
        source_dir,
        RootLinkPolicy::Reject,
        &mut after_opened_component,
    )?;

    let mut opened_path = std::path::PathBuf::new();
    for ancestor in ancestors {
        directory = windows_open_at(&directory, ancestor, true, false, false)?;
        let metadata = directory.metadata()?;
        if windows_metadata_is_reparse_point(&metadata) || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "source path ancestor is not a regular directory",
            ));
        }
        opened_path.push(ancestor);
        after_opened_component(&opened_path);
    }

    let mut file = windows_open_file_at(&directory, file_name, create_new)?;
    let metadata = file.metadata()?;
    if windows_metadata_is_reparse_point(&metadata) || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source path is not a regular file",
        ));
    }
    ensure_source_file_has_single_link(&file, &metadata)?;
    if !create_new {
        file.set_len(0)?;
    }
    file.write_all(content)
}

#[cfg(windows)]
fn ensure_source_file_has_single_link(file: &fs::File, _metadata: &fs::Metadata) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use winapi::um::fileapi::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};

    // SAFETY: BY_HANDLE_FILE_INFORMATION is a plain C data structure initialized before the call.
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the file owns a valid handle and information points to writable initialized storage.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if information.nNumberOfLinks != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source path must have exactly one hard link",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn windows_open_at(
    directory: &fs::File,
    name: &std::ffi::OsStr,
    directory_only: bool,
    read_file: bool,
    follow_reparse_point: bool,
) -> io::Result<fs::File> {
    use ntapi::ntioapi::{
        FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
    };
    use winapi::um::winnt::{
        FILE_GENERIC_READ, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_TRAVERSE, SYNCHRONIZE,
    };

    let mut create_options = FILE_SYNCHRONOUS_IO_NONALERT;
    if !follow_reparse_point {
        create_options |= FILE_OPEN_REPARSE_POINT;
    }
    if directory_only {
        create_options |= FILE_DIRECTORY_FILE;
    }
    let desired_access = if directory_only {
        FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE
    } else if read_file {
        FILE_GENERIC_READ
    } else {
        FILE_READ_ATTRIBUTES | SYNCHRONIZE
    };
    windows_open_at_with_options(directory, name, desired_access, FILE_OPEN, create_options)
}

#[cfg(windows)]
fn windows_open_file_at(
    directory: &fs::File,
    name: &std::ffi::OsStr,
    create_new: bool,
) -> io::Result<fs::File> {
    use ntapi::ntioapi::{
        FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT,
        FILE_SYNCHRONOUS_IO_NONALERT,
    };
    use winapi::um::winnt::{FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES, SYNCHRONIZE};

    let create_disposition = if create_new {
        FILE_CREATE
    } else {
        FILE_OPEN_IF
    };
    windows_open_at_with_options(
        directory,
        name,
        FILE_GENERIC_WRITE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        create_disposition,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    )
}

#[cfg(windows)]
fn windows_open_at_with_options(
    directory: &fs::File,
    name: &std::ffi::OsStr,
    desired_access: u32,
    create_disposition: u32,
    create_options: u32,
) -> io::Result<fs::File> {
    use ntapi::ntioapi::{NtCreateFile, IO_STATUS_BLOCK};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use winapi::shared::ntdef::{
        HANDLE, NT_SUCCESS, OBJECT_ATTRIBUTES, OBJ_CASE_INSENSITIVE, UNICODE_STRING,
    };
    use winapi::um::winnt::{FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE};

    let mut name: Vec<u16> = name.encode_wide().collect();
    let name_bytes = name
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "supporting file path component is too long",
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
    // SAFETY: IO_STATUS_BLOCK is a plain C data structure initialized before the synchronous call.
    let mut io_status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
    // SAFETY: all pointers reference initialized values for the duration of the synchronous call.
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            desired_access,
            &mut attributes,
            &mut io_status,
            std::ptr::null_mut(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            create_disposition,
            create_options,
            std::ptr::null_mut(),
            0,
        )
    };
    if !NT_SUCCESS(status) {
        return Err(windows_nt_status_error(status));
    }
    // SAFETY: NtCreateFile returned a new owned handle on success.
    Ok(unsafe { fs::File::from_raw_handle(handle.cast()) })
}

#[cfg(windows)]
fn windows_metadata_is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use winapi::um::winnt::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(windows)]
fn windows_nt_status_error(status: winapi::shared::ntdef::NTSTATUS) -> io::Error {
    // SAFETY: RtlNtStatusToDosError accepts every NTSTATUS value.
    let error = unsafe { ntapi::ntrtl::RtlNtStatusToDosError(status) };
    io::Error::from_raw_os_error(error as i32)
}

#[cfg(not(any(unix, windows)))]
fn read_confined_file_with_hook(
    _skill_dir: &Path,
    relative: &Path,
    _limit: ReadLimit,
    _root_link_policy: RootLinkPolicy,
    _after_opened_component: impl FnMut(&Path),
) -> io::Result<String> {
    validated_relative_components(relative)?;
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "secure supporting file reads are not supported on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn write_confined_file_with_hook(
    _source_dir: &Path,
    relative: &Path,
    _content: &[u8],
    _create_new: bool,
    _after_opened_component: impl FnMut(&Path),
) -> io::Result<()> {
    validated_relative_components(relative)?;
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "secure source file writes are not supported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(unix, windows))]
    #[test]
    fn reads_nested_regular_file() {
        let root = tempfile::tempdir().unwrap();
        let skill_dir = fs::canonicalize(root.path()).unwrap();
        let nested = skill_dir.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("guide.md"), "nested guidance").unwrap();

        let content = read_supporting_file_with_limit(
            &skill_dir,
            Path::new("nested/guide.md"),
            crate::agents::max_tool_response_size(),
            RootLinkPolicy::Reject,
        )
        .unwrap();

        assert_eq!(content, "nested guidance");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn source_file_safety_limit_is_independent() {
        let root = tempfile::tempdir().unwrap();
        let source_dir = fs::canonicalize(root.path()).unwrap();
        fs::write(
            source_dir.join("source.md"),
            "x".repeat(MAX_SOURCE_FILE_BYTES + 1),
        )
        .unwrap();

        assert!(read_source_file(&source_dir, Path::new("source.md")).is_err());
    }

    #[cfg(all(
        unix,
        any(
            target_vendor = "apple",
            target_os = "aix",
            target_os = "android",
            target_os = "freebsd",
            target_os = "illumos",
            target_os = "linux",
            target_os = "netbsd",
            target_os = "solaris"
        )
    ))]
    #[test]
    fn reads_through_search_only_ancestor() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let skill_dir = root.path().join("skill");
        fs::create_dir(&skill_dir).unwrap();
        fs::write(skill_dir.join("guide.md"), "search-only guidance").unwrap();
        let skill_dir = fs::canonicalize(skill_dir).unwrap();
        let original_permissions = fs::metadata(root.path()).unwrap().permissions();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o111)).unwrap();

        let result = read_supporting_file_with_limit(
            &skill_dir,
            Path::new("guide.md"),
            crate::agents::max_tool_response_size(),
            RootLinkPolicy::Reject,
        );

        fs::set_permissions(root.path(), original_permissions).unwrap();
        assert_eq!(result.unwrap(), "search-only guidance");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn reads_utf8_file_at_exact_character_limit() {
        let root = tempfile::tempdir().unwrap();
        let skill_dir = fs::canonicalize(root.path()).unwrap();
        fs::write(skill_dir.join("guide.md"), "🙂🙂🙂🙂").unwrap();

        let content = read_supporting_file_with_limit(
            &skill_dir,
            Path::new("guide.md"),
            4,
            RootLinkPolicy::Reject,
        )
        .unwrap();

        assert_eq!(content, "🙂🙂🙂🙂");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn wrapped_file_respects_total_character_limit() {
        let root = tempfile::tempdir().unwrap();
        let skill_dir = fs::canonicalize(root.path()).unwrap();
        fs::write(skill_dir.join("guide.md"), "🙂🙂🙂🙂").unwrap();
        let skill_name = "test-skill/guide.md";
        let wrapper_characters = LOADED_FILE_PREFIX.chars().count()
            + skill_name.chars().count()
            + LOADED_FILE_SEPARATOR.chars().count()
            + LOADED_FILE_SUFFIX.chars().count();
        let max_characters = wrapper_characters + 4;

        let content = load_supporting_file_with_limit(
            &skill_dir,
            Path::new("guide.md"),
            skill_name,
            max_characters,
            false,
        )
        .unwrap();

        assert_eq!(content.chars().count(), max_characters);
        assert!(content.contains("🙂🙂🙂🙂"));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn rejects_file_that_exceeds_wrapped_character_limit() {
        let root = tempfile::tempdir().unwrap();
        let skill_dir = fs::canonicalize(root.path()).unwrap();
        fs::write(skill_dir.join("guide.md"), "ééééé").unwrap();
        let skill_name = "test-skill/guide.md";
        let wrapper_characters = LOADED_FILE_PREFIX.chars().count()
            + skill_name.chars().count()
            + LOADED_FILE_SEPARATOR.chars().count()
            + LOADED_FILE_SUFFIX.chars().count();

        let error = load_supporting_file_with_limit(
            &skill_dir,
            Path::new("guide.md"),
            skill_name,
            wrapper_characters + 4,
            false,
        )
        .expect_err("wrapped supporting-file limit was not enforced");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error
            .to_string()
            .contains("exceeds the maximum size of 4 characters"));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn rejects_file_one_character_over_size_limit() {
        let root = tempfile::tempdir().unwrap();
        let skill_dir = fs::canonicalize(root.path()).unwrap();
        fs::write(skill_dir.join("guide.md"), "ééééé").unwrap();

        let error = read_supporting_file_with_limit(
            &skill_dir,
            Path::new("guide.md"),
            4,
            RootLinkPolicy::Reject,
        )
        .expect_err("oversized supporting file was accepted");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error
            .to_string()
            .contains("exceeds the maximum size of 4 characters"));
    }

    #[test]
    fn streaming_limit_reads_only_limit_plus_one() {
        use std::cell::Cell;
        use std::rc::Rc;

        struct CountingReader(Rc<Cell<usize>>);

        impl io::Read for CountingReader {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                buffer.fill(b'a');
                self.0.set(self.0.get() + buffer.len());
                Ok(buffer.len())
            }
        }

        let bytes_read = Rc::new(Cell::new(0));
        let error = read_utf8_with_limit(CountingReader(Rc::clone(&bytes_read)), 4)
            .expect_err("streaming size limit was not enforced");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error
            .to_string()
            .contains("exceeds the maximum encoded size of 16 bytes"));
        assert_eq!(bytes_read.get(), 17);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_ancestor() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let skill_dir = fs::canonicalize(root.path()).unwrap();
        fs::write(outside.path().join("secret.txt"), "outside secret").unwrap();
        std::os::unix::fs::symlink(outside.path(), skill_dir.join("nested")).unwrap();

        let result = read_supporting_file_with_limit(
            &skill_dir,
            Path::new("nested/secret.txt"),
            crate::agents::max_tool_response_size(),
            RootLinkPolicy::Reject,
        );

        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn stays_in_opened_ancestor_after_symlink_swap() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let skill_dir = fs::canonicalize(root.path()).unwrap();
        let nested = skill_dir.join("nested");
        let moved_nested = skill_dir.join("moved-nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("payload"), "safe content").unwrap();
        fs::write(outside.path().join("payload"), "outside secret").unwrap();

        let content = read_supporting_file_with_hook(
            &skill_dir,
            Path::new("nested/payload"),
            crate::agents::max_tool_response_size(),
            RootLinkPolicy::Reject,
            |opened_path| {
                if opened_path == Path::new("nested") {
                    fs::rename(&nested, &moved_nested).unwrap();
                    std::os::unix::fs::symlink(outside.path(), &nested).unwrap();
                }
            },
        )
        .unwrap();

        assert_eq!(content, "safe content");
        assert!(!content.contains("outside secret"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_regular_skill_root_replaced_with_symlink_during_open() {
        let parent = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let parent = fs::canonicalize(parent.path()).unwrap();
        let skill_dir = parent.join("skill");
        let moved_skill_dir = parent.join("moved-skill");
        fs::create_dir(&skill_dir).unwrap();
        fs::write(skill_dir.join("payload"), "safe content").unwrap();
        fs::write(outside.path().join("payload"), "outside secret").unwrap();

        let result = read_supporting_file_with_hook(
            &skill_dir,
            Path::new("payload"),
            crate::agents::max_tool_response_size(),
            RootLinkPolicy::Reject,
            |opened_path| {
                if opened_path == parent {
                    fs::rename(&skill_dir, &moved_skill_dir).unwrap();
                    std::os::unix::fs::symlink(outside.path(), &skill_dir).unwrap();
                }
            },
        );

        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn follows_discovered_linked_skill_root() {
        let parent = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let parent = fs::canonicalize(parent.path()).unwrap();
        let skill_dir = parent.join("linked-skill");
        fs::write(target.path().join("payload"), "linked content").unwrap();
        std::os::unix::fs::symlink(target.path(), &skill_dir).unwrap();

        let content = read_supporting_file_with_hook(
            &skill_dir,
            Path::new("payload"),
            crate::agents::max_tool_response_size(),
            RootLinkPolicy::FollowFinal,
            |_| {},
        )
        .unwrap();

        assert_eq!(content, "linked content");
    }

    #[cfg(windows)]
    #[test]
    fn windows_stays_in_opened_ancestor_after_directory_swap() {
        let root = tempfile::tempdir().unwrap();
        let skill_dir = fs::canonicalize(root.path()).unwrap();
        let nested = skill_dir.join("nested");
        let moved_nested = skill_dir.join("moved-nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("payload"), "safe content").unwrap();

        let content = read_supporting_file_with_hook(
            &skill_dir,
            Path::new("nested/payload"),
            crate::agents::max_tool_response_size(),
            RootLinkPolicy::Reject,
            |opened_path| {
                if opened_path == Path::new("nested") {
                    fs::rename(&nested, &moved_nested).unwrap();
                    fs::create_dir(&nested).unwrap();
                    fs::write(nested.join("payload"), "outside secret").unwrap();
                }
            },
        )
        .unwrap();

        assert_eq!(content, "safe content");
        assert!(!content.contains("outside secret"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_rejects_skill_root_replaced_with_symlink_during_open() {
        let parent = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let parent = fs::canonicalize(parent.path()).unwrap();
        let skill_dir = parent.join("skill");
        let moved_skill_dir = parent.join("moved-skill");
        let replacement = parent.join("replacement");
        fs::create_dir(&skill_dir).unwrap();
        fs::write(skill_dir.join("payload"), "safe content").unwrap();
        fs::write(outside.path().join("payload"), "outside secret").unwrap();
        if std::os::windows::fs::symlink_dir(outside.path(), &replacement).is_err() {
            return;
        }

        let result = read_supporting_file_with_hook(
            &skill_dir,
            Path::new("payload"),
            crate::agents::max_tool_response_size(),
            RootLinkPolicy::Reject,
            |opened_path| {
                if opened_path == parent {
                    fs::rename(&skill_dir, &moved_skill_dir).unwrap();
                    fs::rename(&replacement, &skill_dir).unwrap();
                }
            },
        );

        assert!(result.is_err());
    }
}
