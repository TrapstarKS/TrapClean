use crate::index::{Index, IndexedEntry};
use crate::model::DuplicateGroup;
use crate::scanner;
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

const SAMPLE_BYTES: usize = 64 * 1024;
const MAX_GROUPS: usize = 200;
const MAX_MEMBERS: usize = 20;

fn hex(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(64);
    for value in bytes.as_ref() {
        let _ = write!(result, "{value:02x}");
    }
    result
}

fn open_record(record: &IndexedEntry) -> Result<File, String> {
    #[cfg(unix)]
    {
        let parent =
            crate::native::open_directory(record.path.parent().ok_or("Missing parent directory")?)?;
        crate::native::open_child(
            &parent,
            Path::new(record.path.file_name().ok_or("Missing filename")?),
        )
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&record.path)
            .map_err(|e| e.to_string())
    }
}

fn validate_file(record: &IndexedEntry, file: &File) -> Result<(), String> {
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if scanner::path_fingerprint(&record.path, &metadata)? != record.fingerprint
        || !metadata.is_file()
        || scanner::is_reparse(&metadata)
    {
        return Err(
            "A file changed after its scan. Scan the directory again before checking duplicates."
                .into(),
        );
    }
    Ok(())
}

fn simple_content(file: &File) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        let size = unsafe {
            libc::fgetxattr(
                file.as_raw_fd(),
                c"com.apple.ResourceFork".as_ptr(),
                std::ptr::null_mut(),
                0,
                0,
                0,
            )
        };
        if size >= 0 {
            return size == 0;
        }
        matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ENOATTR) | Some(libc::ENOTSUP)
        )
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FileStreamInfo, GetFileInformationByHandleEx, FILE_STREAM_INFO,
        };
        let mut buffer = [0u64; 8192];
        let length = std::mem::size_of_val(&buffer);
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileStreamInfo,
                buffer.as_mut_ptr().cast(),
                length as u32,
            )
        } == 0
        {
            return false;
        }
        let header = std::mem::offset_of!(FILE_STREAM_INFO, StreamName);
        let mut offset = 0usize;
        for _ in 0..64 {
            if !offset.is_multiple_of(std::mem::align_of::<FILE_STREAM_INFO>())
                || offset
                    .checked_add(std::mem::size_of::<FILE_STREAM_INFO>())
                    .is_none_or(|end| end > length)
            {
                return false;
            }
            let info = unsafe {
                &*(buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<FILE_STREAM_INFO>())
            };
            let bytes = info.StreamNameLength as usize;
            if !bytes.is_multiple_of(2)
                || offset
                    .checked_add(header)
                    .and_then(|value| value.checked_add(bytes))
                    .is_none_or(|end| end > length)
            {
                return false;
            }
            let name = unsafe {
                std::slice::from_raw_parts(
                    buffer
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset + header)
                        .cast::<u16>(),
                    bytes / 2,
                )
            };
            if !"::$DATA".encode_utf16().eq(name.iter().copied()) {
                return false;
            }
            if info.NextEntryOffset == 0 {
                return true;
            }
            let next = info.NextEntryOffset as usize;
            if next < header + bytes || offset.checked_add(next).is_none_or(|end| end >= length) {
                return false;
            }
            offset += next;
        }
        false
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = file;
        true
    }
}

fn hash_file(
    record: &IndexedEntry,
    partial: bool,
    cancel: &AtomicBool,
) -> Result<Option<String>, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Duplicate analysis cancelled.".into());
    }
    let mut file = open_record(record)?;
    validate_file(record, &file)?;
    if !simple_content(&file) {
        return Ok(None);
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; SAMPLE_BYTES];
    if partial {
        hash.update(record.fingerprint.len.to_le_bytes());
        let first = record.fingerprint.len.min(SAMPLE_BYTES as u64) as usize;
        file.read_exact(&mut buffer[..first])
            .map_err(|e| e.to_string())?;
        hash.update(&buffer[..first]);
        if record.fingerprint.len > SAMPLE_BYTES as u64 {
            file.seek(SeekFrom::Start(
                record.fingerprint.len.saturating_sub(SAMPLE_BYTES as u64),
            ))
            .map_err(|e| e.to_string())?;
            file.read_exact(&mut buffer).map_err(|e| e.to_string())?;
            hash.update(buffer);
        }
    } else {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Duplicate analysis cancelled.".into());
            }
            let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
    }
    validate_file(record, &file)?;
    Ok(Some(hex(hash.finalize())))
}

pub fn hash_current(record: &IndexedEntry, cancel: &AtomicBool) -> Result<String, String> {
    hash_file(record, false, cancel)?.ok_or_else(||"Files with additional data streams or resource forks cannot be confirmed as redundant by TrapClean.".into())
}

pub fn find(
    db_path: &Path,
    scan_id: i64,
    cancel: Arc<AtomicBool>,
    mut progress: impl FnMut(u64),
) -> Result<Vec<DuplicateGroup>, String> {
    let index = Index::open(db_path)?;
    if matches!(
        index.get_scan(scan_id)?.status.as_str(),
        "queued" | "scanning"
    ) {
        return Err("Wait for the storage scan to finish.".into());
    }
    index.conn.execute_batch("CREATE TEMP TABLE duplicate_work(entry_id INTEGER PRIMARY KEY,len INTEGER NOT NULL,partial TEXT,full TEXT); CREATE INDEX duplicate_partial ON duplicate_work(len,partial); CREATE INDEX duplicate_full ON duplicate_work(full);").map_err(|e|e.to_string())?;
    index.conn.execute("INSERT INTO duplicate_work(entry_id,len) SELECT e.id,e.len FROM entries e WHERE e.scan_id=?1 AND e.kind='file' AND e.complete=1 AND e.protected=0 AND e.len>0 AND NOT EXISTS(SELECT 1 FROM entries h WHERE h.scan_id=e.scan_id AND h.device=e.device AND h.inode=e.inode AND h.id<e.id AND h.kind='file') AND e.len IN (SELECT len FROM entries WHERE scan_id=?1 AND kind='file' AND complete=1 AND protected=0 AND len>0 GROUP BY len HAVING count(*)>1)",[scan_id]).map_err(|e|e.to_string())?;
    let mut processed = 0;
    let mut failures = 0;
    let mut last = Instant::now();
    for partial in [true, false] {
        let mut after = 0;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(
                    "Duplicate analysis cancelled. No new duplicate selections were enabled."
                        .into(),
                );
            }
            let query = if partial {
                "SELECT entry_id FROM duplicate_work WHERE entry_id>?1 ORDER BY entry_id LIMIT 1"
            } else {
                "SELECT w.entry_id FROM duplicate_work w WHERE w.entry_id>?1 AND w.partial IS NOT NULL AND EXISTS(SELECT 1 FROM duplicate_work v WHERE v.len=w.len AND v.partial=w.partial AND v.entry_id<>w.entry_id) ORDER BY w.entry_id LIMIT 1"
            };
            let id = index
                .conn
                .query_row(query, [after], |r| r.get::<_, i64>(0))
                .optional()
                .map_err(|e| e.to_string())?;
            let Some(id) = id else {
                break;
            };
            after = id;
            let record = index.get_record(scan_id, id)?;
            if crate::rules::duplicate_protected(&record.path) {
                index
                    .conn
                    .execute("DELETE FROM duplicate_work WHERE entry_id=?1", [id])
                    .map_err(|e| e.to_string())?;
                continue;
            }
            match hash_file(&record, partial, &cancel) {
                Ok(Some(hash)) => {
                    index
                        .conn
                        .execute(
                            if partial {
                                "UPDATE duplicate_work SET partial=?1 WHERE entry_id=?2"
                            } else {
                                "UPDATE duplicate_work SET full=?1 WHERE entry_id=?2"
                            },
                            params![hash, id],
                        )
                        .map_err(|e| e.to_string())?;
                }
                Ok(None) => {
                    index
                        .conn
                        .execute("DELETE FROM duplicate_work WHERE entry_id=?1", [id])
                        .map_err(|e| e.to_string())?;
                }
                Err(_) if cancel.load(Ordering::Relaxed) => {
                    return Err(
                        "Duplicate analysis cancelled. No new duplicate selections were enabled."
                            .into(),
                    )
                }
                Err(_) => {
                    failures += 1;
                    index
                        .conn
                        .execute("DELETE FROM duplicate_work WHERE entry_id=?1", [id])
                        .map_err(|e| e.to_string())?;
                }
            }
            processed += 1;
            if last.elapsed() >= Duration::from_millis(250) {
                progress(processed);
                last = Instant::now();
            }
        }
    }
    if failures > 0 {
        return Err(format!("Duplicate analysis was incomplete: {failures} files changed or could not be read. Scan again; no new duplicate selections were enabled."));
    }
    let mut query=index.conn.prepare("SELECT full,len FROM duplicate_work WHERE full IS NOT NULL GROUP BY full,len HAVING count(*)>1 ORDER BY len DESC,full LIMIT ?1").map_err(|e|e.to_string())?;
    let hashes = query
        .query_map([MAX_GROUPS as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    let mut groups = Vec::with_capacity(hashes.len());
    for (hash, len) in hashes {
        if cancel.load(Ordering::Relaxed) {
            return Err(
                "Duplicate analysis cancelled. No new duplicate selections were enabled.".into(),
            );
        }
        let mut query = index
            .conn
            .prepare("SELECT entry_id FROM duplicate_work WHERE full=?1 ORDER BY entry_id LIMIT ?2")
            .map_err(|e| e.to_string())?;
        let ids = query
            .query_map(params![hash, MAX_MEMBERS as i64], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        let mut items = Vec::with_capacity(ids.len());
        for id in ids {
            let record = index.get_record(scan_id, id)?;
            // Rehash immediately before publishing, since earlier files may have changed meanwhile.
            if hash_current(&record, &cancel)? != hash {
                return Err(
                    "A file changed while duplicates were being verified. Scan again.".into(),
                );
            }
            items.push(record.row);
        }
        groups.push(DuplicateGroup {
            hash,
            bytes_each: len as u64,
            items,
        });
    }
    index
        .conn
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;
    let persisted = (|| {
        index
            .conn
            .execute("DELETE FROM duplicate_members WHERE scan_id=?1", [scan_id])
            .map_err(|e| e.to_string())?;
        index.conn.execute("UPDATE entries SET removable=0,disposition='keep',category='storage',reason='Duplicate analysis must be rerun before selecting this file.' WHERE scan_id=?1 AND category='duplicate'",[scan_id]).map_err(|e|e.to_string())?;
        for group in &mut groups {
            for item in &mut group.items {
                index
                    .conn
                    .execute(
                        "INSERT INTO duplicate_members(scan_id,entry_id,hash) VALUES(?1,?2,?3)",
                        params![scan_id, item.id, group.hash],
                    )
                    .map_err(|e| e.to_string())?;
                index.conn.execute("UPDATE entries SET category='duplicate',disposition='review',removable=1,reason='Independent files have identical complete SHA-256 content hashes.',consequence='Keep at least one verified copy. Identical content does not establish that every location has the same purpose.',uncertainty='Access times were recorded before reading. Usage is unknown. Allocation may be shared by clones or snapshots.',evidence='[\"Complete SHA-256 match\",\"Independent filesystem identities\"]' WHERE id=?1",[item.id]).map_err(|e|e.to_string())?;
                *item = index.get_entry(scan_id, item.id)?;
            }
        }
        Ok::<_, String>(())
    })();
    index
        .conn
        .execute_batch(if persisted.is_ok() {
            "COMMIT"
        } else {
            "ROLLBACK"
        })
        .map_err(|e| e.to_string())?;
    persisted?;
    progress(processed);
    Ok(groups)
}

pub fn verify_selection(
    index: &Index,
    scan_id: i64,
    ids: &[i64],
    cancel: &AtomicBool,
) -> Result<(), String> {
    let selected: HashSet<i64> = ids.iter().copied().collect();
    let selected_paths = ids
        .iter()
        .map(|id| index.get_record(scan_id, *id).map(|record| record.path))
        .collect::<Result<Vec<_>, _>>()?;
    let mut hashes = HashSet::new();
    for id in ids {
        let hash = index
            .conn
            .query_row(
                "SELECT hash FROM duplicate_members WHERE scan_id=?1 AND entry_id=?2",
                params![scan_id, id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(hash) = hash else {
            continue;
        };
        let record = index.get_record(scan_id, *id)?;
        if record.protected
            || crate::rules::duplicate_protected(&record.path)
            || !record.row.complete
            || hash_current(&record, cancel)? != hash
        {
            return Err("A selected duplicate changed. Run duplicate analysis again.".into());
        }
        hashes.insert(hash);
    }
    for hash in hashes {
        let mut query=index.conn.prepare("SELECT entry_id FROM duplicate_members WHERE scan_id=?1 AND hash=?2 ORDER BY entry_id").map_err(|e|e.to_string())?;
        let members = query
            .query_map(params![scan_id, hash], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        let mut retained = false;
        for id in members {
            let id = id.map_err(|e| e.to_string())?;
            if selected.contains(&id) {
                continue;
            }
            let record = index.get_record(scan_id, id)?;
            if selected_paths
                .iter()
                .any(|path| record.path.starts_with(path))
            {
                continue;
            }
            if hash_current(&record, cancel).is_ok_and(|current| current == hash) {
                retained = true;
                break;
            }
        }
        if !retained {
            return Err(
                "Keep at least one unchanged independent copy in each duplicate group.".into(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, std::path::PathBuf, std::path::PathBuf, i64) {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("root");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("a.txt"), [1u8; 200_000]).unwrap();
        fs::write(root.join("b.txt"), [1u8; 200_000]).unwrap();
        let mut different = vec![1u8; 200_000];
        different[100_000] = 2;
        fs::write(root.join("middle-diff.txt"), different).unwrap();
        let db = temp.path().join("index.sqlite");
        let index = Index::open(&db).unwrap();
        let id = index.begin_scan(&root).unwrap();
        scanner::scan(&db, id, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        (temp, root, db, id)
    }

    #[test]
    fn partial_hash_collision_is_not_a_duplicate() {
        let (_temp, _root, db, id) = fixture();
        let groups = find(&db, id, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].items.len(), 2);
        assert!(groups[0].items.iter().all(|e| e.name != "middle-diff.txt"));
        let index = Index::open(&db).unwrap();
        let all: Vec<_> = groups[0].items.iter().map(|e| e.id).collect();
        assert!(verify_selection(&index, id, &all, &AtomicBool::new(false)).is_err());
        assert!(verify_selection(&index, id, &all[..1], &AtomicBool::new(false)).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn hardlinks_are_not_independent_copies() {
        let (_temp, root, db, id) = fixture();
        fs::hard_link(root.join("a.txt"), root.join("hardlink.txt")).unwrap();
        let index = Index::open(&db).unwrap();
        let id = index.begin_scan(&root).unwrap_or(id);
        scanner::scan(&db, id, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let groups = find(&db, id, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert_eq!(groups[0].items.len(), 2);
    }

    #[test]
    fn modified_files_are_not_published_as_duplicates() {
        let (_temp, root, db, id) = fixture();
        fs::write(root.join("b.txt"), [9u8; 200_000]).unwrap();
        assert!(find(&db, id, Arc::new(AtomicBool::new(false)), |_| {}).is_err());
        let index = Index::open(&db).unwrap();
        let count = index
            .conn
            .query_row(
                "SELECT count(*) FROM duplicate_members WHERE scan_id=?1",
                [id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn same_length_change_with_restored_mtime_is_rejected() {
        use std::io::Write;
        let (_temp, root, db, id) = fixture();
        let path = root.join("b.txt");
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.write_all(&[9u8; 200_000]).unwrap();
        file.set_modified(modified).unwrap();
        drop(file);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        assert!(find(&db, id, Arc::new(AtomicBool::new(false)), |_| {}).is_err());
    }

    #[test]
    fn matching_sources_and_configs_are_not_cleanup_duplicates() {
        let (_temp, root, db, _id) = fixture();
        fs::create_dir_all(root.join("node_modules/example")).unwrap();
        fs::write(root.join("node_modules/example/a.js"), "module.exports=1").unwrap();
        fs::write(root.join("node_modules/example/b.js"), "module.exports=1").unwrap();
        fs::write(root.join("config-a.json"), "{\"secret\":true}").unwrap();
        fs::write(root.join("config-b.json"), "{\"secret\":true}").unwrap();
        let index = Index::open(&db).unwrap();
        let id = index.begin_scan(&root).unwrap();
        scanner::scan(&db, id, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let groups = find(&db, id, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert!(groups
            .iter()
            .flat_map(|group| &group.items)
            .all(|entry| entry.name.ends_with(".txt")));
    }

    #[test]
    fn retained_copy_inside_a_selected_folder_does_not_count() {
        let (_temp, root, db, _id) = fixture();
        fs::create_dir(root.join("copies")).unwrap();
        fs::write(root.join("copies/c.txt"), [1u8; 200_000]).unwrap();
        let index = Index::open(&db).unwrap();
        let id = index.begin_scan(&root).unwrap();
        scanner::scan(&db, id, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let groups = find(&db, id, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let a = groups[0]
            .items
            .iter()
            .find(|entry| entry.name == "a.txt")
            .unwrap()
            .id;
        let b = groups[0]
            .items
            .iter()
            .find(|entry| entry.name == "b.txt")
            .unwrap()
            .id;
        let folder = index
            .record_by_path(id, &root.join("copies"))
            .unwrap()
            .row
            .id;
        assert!(verify_selection(&index, id, &[a, b, folder], &AtomicBool::new(false)).is_err());
        assert!(verify_selection(&index, id, &[a, folder], &AtomicBool::new(false)).is_ok());
    }

    #[test]
    fn duplicate_group_results_have_bounded_members() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("root");
        fs::create_dir(&root).unwrap();
        for i in 0..25 {
            fs::write(root.join(format!("copy-{i}.txt")), [7u8; 32]).unwrap();
        }
        let db = temp.path().join("index.sqlite");
        let index = Index::open(&db).unwrap();
        let id = index.begin_scan(&root).unwrap();
        scanner::scan(&db, id, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let groups = find(&db, id, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].items.len(), MAX_MEMBERS);
        let stored = index
            .conn
            .query_row(
                "SELECT count(*) FROM duplicate_members WHERE scan_id=?1",
                [id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(stored, MAX_MEMBERS as i64);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn identical_primary_data_with_resource_forks_is_excluded() {
        use std::os::fd::AsRawFd;
        let (_temp, root, db, _id) = fixture();
        for (name, data) in [
            ("a.txt", b"persistent fork A".as_slice()),
            ("b.txt", b"persistent fork B".as_slice()),
        ] {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(root.join(name))
                .unwrap();
            assert_eq!(
                unsafe {
                    libc::fsetxattr(
                        file.as_raw_fd(),
                        c"com.apple.ResourceFork".as_ptr(),
                        data.as_ptr().cast(),
                        data.len(),
                        0,
                        0,
                    )
                },
                0
            );
        }
        let index = Index::open(&db).unwrap();
        let id = index.begin_scan(&root).unwrap();
        scanner::scan(&db, id, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert!(find(&db, id, Arc::new(AtomicBool::new(false)), |_| {})
            .unwrap()
            .is_empty());
        let record = index.record_by_path(id, &root.join("a.txt")).unwrap();
        assert!(hash_current(&record, &AtomicBool::new(false)).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn identical_primary_data_with_named_streams_is_excluded() {
        let (_temp, root, db, _id) = fixture();
        fs::write(root.join("a.txt:extra"), b"persistent stream A").unwrap();
        fs::write(root.join("b.txt:extra"), b"persistent stream B").unwrap();
        let index = Index::open(&db).unwrap();
        let id = index.begin_scan(&root).unwrap();
        scanner::scan(&db, id, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert!(find(&db, id, Arc::new(AtomicBool::new(false)), |_| {})
            .unwrap()
            .is_empty());
        let record = index.record_by_path(id, &root.join("a.txt")).unwrap();
        assert!(hash_current(&record, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn cancellation_does_not_enable_selection() {
        let (_temp, _root, db, id) = fixture();
        assert!(find(&db, id, Arc::new(AtomicBool::new(true)), |_| {}).is_err());
    }
}
