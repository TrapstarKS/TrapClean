#[cfg(unix)]
use std::path::{Path, PathBuf};

#[cfg(unix)]
mod platform {
    use super::*;
    use std::ffi::{CStr, CString, OsString};
    use std::fs::{File, Metadata};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::MetadataExt;

    fn cpath(path: &Path) -> Result<CString, String> {
        CString::new(path.as_os_str().as_bytes()).map_err(|_| "Invalid native path".into())
    }
    fn last_error() -> String {
        std::io::Error::last_os_error().to_string()
    }
    fn rename_exclusive(from_dir: i32, from: &CString, to_dir: i32, to: &CString) -> i32 {
        #[cfg(target_os = "macos")]
        unsafe {
            libc::renameatx_np(
                from_dir,
                from.as_ptr(),
                to_dir,
                to.as_ptr(),
                libc::RENAME_EXCL,
            )
        }
        #[cfg(not(target_os = "macos"))]
        unsafe {
            libc::renameat2(
                from_dir,
                from.as_ptr(),
                to_dir,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        }
    }
    fn file(fd: i32) -> Result<File, String> {
        if fd < 0 {
            Err(last_error())
        } else {
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }
    pub fn open_directory(path: &Path) -> Result<File, String> {
        if !path.is_absolute() {
            return Err("An absolute local folder is required".into());
        }
        let mut dir = file(unsafe {
            libc::open(
                c"/".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        })?;
        for component in path.components() {
            match component {
                std::path::Component::RootDir => {}
                std::path::Component::Normal(name) => {
                    let name = CString::new(name.as_bytes()).map_err(|_| "Invalid path")?;
                    dir = file(unsafe {
                        libc::openat(
                            dir.as_raw_fd(),
                            name.as_ptr(),
                            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                        )
                    })?;
                }
                _ => return Err("Parent traversal is not allowed".into()),
            }
        }
        Ok(dir)
    }

    pub fn open_child(dir: &File, name: &Path) -> Result<File, String> {
        if name.components().count() != 1 {
            return Err("Expected one path component".into());
        }
        let name = cpath(name)?;
        file(unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        })
    }

    pub fn for_each_name(
        dir: &File,
        mut visit: impl FnMut(OsString) -> Result<(), String>,
    ) -> Result<(), String> {
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                c".".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(last_error());
        }
        let stream = unsafe { libc::fdopendir(fd) };
        if stream.is_null() {
            unsafe {
                libc::close(fd);
            }
            return Err(last_error());
        }
        struct Stream(*mut libc::DIR);
        impl Drop for Stream {
            fn drop(&mut self) {
                unsafe {
                    libc::closedir(self.0);
                }
            }
        }
        let stream = Stream(stream);
        loop {
            #[cfg(target_os = "macos")]
            unsafe {
                *libc::__error() = 0;
            }
            #[cfg(target_os = "linux")]
            unsafe {
                *libc::__errno_location() = 0;
            }
            let entry = unsafe { libc::readdir(stream.0) };
            if entry.is_null() {
                let error = std::io::Error::last_os_error();
                return if error.raw_os_error() == Some(0) {
                    Ok(())
                } else {
                    Err(error.to_string())
                };
            }
            let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            visit(OsString::from_vec(bytes.to_vec()))?;
        }
    }

    pub struct Staged {
        parent: File,
        directory: File,
        original_name: PathBuf,
        stage_name: PathBuf,
        item_name: PathBuf,
        pub path: PathBuf,
        pub handle: File,
        pub trash_location: Option<PathBuf>,
        pub deleted_bytes: u64,
        restored_or_removed: bool,
    }

    impl Staged {
        #[cfg(test)]
        pub fn capture(
            path: &Path,
            expected: &crate::scanner::Fingerprint,
        ) -> Result<Self, String> {
            let parent_path = path.parent().ok_or("Cannot clean a filesystem root")?;
            let root_expected = crate::scanner::fingerprint(
                &std::fs::symlink_metadata(parent_path).map_err(|e| e.to_string())?,
            );
            Self::capture_scoped(path, expected, parent_path, &root_expected)
        }

        pub fn capture_scoped(
            path: &Path,
            expected: &crate::scanner::Fingerprint,
            root: &Path,
            root_expected: &crate::scanner::Fingerprint,
        ) -> Result<Self, String> {
            let parent_path = path.parent().ok_or("Cannot clean a filesystem root")?;
            let original_name = PathBuf::from(path.file_name().ok_or("Missing filename")?);
            let relative = parent_path
                .strip_prefix(root)
                .map_err(|_| "The item is outside the authorized root")?;
            let mut parent = open_directory(root)?;
            let root_metadata = parent.metadata().map_err(|e| e.to_string())?;
            if !root_metadata.is_dir()
                || root_metadata.dev() != root_expected.device
                || root_metadata.ino() != root_expected.inode
            {
                return Err("The authorized root was replaced; refused.".into());
            }
            for component in relative.components() {
                if !matches!(component, std::path::Component::Normal(_)) {
                    return Err("Parent traversal is not allowed".into());
                }
                parent = open_child(&parent, Path::new(component.as_os_str()))?;
                let metadata = parent.metadata().map_err(|e| e.to_string())?;
                if !metadata.is_dir() || metadata.dev() != root_expected.device {
                    return Err(
                        "A mount or non-directory interrupts the authorized root; refused.".into(),
                    );
                }
            }
            let before = open_child(&parent, &original_name)?;
            let metadata = before.metadata().map_err(|e| e.to_string())?;
            if crate::scanner::fingerprint(&metadata) != *expected {
                return Err("Changed since scan. Scan this folder again.".into());
            }
            let stage_name = PathBuf::from(format!(".trapclean-{}", uuid::Uuid::new_v4()));
            let stage_c = cpath(&stage_name)?;
            if unsafe { libc::mkdirat(parent.as_raw_fd(), stage_c.as_ptr(), 0o700) } != 0 {
                return Err(last_error());
            }
            let directory = open_child(&parent, &stage_name)?;
            let item_name = original_name.clone();
            let original_c = cpath(&original_name)?;
            let item_c = cpath(&item_name)?;
            if unsafe {
                libc::renameat(
                    parent.as_raw_fd(),
                    original_c.as_ptr(),
                    directory.as_raw_fd(),
                    item_c.as_ptr(),
                )
            } != 0
            {
                unsafe {
                    libc::unlinkat(parent.as_raw_fd(), stage_c.as_ptr(), libc::AT_REMOVEDIR);
                }
                return Err(last_error());
            }
            let path = parent_path.join(&stage_name).join(&item_name);
            let handle = match open_child(&directory, &item_name) {
                Ok(file) => file,
                Err(error) => {
                    let restored = rename_exclusive(
                        directory.as_raw_fd(),
                        &item_c,
                        parent.as_raw_fd(),
                        &original_c,
                    );
                    if restored != 0 {
                        return Err(format!(
                            "{error}. Recovery needed: captured item retained at {} ({})",
                            path.display(),
                            last_error()
                        ));
                    }
                    unsafe {
                        libc::unlinkat(parent.as_raw_fd(), stage_c.as_ptr(), libc::AT_REMOVEDIR);
                    }
                    return Err(error);
                }
            };
            let mut staged = Self {
                parent,
                directory,
                original_name,
                stage_name,
                item_name,
                path,
                handle,
                trash_location: None,
                deleted_bytes: 0,
                restored_or_removed: false,
            };
            let after = staged.handle.metadata().map_err(|e| e.to_string())?;
            if after.dev() != metadata.dev()
                || after.ino() != metadata.ino()
                || after.file_type() != metadata.file_type()
            {
                staged.restore()?;
                return Err("Item was replaced during cleanup; refused.".into());
            }
            Ok(staged)
        }

        pub fn restore(&mut self) -> Result<(), String> {
            let from = cpath(&self.item_name)?;
            let to = cpath(&self.original_name)?;
            let status = rename_exclusive(
                self.directory.as_raw_fd(),
                &from,
                self.parent.as_raw_fd(),
                &to,
            );
            if status != 0 {
                return Err(format!(
                    "Recovery needed: item retained at {} ({})",
                    self.path.display(),
                    last_error()
                ));
            }
            self.restored_or_removed = true;
            self.remove_stage_directory();
            Ok(())
        }

        pub fn trash(&mut self) -> Result<(), String> {
            if self.path.to_str().is_none() {
                return Err("The system trash API cannot preserve this non-UTF-8 path. Use the separate permanent action or move it in the system file manager.".into());
            }
            let current = open_directory(self.path.parent().ok_or("Missing staging folder")?)?;
            let expected = self.directory.metadata().map_err(|e| e.to_string())?;
            let actual = current.metadata().map_err(|e| e.to_string())?;
            if actual.dev() != expected.dev() || actual.ino() != expected.ino() {
                return Err("Staging folder moved; refused.".into());
            }
            let live = open_child(&current, &self.item_name)?
                .metadata()
                .map_err(|e| e.to_string())?;
            let held = self.handle.metadata().map_err(|e| e.to_string())?;
            if live.dev() != held.dev() || live.ino() != held.ino() {
                return Err("Staged item changed; refused.".into());
            }
            #[cfg(target_os = "macos")]
            {
                use objc2_foundation::{NSFileManager, NSString, NSURL};
                let path = NSString::from_str(
                    self.path
                        .to_str()
                        .ok_or("The system trash API requires a UTF-8 path")?,
                );
                let url = NSURL::fileURLWithPath(&path);
                let reference = url.fileReferenceURL().ok_or(
                    "The filesystem cannot create an identity-based Trash reference; refused.",
                )?;
                let resolved = reference
                    .filePathURL()
                    .ok_or("The Trash reference no longer resolves; refused.")?;
                let mut buffer = vec![0i8; 32768];
                if !unsafe {
                    resolved.getFileSystemRepresentation_maxLength(
                        std::ptr::NonNull::new(buffer.as_mut_ptr()).ok_or("Missing path buffer")?,
                        buffer.len(),
                    )
                } {
                    return Err("The Trash reference path cannot be represented; refused.".into());
                }
                let resolved_path = PathBuf::from(OsString::from_vec(
                    unsafe { CStr::from_ptr(buffer.as_ptr()) }
                        .to_bytes()
                        .to_vec(),
                ));
                let resolved_metadata = std::fs::symlink_metadata(&resolved_path)
                    .map_err(|_| "The Trash reference changed; refused.")?;
                if resolved_metadata.dev() != held.dev()
                    || resolved_metadata.ino() != held.ino()
                    || resolved_path.parent() != self.path.parent()
                {
                    return Err(
                        "The Trash reference points outside the captured item; refused.".into(),
                    );
                }
                // A file reference URL binds the OS operation to the captured identity.
                let mut resulting = None;
                NSFileManager::defaultManager()
                    .trashItemAtURL_resultingItemURL_error(&reference, Some(&mut resulting))
                    .map_err(|_| {
                        "System Trash failed. Nothing was permanently deleted.".to_string()
                    })?;
                if let Some(resulting) = resulting {
                    self.trash_location = Some(PathBuf::from(OsString::from_vec(
                        unsafe { CStr::from_ptr(resulting.fileSystemRepresentation().as_ptr()) }
                            .to_bytes()
                            .to_vec(),
                    )));
                }
            }
            #[cfg(not(target_os = "macos"))]
            trash::delete(&self.path)
                .map_err(|_| "System trash failed. Nothing was permanently deleted.".to_string())?;
            self.restored_or_removed = true;
            self.remove_stage_directory();
            Ok(())
        }

        pub fn remove(
            &mut self,
            mut verify: impl FnMut(&Path, &Metadata) -> Result<(), String>,
        ) -> Result<(), String> {
            let metadata = self.handle.metadata().map_err(|e| e.to_string())?;
            remove_at(
                &self.directory,
                &self.item_name,
                Path::new(""),
                metadata.dev(),
                0,
                &mut verify,
                &mut self.deleted_bytes,
            )?;
            self.restored_or_removed = true;
            self.remove_stage_directory();
            Ok(())
        }
        fn remove_stage_directory(&self) {
            if let Ok(name) = cpath(&self.stage_name) {
                unsafe {
                    libc::unlinkat(self.parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR);
                }
            }
        }
    }
    impl Drop for Staged {
        fn drop(&mut self) {
            if !self.restored_or_removed {
                let _ = self.restore();
            }
        }
    }

    fn remove_at(
        parent: &File,
        name: &Path,
        relative: &Path,
        device: u64,
        depth: usize,
        verify: &mut impl FnMut(&Path, &Metadata) -> Result<(), String>,
        deleted_bytes: &mut u64,
    ) -> Result<(), String> {
        if depth > 256 {
            return Err("Cleanup depth limit reached".into());
        }
        let held = open_child(parent, name)?;
        let metadata = held.metadata().map_err(|e| e.to_string())?;
        if metadata.dev() != device || (!metadata.is_file() && !metadata.is_dir()) {
            return Err("Mount, link, or special file encountered; refused.".into());
        }
        verify(relative, &metadata)?;
        if metadata.is_dir() {
            for_each_name(&held, |child| {
                remove_at(
                    &held,
                    Path::new(&child),
                    &relative.join(&child),
                    device,
                    depth + 1,
                    verify,
                    deleted_bytes,
                )
            })?;
        }
        let live = open_child(parent, name)?
            .metadata()
            .map_err(|e| e.to_string())?;
        if live.dev() != metadata.dev() || live.ino() != metadata.ino() {
            return Err("Item changed during deletion; refused.".into());
        }
        let name = cpath(name)?;
        let flags = if metadata.is_dir() {
            libc::AT_REMOVEDIR
        } else {
            0
        };
        if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), flags) } != 0 {
            Err(last_error())
        } else {
            if metadata.is_file() {
                *deleted_bytes = deleted_bytes.saturating_add(metadata.len());
            }
            Ok(())
        }
    }
}

#[cfg(unix)]
pub use platform::*;

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    #[test]
    fn scoped_capture_refuses_replaced_root_before_moving_any_item() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = std::fs::canonicalize(temporary.path()).unwrap();
        let root = parent.join("root");
        std::fs::create_dir(&root).unwrap();
        let path = root.join("fixture.bin");
        std::fs::write(&path, b"original fixture").unwrap();
        let expected = crate::scanner::fingerprint(&std::fs::symlink_metadata(&path).unwrap());
        let root_expected = crate::scanner::fingerprint(&std::fs::symlink_metadata(&root).unwrap());
        std::fs::rename(&root, parent.join("saved-root")).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(&path, b"replacement fixture").unwrap();
        assert!(Staged::capture_scoped(&path, &expected, &root, &root_expected).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement fixture");
        assert_eq!(
            std::fs::read(parent.join("saved-root/fixture.bin")).unwrap(),
            b"original fixture"
        );
    }

    #[test]
    fn partial_permanent_failure_counts_deleted_files_and_restores_remaining_contents() {
        let temporary = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temporary.path()).unwrap();
        let path = root.join("generated");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("one.bin"), b"first fixture").unwrap();
        std::fs::write(path.join("two.bin"), b"second fixture contents").unwrap();
        let fingerprint = crate::scanner::fingerprint(&std::fs::symlink_metadata(&path).unwrap());
        let mut staged = Staged::capture(&path, &fingerprint).unwrap();
        let mut visited = 0;
        let mut first_bytes = 0;
        let outcome = staged.remove(|_, metadata| {
            if metadata.is_file() {
                visited += 1;
                if visited == 2 {
                    return Err("fixture interruption".into());
                }
                first_bytes = metadata.len();
            }
            Ok(())
        });
        assert!(outcome.is_err());
        assert_eq!(staged.deleted_bytes, first_bytes);
        assert!(staged.deleted_bytes > 0);
        staged.restore().unwrap();
        let remaining = std::fs::read_dir(&path).unwrap().collect::<Vec<_>>();
        assert_eq!(remaining.len(), 1);
        assert_eq!(
            remaining[0].as_ref().unwrap().metadata().unwrap().len() + first_bytes,
            (b"first fixture".len() + b"second fixture contents".len()) as u64
        );
    }

    #[test]
    fn restoration_collision_preserves_both_objects() {
        let temporary = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temporary.path()).unwrap();
        let path = root.join("fixture.bin");
        std::fs::write(&path, b"original fixture").unwrap();
        let fingerprint = crate::scanner::fingerprint(&std::fs::symlink_metadata(&path).unwrap());
        let mut staged = Staged::capture(&path, &fingerprint).unwrap();
        std::fs::write(&path, b"replacement fixture").unwrap();
        assert!(staged.restore().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement fixture");
        assert_eq!(std::fs::read(&staged.path).unwrap(), b"original fixture");
        std::fs::remove_file(&path).unwrap();
        staged.restore().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"original fixture");
    }

    #[test]
    fn native_trash_roundtrip_uses_only_a_generated_fixture() {
        let temporary = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temporary.path()).unwrap();
        let path = root.join(format!("trapclean-test-{}.dat", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"only generated fixture data").unwrap();
        let fingerprint = crate::scanner::fingerprint(&std::fs::symlink_metadata(&path).unwrap());
        let mut staged = Staged::capture(&path, &fingerprint).unwrap();
        staged.trash().unwrap();
        assert!(!path.exists());
        let location = staged
            .trash_location
            .as_ref()
            .expect("OS returns the moved fixture location");
        assert_eq!(
            std::fs::read(location).unwrap(),
            b"only generated fixture data"
        );
        std::fs::rename(location, &path).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"only generated fixture data");
    }
}

#[cfg(windows)]
#[path = "native_windows.rs"]
mod platform_windows;
#[cfg(windows)]
pub use platform_windows::*;
