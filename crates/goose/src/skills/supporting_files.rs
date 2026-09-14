use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path};

const LOADED_FILE_PREFIX: &str = "# Loaded: ";
const LOADED_FILE_SEPARATOR: &str = "\n\n";
const LOADED_FILE_SUFFIX: &str = "\n\n---\nFile loaded into context.";
const MAX_SOURCE_FILE_BYTES: usize = crate::scheduler::MAX_SCHEDULE_RECIPE_BYTES as usize;

#[derive(Clone, Copy)]
enum ReadLimit {
    Characters(usize),
    Bytes(usize),
}

pub(crate) fn load_supporting_file(
    skill_dir: &Path,
    relative: &Path,
    skill_name: &str,
) -> io::Result<String> {
    load_supporting_file_with_limit(
        skill_dir,
        relative,
        skill_name,
        crate::agents::max_tool_response_size(),
    )
}

fn load_supporting_file_with_limit(
    skill_dir: &Path,
    relative: &Path,
    skill_name: &str,
    max_characters: usize,
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
    let content = read_supporting_file_with_limit(skill_dir, relative, content_limit)?;
    Ok(format!(
        "{LOADED_FILE_PREFIX}{skill_name}{LOADED_FILE_SEPARATOR}{content}{LOADED_FILE_SUFFIX}"
    ))
}

fn read_supporting_file_with_limit(
    skill_dir: &Path,
    relative: &Path,
    max_characters: usize,
) -> io::Result<String> {
    read_supporting_file_with_hook(skill_dir, relative, max_characters, |_| {})
}

pub(crate) fn read_source_file(source_dir: &Path, relative: &Path) -> io::Result<String> {
    read_confined_file_with_hook(
        source_dir,
        relative,
        ReadLimit::Bytes(MAX_SOURCE_FILE_BYTES),
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
    after_opened_component: impl FnMut(&Path),
) -> io::Result<String> {
    read_confined_file_with_hook(
        skill_dir,
        relative,
        ReadLimit::Characters(max_characters),
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
fn open_skill_root(
    skill_dir: &Path,
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
                directory = open_at(&directory, component, directory_traversal_flags())?;
                opened_path.push(component);
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
    mut after_opened_component: impl FnMut(&Path),
) -> io::Result<String> {
    let components = validated_relative_components(relative)?;
    let (file_name, ancestors) = components.split_last().unwrap();
    let mut directory = open_skill_root(skill_dir, &mut after_opened_component)?;

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
    let mut directory = open_skill_root(source_dir, &mut after_opened_component)?;

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
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let name = CString::new(name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "supporting file path contains a NUL byte",
        )
    })?;
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

#[cfg(windows)]
fn open_skill_root(
    skill_dir: &Path,
    after_opened_component: &mut impl FnMut(&Path),
) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use winapi::um::winbase::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT};
    use winapi::um::winnt::{
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE,
        SYNCHRONIZE,
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
        .access_mode(FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
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
        directory = windows_open_at(&directory, component, true)?;
        let metadata = directory.metadata()?;
        if windows_metadata_is_reparse_point(&metadata) || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "skill path ancestor is not a regular directory",
            ));
        }
        opened_path.push(component);
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
    mut after_opened_component: impl FnMut(&Path),
) -> io::Result<String> {
    let components = validated_relative_components(relative)?;
    let (file_name, ancestors) = components.split_last().unwrap();
    let mut directory = open_skill_root(skill_dir, &mut after_opened_component)?;

    let mut opened_path = std::path::PathBuf::new();
    for ancestor in ancestors {
        directory = windows_open_at(&directory, ancestor, true)?;
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

    let file = windows_open_at(&directory, file_name, false)?;
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
    let mut directory = open_skill_root(source_dir, &mut after_opened_component)?;

    let mut opened_path = std::path::PathBuf::new();
    for ancestor in ancestors {
        directory = windows_open_at(&directory, ancestor, true)?;
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
) -> io::Result<fs::File> {
    use ntapi::ntioapi::{
        FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
    };
    use winapi::um::winnt::{FILE_GENERIC_READ, FILE_READ_ATTRIBUTES, FILE_TRAVERSE, SYNCHRONIZE};

    let mut create_options = FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT;
    if directory_only {
        create_options |= FILE_DIRECTORY_FILE;
    }
    let desired_access = if directory_only {
        FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE
    } else {
        FILE_GENERIC_READ
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

        let content =
            read_supporting_file_with_limit(&skill_dir, Path::new("guide.md"), 4).unwrap();

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

        let error = read_supporting_file_with_limit(&skill_dir, Path::new("guide.md"), 4)
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
    fn rejects_skill_root_replaced_with_symlink_during_open() {
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
            |opened_path| {
                if opened_path == parent {
                    fs::rename(&skill_dir, &moved_skill_dir).unwrap();
                    std::os::unix::fs::symlink(outside.path(), &skill_dir).unwrap();
                }
            },
        );

        assert!(result.is_err());
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
