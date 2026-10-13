use crate::scanner::{self, Fingerprint};
use std::fs::{File, Metadata};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::{Arc, Mutex};
use windows::core::{implement, Error as ComError, Ref, HRESULT, PCWSTR};
use windows::Win32::Foundation::E_ABORT;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{
    FileOperation, IFileOperation, IFileOperationProgressSink, IFileOperationProgressSink_Impl,
    IShellItem, SHCreateItemFromParsingName, FOFX_ADDUNDORECORD, FOFX_EARLYFAILURE,
    FOFX_NOCOPYHOOKS, FOFX_RECYCLEONDELETE, FOF_NOERRORUI, FOF_NORECURSEREPARSE,
    FOF_NO_CONNECTED_ELEMENTS, FOF_SILENT, FOF_WANTNUKEWARNING, TSF_DELETE_RECYCLE_IF_POSSIBLE,
};
use windows_sys::Wdk::Storage::FileSystem::{
    FileRenameInformation, NtSetInformationFile, FILE_RENAME_INFORMATION,
};
use windows_sys::Win32::Foundation::{
    GetLastError, RtlNtStatusToDosError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

fn wide(path: &Path) -> Result<Vec<u16>, String> {
    let name: Vec<_> = path.as_os_str().encode_wide().collect();
    if name.contains(&0) || name.len() > 32760 {
        return Err("Invalid or excessively long native path".into());
    }
    Ok(name.into_iter().chain(Some(0)).collect())
}

fn error() -> String {
    std::io::Error::from_raw_os_error(unsafe { GetLastError() } as i32).to_string()
}

fn open_locked(path: &Path, delete: bool) -> Result<File, String> {
    let name = wide(path)?;
    let attributes = unsafe { GetFileAttributesW(name.as_ptr()) };
    if attributes == INVALID_FILE_ATTRIBUTES {
        return Err(error());
    }
    let access = FILE_READ_ATTRIBUTES
        | if delete { DELETE } else { 0 }
        | if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            FILE_TRAVERSE
        } else {
            0
        };
    let share = FILE_SHARE_READ
        | if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            FILE_SHARE_WRITE
        } else {
            0
        };
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            access,
            share,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(error());
    }
    let file = unsafe { File::from_raw_handle(handle) };
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if scanner::is_reparse(&metadata) || (!metadata.is_dir() && !metadata.is_file()) {
        return Err("Mount, reparse point, or special file encountered; refused.".into());
    }
    Ok(file)
}

fn observe(path: &Path) -> Result<File, String> {
    let name = wide(path)?;
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(error());
    }
    let file = unsafe { File::from_raw_handle(handle) };
    if scanner::is_reparse(&file.metadata().map_err(|e| e.to_string())?) {
        return Err("Reparse point encountered; refused.".into());
    }
    Ok(file)
}

fn handle_fingerprint(file: &File) -> Result<Fingerprint, String> {
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) } == 0 {
        return Err(error());
    }
    let mut fingerprint = scanner::fingerprint(&metadata);
    fingerprint.device = info.dwVolumeSerialNumber as u64;
    fingerprint.inode = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    let mut basic: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as HANDLE,
            FileBasicInfo,
            (&mut basic as *mut FILE_BASIC_INFO).cast(),
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        )
    } == 0
    {
        return Err(error());
    }
    fingerprint.changed_ns = basic
        .ChangeTime
        .saturating_sub(116_444_736_000_000_000)
        .saturating_mul(100);
    Ok(fingerprint)
}

fn same_identity(actual: &Fingerprint, expected: &Fingerprint) -> bool {
    actual.device == expected.device
        && actual.inode == expected.inode
        && actual.kind == expected.kind
}

pub fn locked_path(path: &Path) -> Result<Vec<File>, String> {
    if !path.is_absolute() {
        return Err("An absolute local path is required".into());
    }
    let mut part = PathBuf::new();
    let mut handles = Vec::new();
    let mut volume = None;
    for component in path.components() {
        match component {
            Component::Prefix(prefix)
                if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) =>
            {
                part.push(component.as_os_str());
                continue;
            }
            Component::Prefix(_) | Component::ParentDir | Component::CurDir => {
                return Err(
                    "Only an absolute local drive path without traversal is supported".into(),
                )
            }
            Component::Normal(name) if name.encode_wide().any(|c| c == b':' as u16 || c == 0) => {
                return Err("File streams and invalid names are not supported".into())
            }
            _ => part.push(component.as_os_str()),
        }
        if matches!(component, Component::RootDir)
            && unsafe { GetDriveTypeW(wide(&part)?.as_ptr()) } == 4
        {
            return Err("Network drives are outside local cleanup scope.".into());
        }
        let file = open_locked(&part, false)?;
        let device = handle_fingerprint(&file)?.device;
        if volume.is_some_and(|volume| volume != device) {
            return Err("Cross-volume mount encountered; refused.".into());
        }
        volume = Some(device);
        handles.push(file);
    }
    Ok(handles)
}

fn rename_handle(file: &File, parent: &File, to: &Path) -> Result<(), String> {
    if to.components().count() != 1 || !matches!(to.components().next(), Some(Component::Normal(_)))
    {
        return Err("Expected one rename component".into());
    }
    let name = wide(to)?;
    let name = &name[..name.len() - 1];
    let size = std::mem::size_of::<FILE_RENAME_INFORMATION>() + name.len() * 2;
    let mut buffer = vec![0u64; size.div_ceil(8)];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = parent.as_raw_handle() as HANDLE;
        (*info).FileNameLength = (name.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(name.as_ptr(), (*info).FileName.as_mut_ptr(), name.len());
        let mut io: IO_STATUS_BLOCK = std::mem::zeroed();
        let status = NtSetInformationFile(
            file.as_raw_handle() as HANDLE,
            &mut io,
            info.cast(),
            size as u32,
            FileRenameInformation,
        );
        if status < 0 {
            return Err(
                std::io::Error::from_raw_os_error(RtlNtStatusToDosError(status) as i32).to_string(),
            );
        }
    }
    Ok(())
}

fn mark_deleted(file: &File) -> Result<(), String> {
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if scanner::is_reparse(&metadata) {
        return Err("Reparse point appeared before deletion; refused.".into());
    }
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as HANDLE,
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(error());
    }
    Ok(())
}

pub struct Staged {
    _ancestors: Vec<File>,
    original: PathBuf,
    stage: PathBuf,
    stage_handle: Option<File>,
    stage_identity: Fingerprint,
    item_identity: Fingerprint,
    pub path: PathBuf,
    pub deleted_bytes: u64,
    handle: Option<File>,
    finished: bool,
}

impl Staged {
    #[cfg(test)]
    pub fn capture(path: &Path, expected: &Fingerprint) -> Result<Self, String> {
        let parent = path.parent().ok_or("Cannot clean a filesystem root")?;
        let root_expected = handle_fingerprint(&observe(parent)?)?;
        Self::capture_scoped(path, expected, parent, &root_expected)
    }

    pub fn capture_scoped(
        path: &Path,
        expected: &Fingerprint,
        root: &Path,
        root_expected: &Fingerprint,
    ) -> Result<Self, String> {
        let parent = path.parent().ok_or("Cannot clean a filesystem root")?;
        if !root.is_absolute()
            || root == path
            || !parent.starts_with(root)
            || root_expected.kind != "directory"
        {
            return Err("Cleanup target is outside the indexed directory root; refused.".into());
        }
        let ancestors = locked_path(parent)?;
        let root_position = root
            .components()
            .filter(|component| !matches!(component, Component::Prefix(_)))
            .count()
            .checked_sub(1)
            .ok_or("Invalid selected root")?;
        let root_handle = ancestors
            .get(root_position)
            .ok_or("Selected root handle missing")?;
        if !same_identity(&handle_fingerprint(root_handle)?, root_expected) {
            return Err("Selected root changed since scan; refused.".into());
        }
        let handle = open_locked(path, true)?;
        let actual = handle_fingerprint(&handle)?;
        if actual != *expected {
            return Err("Changed since scan. Scan again.".into());
        }
        if ancestors
            .last()
            .map(handle_fingerprint)
            .transpose()?
            .is_none_or(|parent| parent.device != actual.device)
        {
            return Err("Cross-volume cleanup refused.".into());
        }
        let stage = parent.join(format!(".trapclean-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&stage).map_err(|e| e.to_string())?;
        let stage_handle = match open_locked(&stage, false) {
            Ok(handle) => handle,
            Err(error) => {
                let _ = std::fs::remove_dir(&stage);
                return Err(error);
            }
        };
        let stage_identity = handle_fingerprint(&stage_handle)?;
        let destination = stage.join(path.file_name().ok_or("Missing filename")?);
        if let Err(error) = rename_handle(
            &handle,
            &stage_handle,
            Path::new(path.file_name().ok_or("Missing filename")?),
        ) {
            drop(stage_handle);
            remove_directory(&stage, &stage_identity);
            return Err(error);
        }
        Ok(Self {
            _ancestors: ancestors,
            original: path.into(),
            stage,
            stage_handle: Some(stage_handle),
            stage_identity,
            item_identity: actual,
            path: destination,
            deleted_bytes: 0,
            handle: Some(handle),
            finished: false,
        })
    }

    pub fn validate(
        &self,
        mut verify: impl FnMut(&Path, &Path, &Metadata) -> Result<u64, String>,
    ) -> Result<(), String> {
        fn visit(
            file: &File,
            current: &Path,
            relative: &Path,
            volume: u64,
            depth: usize,
            verify: &mut impl FnMut(&Path, &Path, &Metadata) -> Result<u64, String>,
        ) -> Result<(), String> {
            if depth > 256 {
                return Err("Cleanup depth limit reached".into());
            }
            let metadata = file.metadata().map_err(|e| e.to_string())?;
            if handle_fingerprint(file)?.device != volume {
                return Err("Cross-volume mount encountered; refused.".into());
            }
            let expected_count = verify(relative, current, &metadata)?;
            if metadata.is_dir() {
                let mut count = 0;
                for child in std::fs::read_dir(current).map_err(|e| e.to_string())? {
                    let child = child.map_err(|e| e.to_string())?;
                    let handle = open_locked(&child.path(), false)?;
                    visit(
                        &handle,
                        &child.path(),
                        &relative.join(child.file_name()),
                        volume,
                        depth + 1,
                        verify,
                    )?;
                    count += 1;
                }
                if count != expected_count {
                    return Err("Directory contents changed since scan; refused.".into());
                }
            }
            Ok(())
        }
        visit(
            self.handle.as_ref().ok_or("Missing staged handle")?,
            &self.path,
            Path::new(""),
            self.item_identity.device,
            0,
            &mut verify,
        )
    }

    pub fn restore(&mut self) -> Result<(), String> {
        if self.finished {
            return Ok(());
        }
        if self.stage_handle.is_none() {
            let handle = open_locked(&self.stage, false)?;
            if !same_identity(&handle_fingerprint(&handle)?, &self.stage_identity) {
                return Err("Staging folder changed; recovery needs manual review.".into());
            }
            self.stage_handle = Some(handle);
        }
        if self.handle.is_none() {
            let handle = open_locked(&self.path, true)?;
            if !same_identity(&handle_fingerprint(&handle)?, &self.item_identity) {
                return Err("Staged item changed; recovery needs manual review.".into());
            }
            self.handle = Some(handle);
        }
        rename_handle(
            self.handle.as_ref().ok_or("Missing handle")?,
            self._ancestors
                .last()
                .ok_or("Missing original parent handle")?,
            Path::new(self.original.file_name().ok_or("Missing original name")?),
        )
        .map_err(|error| {
            format!(
                "Recovery needed: item retained at {} ({error})",
                self.path.display()
            )
        })?;
        self.finished = true;
        self.handle = None;
        self.remove_stage();
        Ok(())
    }

    pub fn trash(&mut self) -> Result<(), String> {
        self.check_namespace()?;
        let apartment = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        apartment
            .ok()
            .map_err(|_| "System Recycle Bin is unavailable on this worker thread.".to_string())?;
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe {
                    CoUninitialize();
                }
            }
        }
        let _apartment = Apartment;
        let stage_path = shell_path(&self.stage)?;
        if !same_identity(
            &handle_fingerprint(&observe(&stage_path)?)?,
            &self.stage_identity,
        ) {
            return Err("The Shell staging path resolves to a different item; refused.".into());
        }
        let stage_path = wide(&stage_path)?;
        let operation: IFileOperation =
            unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER) }
                .map_err(|_| "System Recycle Bin is unavailable.".to_string())?;
        let flags = FOF_SILENT
            | FOF_NOERRORUI
            | FOF_WANTNUKEWARNING
            | FOF_NORECURSEREPARSE
            | FOF_NO_CONNECTED_ELEMENTS
            | FOFX_RECYCLEONDELETE
            | FOFX_EARLYFAILURE
            | FOFX_ADDUNDORECORD
            | FOFX_NOCOPYHOOKS;
        unsafe { operation.SetOperationFlags(flags) }.map_err(|_| {
            "System Recycle Bin does not support the required safe operation flags.".to_string()
        })?;
        let item: IShellItem =
            unsafe { SHCreateItemFromParsingName(PCWSTR(stage_path.as_ptr()), None) }.map_err(
                |_| "Could not identify the staging item for the system Recycle Bin.".to_string(),
            )?;
        let status = Arc::new(Mutex::new(RecycleStatus::default()));
        let sink: IFileOperationProgressSink = RecycleGuard {
            stage: self.stage.clone(),
            stage_identity: self.stage_identity.clone(),
            item: self.path.clone(),
            item_identity: self.item_identity.clone(),
            status: status.clone(),
        }
        .into();
        unsafe { operation.DeleteItem(&item, &sink) }
            .map_err(|_| "System Recycle Bin rejected the staging item.".to_string())?;
        self.check_namespace()?;
        self.handle = None;
        self.stage_handle = None;
        let performed = unsafe { operation.PerformOperations() };
        let aborted = unsafe { operation.GetAnyOperationsAborted() }
            .map(|aborted| aborted.as_bool())
            .unwrap_or(true);
        let status = status
            .lock()
            .map_err(|_| "Could not verify the Recycle Bin result.".to_string())?;
        if performed.is_err() || aborted || !status.recycled || status.failed {
            return Err("System Recycle Bin failed or could not verify recycling. Permanent deletion was not requested; inspect the original or staging folder before retrying.".into());
        }
        self.finished = true;
        Ok(())
    }

    pub fn remove(
        &mut self,
        mut verify: impl FnMut(&Path, &Metadata) -> Result<(), String>,
    ) -> Result<(), String> {
        fn remove(
            file: &File,
            current: &Path,
            relative: &Path,
            volume: u64,
            depth: usize,
            verify: &mut impl FnMut(&Path, &Metadata) -> Result<(), String>,
            deleted_bytes: &mut u64,
        ) -> Result<u64, String> {
            if depth > 256 {
                return Err("Cleanup depth limit reached".into());
            }
            let metadata = file.metadata().map_err(|e| e.to_string())?;
            if handle_fingerprint(file)?.device != volume {
                return Err("Cross-volume mount encountered; refused.".into());
            }
            verify(relative, &metadata)?;
            if metadata.is_dir() {
                for child in std::fs::read_dir(current).map_err(|e| e.to_string())? {
                    let child = child.map_err(|e| e.to_string())?;
                    let handle = open_locked(&child.path(), true)?;
                    let bytes = remove(
                        &handle,
                        &child.path(),
                        &relative.join(child.file_name()),
                        volume,
                        depth + 1,
                        verify,
                        deleted_bytes,
                    )?;
                    drop(handle);
                    *deleted_bytes = deleted_bytes.saturating_add(bytes);
                }
            }
            mark_deleted(file)?;
            Ok(if metadata.is_file() {
                metadata.len()
            } else {
                0
            })
        }
        self.check_namespace()?;
        let bytes = remove(
            self.handle.as_ref().ok_or("Missing staged handle")?,
            &self.path,
            Path::new(""),
            self.item_identity.device,
            0,
            &mut verify,
            &mut self.deleted_bytes,
        )?;
        self.handle = None;
        self.deleted_bytes = self.deleted_bytes.saturating_add(bytes);
        self.finished = true;
        self.remove_stage();
        Ok(())
    }

    fn check_namespace(&self) -> Result<(), String> {
        if !same_identity(
            &handle_fingerprint(self.stage_handle.as_ref().ok_or("Missing staging handle")?)?,
            &self.stage_identity,
        ) || !same_identity(
            &handle_fingerprint(self.handle.as_ref().ok_or("Missing staged handle")?)?,
            &self.item_identity,
        ) {
            return Err("Staged identities changed; refused.".into());
        }
        check_staging_contents(&self.stage, &self.path)?;
        Ok(())
    }

    fn remove_stage(&mut self) {
        self.stage_handle = None;
        remove_directory(&self.stage, &self.stage_identity);
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.restore();
        }
    }
}

fn remove_directory(path: &Path, expected: &Fingerprint) {
    if let Ok(handle) = open_locked(path, true) {
        if handle_fingerprint(&handle).is_ok_and(|actual| same_identity(&actual, expected)) {
            let _ = mark_deleted(&handle);
        }
    }
}

fn check_staging_contents(stage: &Path, item: &Path) -> Result<(), String> {
    let mut children = std::fs::read_dir(stage).map_err(|e| e.to_string())?;
    let child = children
        .next()
        .transpose()
        .map_err(|e| e.to_string())?
        .ok_or("Staged item disappeared")?;
    if child.file_name() != item.file_name().ok_or("Missing staged name")?
        || children.next().is_some()
    {
        return Err("Unexpected data appeared in the staging folder; refused.".into());
    }
    Ok(())
}

fn shell_path(path: &Path) -> Result<PathBuf, String> {
    let mut components = path.components();
    let mut result = match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
                PathBuf::from(format!("{}:\\", drive as char))
            }
            _ => return Err("Recycle Bin requires a local drive path.".into()),
        },
        _ => return Err("Recycle Bin requires an absolute local path.".into()),
    };
    for component in components {
        match component {
            Component::RootDir => (),
            Component::Normal(name) => result.push(name),
            _ => return Err("Invalid staging path.".into()),
        }
    }
    Ok(result)
}

#[derive(Default)]
struct RecycleStatus {
    recycled: bool,
    failed: bool,
}

#[implement(IFileOperationProgressSink)]
struct RecycleGuard {
    stage: PathBuf,
    stage_identity: Fingerprint,
    item: PathBuf,
    item_identity: Fingerprint,
    status: Arc<Mutex<RecycleStatus>>,
}

#[allow(non_snake_case)]
impl IFileOperationProgressSink_Impl for RecycleGuard_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, result: HRESULT) -> windows::core::Result<()> {
        if result.is_err() {
            self.status
                .lock()
                .map_err(|_| ComError::from_hresult(E_ABORT))?
                .failed = true;
        }
        Ok(())
    }
    fn PreDeleteItem(&self, flags: u32, _: Ref<IShellItem>) -> windows::core::Result<()> {
        if flags & TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32 == 0 {
            return Err(ComError::from_hresult(E_ABORT));
        }
        let validate = || -> Result<(), String> {
            let stage = observe(&self.stage)?;
            let item = observe(&self.item)?;
            if !same_identity(&handle_fingerprint(&stage)?, &self.stage_identity)
                || !same_identity(&handle_fingerprint(&item)?, &self.item_identity)
            {
                return Err("Staging identities changed before recycling.".into());
            }
            check_staging_contents(&self.stage, &self.item)?;
            Ok(())
        };
        validate().map_err(|_| ComError::from_hresult(E_ABORT))
    }
    fn PostDeleteItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        result: HRESULT,
        recycled: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        let mut status = self
            .status
            .lock()
            .map_err(|_| ComError::from_hresult(E_ABORT))?;
        if result.is_err() || recycled.is_none() {
            status.failed = true;
            return Err(ComError::from_hresult(E_ABORT));
        }
        status.recycled = true;
        Ok(())
    }
    fn PreRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        Err(ComError::from_hresult(E_ABORT))
    }
    fn PostRenameItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreMoveItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        Err(ComError::from_hresult(E_ABORT))
    }
    fn PostMoveItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreCopyItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        Err(ComError::from_hresult(E_ABORT))
    }
    fn PostCopyItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        Err(ComError::from_hresult(E_ABORT))
    }
    fn PostNewItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: &PCWSTR,
        _: u32,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, _: u32, _: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, PathBuf, Fingerprint) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("selected");
        std::fs::create_dir_all(root.join("work")).unwrap();
        std::fs::write(root.join("work/output.bin"), b"generated fixture data").unwrap();
        let expected = handle_fingerprint(&observe(&root).unwrap()).unwrap();
        (temp, root, expected)
    }

    #[test]
    fn handle_rename_into_guarded_directory_does_not_share_delete() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.bin");
        let directory = temp.path().join("destination");
        std::fs::write(&source, b"scoped fixture").unwrap();
        std::fs::create_dir(&directory).unwrap();
        let _ancestors = locked_path(temp.path()).unwrap();
        let destination = open_locked(&directory, false).unwrap();
        let file = open_locked(&source, true).unwrap();
        let expected = handle_fingerprint(&file).unwrap();
        rename_handle(&file, &destination, Path::new("captured.bin")).unwrap();
        assert!(!source.exists());
        assert!(same_identity(
            &handle_fingerprint(&file).unwrap(),
            &expected
        ));
        assert_eq!(
            std::fs::read(directory.join("captured.bin")).unwrap(),
            b"scoped fixture"
        );
        assert!(std::fs::rename(&directory, temp.path().join("swapped")).is_err());
        rename_handle(&file, _ancestors.last().unwrap(), Path::new("source.bin")).unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), b"scoped fixture");
    }

    #[test]
    fn capture_validate_and_restore_keep_owned_handles_without_share_conflicts() {
        let (_temp, root, expected) = fixture();
        let mut staged = Staged::capture(&root, &expected).unwrap();
        assert!(!root.exists());
        assert!(staged.path.join("work/output.bin").exists());
        assert!(locked_path(&staged.path).is_err());
        let mut inspected = 0;
        staged
            .validate(|relative, current, metadata| {
                assert!(!scanner::is_reparse(metadata));
                if relative.as_os_str().is_empty() {
                    assert!(same_identity(
                        &handle_fingerprint(&observe(current)?)?,
                        &expected
                    ));
                }
                inspected += 1;
                Ok(if metadata.is_dir() { 1 } else { 0 })
            })
            .unwrap();
        assert_eq!(inspected, 3);
        staged.restore().unwrap();
        assert_eq!(
            std::fs::read(root.join("work/output.bin")).unwrap(),
            b"generated fixture data"
        );
        assert!(same_identity(
            &handle_fingerprint(&observe(&root).unwrap()).unwrap(),
            &expected
        ));
        assert!(!staged.stage.exists());
    }

    #[test]
    fn scoped_capture_rejects_replaced_root_before_staging() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("authorized");
        let selected = root.join("selected");
        std::fs::create_dir_all(&selected).unwrap();
        std::fs::write(selected.join("original.txt"), b"original").unwrap();
        let root_expected = handle_fingerprint(&observe(&root).unwrap()).unwrap();
        let expected = handle_fingerprint(&observe(&selected).unwrap()).unwrap();
        let previous = temp.path().join("previous-root");
        std::fs::rename(&root, &previous).unwrap();
        std::fs::create_dir_all(&selected).unwrap();
        std::fs::write(selected.join("replacement.txt"), b"replacement").unwrap();
        let error = match Staged::capture_scoped(&selected, &expected, &root, &root_expected) {
            Ok(_) => panic!("A replaced selected root must be refused"),
            Err(error) => error,
        };
        assert!(error.contains("Selected root changed"));
        assert_eq!(
            std::fs::read(selected.join("replacement.txt")).unwrap(),
            b"replacement"
        );
        assert_eq!(
            std::fs::read(previous.join("selected/original.txt")).unwrap(),
            b"original"
        );
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    }

    #[test]
    fn scoped_capture_rejects_outside_target_and_retains_root_lock() {
        let (temp, selected, expected) = fixture();
        let root = temp.path();
        let root_expected = handle_fingerprint(&observe(root).unwrap()).unwrap();
        let unrelated = root.join("unrelated");
        std::fs::create_dir(&unrelated).unwrap();
        let unrelated_expected = handle_fingerprint(&observe(&unrelated).unwrap()).unwrap();
        assert!(
            Staged::capture_scoped(&selected, &expected, &unrelated, &unrelated_expected).is_err()
        );
        let mut staged =
            Staged::capture_scoped(&selected, &expected, root, &root_expected).unwrap();
        assert!(std::fs::rename(root, root.with_extension("replaced")).is_err());
        staged.restore().unwrap();
        assert_eq!(
            std::fs::read(selected.join("work/output.bin")).unwrap(),
            b"generated fixture data"
        );
    }

    #[test]
    fn held_target_and_stage_refuse_namespace_replacement() {
        let (temp, root, expected) = fixture();
        let mut staged = Staged::capture(&root, &expected).unwrap();
        assert!(std::fs::rename(&staged.path, temp.path().join("replacement-target")).is_err());
        assert!(std::fs::rename(&staged.stage, temp.path().join("replacement-stage")).is_err());
        staged.restore().unwrap();
    }

    #[test]
    fn permanent_deletion_uses_verified_handles_and_preserves_outside_fixture() {
        let (temp, root, expected) = fixture();
        let outside = temp.path().join("outside.txt");
        std::fs::write(&outside, b"keep").unwrap();
        let mut staged = Staged::capture(&root, &expected).unwrap();
        let mut count = 0;
        staged
            .remove(|_, metadata| {
                assert!(!scanner::is_reparse(metadata));
                count += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(count, 3);
        assert_eq!(staged.deleted_bytes, b"generated fixture data".len() as u64);
        assert!(!root.exists() && !staged.path.exists() && !staged.stage.exists());
        assert_eq!(std::fs::read(outside).unwrap(), b"keep");
    }

    #[test]
    fn partial_permanent_failure_keeps_deleted_bytes_and_restores_remaining_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("selected");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("alpha.bin"), [4u8; 23]).unwrap();
        std::fs::write(root.join("beta.bin"), [5u8; 31]).unwrap();
        let expected = handle_fingerprint(&observe(&root).unwrap()).unwrap();
        let mut staged = Staged::capture(&root, &expected).unwrap();
        let mut first_deleted = None;
        let mut first_bytes = 0;
        let result = staged.remove(|relative, metadata| {
            if !metadata.is_file() {
                return Ok(());
            }
            if first_deleted.is_some() {
                return Err("Cancelled after the first file".into());
            }
            first_deleted = Some(relative.to_path_buf());
            first_bytes = metadata.len();
            Ok(())
        });
        assert!(result.is_err());
        assert!(staged.deleted_bytes > 0);
        assert_eq!(staged.deleted_bytes, first_bytes);
        let deleted = first_deleted.unwrap();
        assert!(!staged.path.join(&deleted).exists());
        let remaining = if deleted == Path::new("alpha.bin") {
            "beta.bin"
        } else {
            "alpha.bin"
        };
        assert!(staged.path.join(remaining).exists());
        staged.restore().unwrap();
        assert!(!root.join(deleted).exists());
        assert!(root.join(remaining).exists());
        assert_eq!(staged.deleted_bytes, first_bytes);
    }

    #[test]
    fn rejected_verification_restores_contents_and_collision_never_overwrites() {
        let (_temp, root, expected) = fixture();
        let mut staged = Staged::capture(&root, &expected).unwrap();
        assert!(staged
            .remove(|relative, _| if relative == Path::new("work/output.bin") {
                Err("Changed before deletion".into())
            } else {
                Ok(())
            })
            .is_err());
        assert!(staged.path.join("work/output.bin").exists());
        assert_eq!(staged.deleted_bytes, 0);
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("replacement.txt"), b"replacement").unwrap();
        assert!(staged.restore().is_err());
        assert_eq!(
            std::fs::read(root.join("replacement.txt")).unwrap(),
            b"replacement"
        );
        assert_eq!(
            std::fs::read(staged.path.join("work/output.bin")).unwrap(),
            b"generated fixture data"
        );
        std::fs::remove_dir_all(&root).unwrap();
        staged.restore().unwrap();
    }

    #[test]
    fn unexpected_staging_data_prevents_recycling() {
        let (_temp, root, expected) = fixture();
        let mut staged = Staged::capture(&root, &expected).unwrap();
        let extra = staged.stage.join("unexpected.txt");
        std::fs::write(&extra, b"unselected").unwrap();
        assert!(staged.trash().is_err());
        staged.restore().unwrap();
        assert_eq!(std::fs::read(&extra).unwrap(), b"unselected");
        assert!(root.join("work/output.bin").exists());
    }

    #[test]
    fn reparse_ancestors_and_targets_are_never_followed() {
        let (temp, root, expected) = fixture();
        let link = temp.path().join("link");
        match std::os::windows::fs::symlink_dir(&root, &link) {
            Ok(()) => (),
            Err(error) if error.raw_os_error() == Some(1314) => return,
            Err(error) => panic!("Could not construct reparse fixture: {error}"),
        }
        assert!(locked_path(&link).is_err());
        assert!(Staged::capture(&link, &expected).is_err());
        assert!(locked_path(&link.join("work")).is_err());
        assert_eq!(
            std::fs::read(root.join("work/output.bin")).unwrap(),
            b"generated fixture data"
        );
    }

    #[test]
    fn recycle_fixture_succeeds_or_restores_without_permanent_fallback() {
        let (_temp, root, expected) = fixture();
        let mut staged = Staged::capture(&root, &expected).unwrap();
        match staged.trash() {
            Ok(()) => assert!(!staged.stage.exists()),
            Err(_) => {
                staged.restore().unwrap();
                assert_eq!(
                    std::fs::read(root.join("work/output.bin")).unwrap(),
                    b"generated fixture data"
                );
            }
        }
    }

    #[test]
    fn traversal_stream_and_nonlocal_paths_are_rejected() {
        for path in [
            r"relative\path",
            r"C:\folder\..\outside",
            r"C:\folder\file:stream",
            r"\\server\share\folder",
            r"\\.\PhysicalDrive0",
        ] {
            assert!(locked_path(Path::new(path)).is_err());
        }
    }
}
