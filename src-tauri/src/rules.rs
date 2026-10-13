use crate::index::Index;
use rusqlite::{params, OptionalExtension};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub fn protected(path: &Path) -> bool {
    let parts: Vec<String> = path
        .components()
        .map(|v| v.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    if parts.iter().any(|v| {
        matches!(
            v.as_str(),
            ".git"
                | ".ssh"
                | ".gnupg"
                | ".aws"
                | ".azure"
                | ".kube"
                | "keychains"
                | "backups"
                | "backup"
                | "system"
                | "windows"
                | "program files"
                | "program files (x86)"
        )
    }) {
        return true;
    }
    let Some(name) = parts.last() else {
        return true;
    };
    let installed_dependency = parts.iter().any(|part| part == "node_modules");
    let generated_json = name == ".rustc_info.json"
        || parts
            .iter()
            .any(|part| part == ".fingerprint" || part == "_cacache");
    let source_or_config = !installed_dependency
        && (matches!(
            name.as_str(),
            "cargo.toml"
                | "cargo.lock"
                | "package.json"
                | "package-lock.json"
                | "npm-shrinkwrap.json"
                | "pnpm-lock.yaml"
                | "yarn.lock"
                | "bun.lock"
                | "bun.lockb"
        ) || [
            ".rs", ".ts", ".tsx", ".js", ".jsx", ".c", ".h", ".cpp", ".hpp", ".go", ".py", ".rb",
            ".swift", ".java", ".kt", ".lua", ".luau", ".svelte", ".vue", ".css", ".scss", ".html",
            ".svg", ".md", ".sql", ".plist", ".sh", ".ps1", ".bat", ".cmd", ".toml", ".yaml",
            ".yml", ".ini", ".conf",
        ]
        .iter()
        .any(|suffix| name.ends_with(suffix))
            || (!generated_json && name.ends_with(".json")));
    source_or_config
        || name == ".env"
        || name.starts_with(".env.")
        || matches!(
            name.as_str(),
            "id_rsa" | "id_ed25519" | "credentials" | "credentials.json" | "secrets.json"
        )
        || [
            ".sqlite", ".sqlite3", ".db", ".db-wal", ".db-shm", ".pem", ".key", ".p12", ".pfx",
            ".kdbx", ".bak",
        ]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

pub fn duplicate_protected(path: &Path) -> bool {
    if protected(path) {
        return true;
    }
    let Some(name) = path.file_name() else {
        return true;
    };
    let name = name.to_string_lossy().to_ascii_lowercase();
    [
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".c", ".h", ".cpp", ".hpp", ".go", ".py", ".rb",
        ".swift", ".java", ".kt", ".lua", ".luau", ".svelte", ".vue", ".css", ".scss", ".html",
        ".svg", ".md", ".sql", ".plist", ".sh", ".ps1", ".bat", ".cmd", ".json", ".toml", ".yaml",
        ".yml", ".ini", ".conf", ".lock",
    ]
    .iter()
    .any(|suffix| name.ends_with(suffix))
        || matches!(
            name.as_str(),
            "cargo.lock"
                | "package-lock.json"
                | "npm-shrinkwrap.json"
                | "pnpm-lock.yaml"
                | "yarn.lock"
                | "bun.lock"
                | "bun.lockb"
        )
}

struct Rule {
    category: &'static str,
    disposition: &'static str,
    reason: &'static str,
    consequence: &'static str,
    uncertainty: &'static str,
    evidence: Vec<&'static str>,
    removable: bool,
}

fn update(index: &Index, id: i64, project: Option<&str>, rule: Rule) -> Result<(), String> {
    let evidence = serde_json::to_string(&rule.evidence).map_err(|e| e.to_string())?;
    index.conn.execute("UPDATE entries SET category=?1,disposition=?2,reason=?3,consequence=?4,uncertainty=?5,project=?6,evidence=?7,removable=?8 WHERE id=?9",params![rule.category,rule.disposition,rule.reason,rule.consequence,rule.uncertainty,project,evidence,rule.removable,id]).map_err(|e|e.to_string())?;
    Ok(())
}

fn regular_child(index: &Index, scan_id: i64, parent: i64, name: &str) -> Result<bool, String> {
    Ok(index.conn.query_row("SELECT complete=1 AND kind='file' FROM entries WHERE scan_id=?1 AND parent_id=?2 AND name=?3 LIMIT 1",params![scan_id,parent,name],|r|r.get::<_,bool>(0)).optional().map_err(|e|e.to_string())?.unwrap_or(false))
}

fn directory_child(index: &Index, scan_id: i64, parent: i64, name: &str) -> Result<bool, String> {
    Ok(index.conn.query_row("SELECT kind='directory' FROM entries WHERE scan_id=?1 AND parent_id=?2 AND name=?3 LIMIT 1",params![scan_id,parent,name],|r|r.get::<_,bool>(0)).optional().map_err(|e|e.to_string())?.unwrap_or(false))
}

fn has_target_config(index: &Index, scan_id: i64, parent: i64) -> Result<bool, String> {
    let mut next = Some(parent);
    while let Some(id) = next {
        if let Some(cargo) = index.optional_child(scan_id, id, ".cargo")? {
            if regular_child(index, scan_id, cargo, "config")?
                || regular_child(index, scan_id, cargo, "config.toml")?
            {
                return Ok(true);
            }
        }
        next = index.get_entry(scan_id, id)?.parent_id;
    }
    Ok(false)
}

fn cargo_layout(index: &Index, scan_id: i64, id: i64) -> Result<bool, String> {
    let unexpected=index.conn.query_row("SELECT count(*) FROM entries WHERE scan_id=?1 AND parent_id=?2 AND name NOT IN ('debug','release','doc','.rustc_info.json','CACHEDIR.TAG','.cargo-lock')",params![scan_id,id],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?;
    Ok(unexpected == 0
        && (directory_child(index, scan_id, id, "debug")?
            || directory_child(index, scan_id, id, "release")?
            || directory_child(index, scan_id, id, "doc")?))
}

fn source_inside(index: &Index, scan_id: i64, id: i64) -> Result<bool, String> {
    let found=index.conn.query_row("WITH RECURSIVE descendants(id) AS (SELECT id FROM entries WHERE parent_id=?1 UNION ALL SELECT e.id FROM entries e JOIN descendants d ON e.parent_id=d.id) SELECT EXISTS(SELECT 1 FROM entries e JOIN descendants d ON d.id=e.id WHERE e.scan_id=?2 AND (e.name IN ('Cargo.toml','Cargo.lock','package.json','package-lock.json','pnpm-lock.yaml','yarn.lock','.env') OR lower(e.name) GLOB '*.rs' OR lower(e.name) GLOB '*.ts' OR lower(e.name) GLOB '*.tsx' OR lower(e.name) GLOB '*.js' OR lower(e.name) GLOB '*.c' OR lower(e.name) GLOB '*.cpp' OR lower(e.name) GLOB '*.go'))",params![id,scan_id],|r|r.get::<_,bool>(0)).map_err(|e|e.to_string())?;
    Ok(found)
}

fn mixed_cargo_files(
    index: &Index,
    scan_id: i64,
    id: i64,
    cancel: &AtomicBool,
) -> Result<bool, String> {
    if source_inside(index, scan_id, id)? {
        return Ok(true);
    }
    let mut query=index.conn.prepare("WITH RECURSIVE descendants(id) AS (SELECT id FROM entries WHERE parent_id=?1 UNION ALL SELECT e.id FROM entries e JOIN descendants d ON e.parent_id=d.id) SELECT e.id,e.name,e.raw_path FROM entries e JOIN descendants d ON d.id=e.id WHERE e.scan_id=?2 AND e.kind='file'").map_err(|e|e.to_string())?;
    let mut rows = query
        .query(params![id, scan_id])
        .map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        if cancel.load(Ordering::Relaxed) {
            return Ok(true);
        }
        let name: String = row.get(1).map_err(|e| e.to_string())?;
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".json") && name != ".rustc_info.json" {
            let path = crate::index::decode_path(row.get(2).map_err(|e| e.to_string())?);
            if !path
                .components()
                .any(|part| part.as_os_str() == ".fingerprint")
            {
                return Ok(true);
            }
        }
        let known = matches!(
            name.as_str(),
            ".rustc_info.json"
                | "CACHEDIR.TAG"
                | ".cargo-lock"
                | "output"
                | "stderr"
                | "root-output"
                | "invoked.timestamp"
        ) || name.starts_with("dep-")
            || name.starts_with("lib-")
            || name.starts_with("bin-")
            || name.starts_with("test-")
            || [
                ".rlib",
                ".rmeta",
                ".d",
                ".o",
                ".a",
                ".so",
                ".dylib",
                ".dll",
                ".exe",
                ".pdb",
                ".lib",
                ".wasm",
                ".bc",
                ".bin",
                ".json",
                ".lock",
                ".timestamp",
            ]
            .iter()
            .any(|suffix| lower.ends_with(suffix));
        if known {
            continue;
        }
        if name.contains('.') {
            return Ok(true);
        }
        let entry_id: i64 = row.get(0).map_err(|e| e.to_string())?;
        let record = index.get_record(scan_id, entry_id)?;
        #[cfg(unix)]
        let file = crate::native::open_directory(record.path.parent().ok_or("Missing parent")?)
            .and_then(|dir| {
                crate::native::open_child(
                    &dir,
                    Path::new(record.path.file_name().ok_or("Missing filename")?),
                )
            });
        #[cfg(windows)]
        let file = std::fs::File::open(&record.path).map_err(|e| e.to_string());
        let Ok(mut file) = file else {
            return Ok(true);
        };
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if crate::scanner::path_fingerprint(&record.path, &metadata)? != record.fingerprint {
            return Ok(true);
        }
        use std::io::Read;
        let mut magic = [0u8; 4];
        if file.read_exact(&mut magic).is_err() {
            return Ok(true);
        }
        let binary = magic == [0x7f, b'E', b'L', b'F']
            || matches!(
                magic,
                [0xcf, 0xfa, 0xed, 0xfe]
                    | [0xce, 0xfa, 0xed, 0xfe]
                    | [0xfe, 0xed, 0xfa, 0xcf]
                    | [0xfe, 0xed, 0xfa, 0xce]
                    | [0xca, 0xfe, 0xba, 0xbe]
            )
            || &magic[..2] == b"MZ";
        if !binary {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn classify(index: &Index, scan_id: i64, cancel: &AtomicBool) -> Result<(), String> {
    let mut after = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let item=index.conn.query_row("SELECT id,parent_id,name,complete,protected FROM entries WHERE scan_id=?1 AND id>?2 AND kind='directory' AND name IN ('target','node_modules','dist','build','.cache','_cacache') ORDER BY id LIMIT 1",params![scan_id,after],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<i64>>(1)?,r.get::<_,String>(2)?,r.get::<_,bool>(3)?,r.get::<_,bool>(4)?))).optional().map_err(|e|e.to_string())?;
        let Some((id, parent, name, complete, has_protected)) = item else {
            break;
        };
        after = id;
        let Some(parent) = parent else {
            continue;
        };
        // A cleanup unit owns its descendants; nested heuristic names are not separate selections.
        let inside_unit=index.conn.query_row("WITH RECURSIVE ancestors(id,parent_id,disposition) AS (SELECT id,parent_id,disposition FROM entries WHERE id=?1 UNION ALL SELECT e.id,e.parent_id,e.disposition FROM entries e JOIN ancestors a ON e.id=a.parent_id) SELECT EXISTS(SELECT 1 FROM ancestors WHERE disposition IN ('candidate','review'))",[parent],|r|r.get::<_,bool>(0)).map_err(|e|e.to_string())?;
        if inside_unit {
            continue;
        }
        let project = index.get_entry(scan_id, parent)?.relative_path;
        let project = if project.is_empty() {
            None
        } else {
            Some(project.as_str())
        };
        let mut rule = Rule {
            category: "mixed",
            disposition: "review",
            reason: "The directory name alone does not establish that its contents can be rebuilt.",
            consequence: "Removing this directory may lose local changes or application data.",
            uncertainty: "Origin and reconstruction are unverified. Usage is unknown.",
            evidence: vec![],
            removable: false,
        };
        if !complete {
            rule.reason="Some descendants were not inspected. Cleanup cannot be recommended for this directory.";
            rule.evidence.push("Incomplete scan coverage");
        } else if has_protected {
            rule.category = "protected";
            rule.reason =
                "This directory contains protected sources, configuration, credentials, persistent data, or backups.";
            rule.evidence.push("Protected or persistent data found");
        } else if name == "target" {
            let manifest = regular_child(index, scan_id, parent, "Cargo.toml")?;
            let sources = if let Some(src) = index.optional_child(scan_id, parent, "src")? {
                let row = index.get_entry(scan_id, src)?;
                row.kind == "directory"
                    && row.complete
                    && (regular_child(index, scan_id, src, "main.rs")?
                        || regular_child(index, scan_id, src, "lib.rs")?)
            } else {
                false
            };
            let marker = regular_child(index, scan_id, id, ".rustc_info.json")?;
            let layout = cargo_layout(index, scan_id, id)?;
            let configured = has_target_config(index, scan_id, parent)?;
            if manifest {
                rule.evidence.push("Cargo manifest present");
            }
            if sources {
                rule.evidence.push("Project source directory preserved");
            }
            if marker {
                rule.evidence.push("Cargo artifact marker present");
            }
            if layout {
                rule.evidence.push("Recognized Cargo output layout");
            }
            if configured {
                rule.evidence.push("Cargo target configuration found");
            }
            rule.category = "cargo-build";
            rule.consequence="Cargo will rebuild generated artifacts. The next build may take longer and require dependency downloads.";
            rule.uncertainty="Shared or external target directories are not fully inferred. Usage is unknown; clones and snapshots can reduce recovered space.";
            if manifest
                && sources
                && marker
                && layout
                && !configured
                && !mixed_cargo_files(index, scan_id, id, cancel)?
            {
                rule.disposition = "candidate";
                rule.reason="Cargo metadata and an ordinary output layout identify generated artifacts; the project source directory remains outside this selection.";
                rule.removable = true;
            } else {
                rule.reason="Cargo evidence is incomplete, output is mixed, or target configuration requires review.";
            }
        } else if name == "node_modules" {
            let manifest = regular_child(index, scan_id, parent, "package.json")?;
            let lock = regular_child(index, scan_id, parent, "package-lock.json")?
                || regular_child(index, scan_id, parent, "npm-shrinkwrap.json")?
                || regular_child(index, scan_id, parent, "pnpm-lock.yaml")?
                || regular_child(index, scan_id, parent, "yarn.lock")?
                || regular_child(index, scan_id, parent, "bun.lock")?
                || regular_child(index, scan_id, parent, "bun.lockb")?;
            let marker = regular_child(index, scan_id, id, ".package-lock.json")?;
            if manifest {
                rule.evidence.push("JavaScript manifest present");
            }
            if lock {
                rule.evidence.push("Dependency lockfile present");
            }
            if marker {
                rule.evidence.push("npm install metadata present");
            }
            rule.category = "javascript-dependencies";
            rule.reason="Installed JavaScript dependencies may be recoverable, but local package edits and download availability cannot be verified.";
            rule.consequence="Reinstall dependencies with the project's package manager before running or building it; reinstall scripts are not executed by TrapClean.";
            rule.uncertainty="A lockfile does not prove that dependencies remain available offline or that installed packages have no local modifications. Usage is unknown.";
            rule.removable = manifest && lock && marker;
        } else if name == "_cacache" {
            let parent_name = index.get_entry(scan_id, parent)?.name;
            let layout = directory_child(index, scan_id, id, "content-v2")?
                && directory_child(index, scan_id, id, "index-v5")?;
            let unexpected=index.conn.query_row("SELECT count(*) FROM entries WHERE parent_id=?1 AND name NOT IN ('content-v2','index-v5','tmp')",[id],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?;
            if matches!(parent_name.as_str(), ".npm" | "npm-cache") && layout && unexpected == 0 {
                rule.category = "cache";
                rule.disposition = "candidate";
                rule.removable = true;
                rule.reason = "This directory matches npm's content-addressed cache layout.";
                rule.consequence="npm may download dependencies again. Cached packages will no longer be available for offline installs.";
                rule.uncertainty="Usage is unknown; another npm process may be active. Snapshots and clones can reduce recovered space.";
                rule.evidence.push("npm content-addressed cache layout");
            }
        }
        update(index, id, project, rule)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn cargo_fixture() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("project");
        fs::create_dir_all(root.join("target/debug/deps")).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='fixture'\nversion='0.1.0'",
        )
        .unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {} ").unwrap();
        fs::write(root.join("target/.rustc_info.json"), "{}").unwrap();
        fs::write(root.join("target/debug/deps/lib.rlib"), [2u8; 512]).unwrap();
        let db = temp.path().join("scan.sqlite");
        (temp, root, db)
    }

    fn candidates(root: &Path, db: &Path) -> Vec<crate::model::EntryRow> {
        let index = Index::open(db).unwrap();
        let id = index.begin_scan(root).unwrap();
        scanner::scan(db, id, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        index.candidates(id, 100).unwrap()
    }

    #[test]
    fn cargo_needs_evidence_and_preserved_sources() {
        let (_temp, root, db) = cargo_fixture();
        let rows = candidates(&root, &db);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].disposition, "candidate");
        assert!(rows[0].removable);
        fs::remove_file(root.join("target/.rustc_info.json")).unwrap();
        let rows = candidates(&root, &db);
        assert_eq!(rows[0].disposition, "review");
        assert!(!rows[0].removable);
    }

    #[test]
    fn an_empty_source_directory_is_not_reconstruction_evidence() {
        let (_temp, root, db) = cargo_fixture();
        fs::remove_file(root.join("src/main.rs")).unwrap();
        let rows = candidates(&root, &db);
        assert_eq!(rows[0].disposition, "review");
        assert!(!rows[0].removable);
    }

    #[test]
    fn protected_and_mixed_descendants_block_cargo_cleanup() {
        let (_temp, root, db) = cargo_fixture();
        fs::write(root.join("target/debug/user.sqlite"), "persistent").unwrap();
        let rows = candidates(&root, &db);
        assert_eq!(rows[0].category, "protected");
        assert!(!rows[0].removable);
        fs::remove_file(root.join("target/debug/user.sqlite")).unwrap();
        fs::write(root.join("target/debug/precious.rs"), "source").unwrap();
        let rows = candidates(&root, &db);
        assert_eq!(rows[0].disposition, "review");
        assert!(!rows[0].removable);
    }

    #[test]
    fn unrecognized_data_inside_cargo_is_mixed() {
        let (_temp, root, db) = cargo_fixture();
        fs::write(root.join("target/debug/notes.txt"), "valuable").unwrap();
        assert!(!candidates(&root, &db)[0].removable);
        fs::remove_file(root.join("target/debug/notes.txt")).unwrap();
        fs::write(root.join("target/debug/notes"), "valuable").unwrap();
        assert!(!candidates(&root, &db)[0].removable);
    }

    #[test]
    fn cargo_nested_in_dependencies_still_rejects_unknown_json_data() {
        let (temp, root, db) = cargo_fixture();
        let dependencies = temp.path().join("node_modules");
        fs::create_dir(&dependencies).unwrap();
        let root_in_dependencies = dependencies.join("crate");
        fs::rename(root, &root_in_dependencies).unwrap();
        fs::write(
            root_in_dependencies.join("target/debug/user.json"),
            "{\"persistent\":true}",
        )
        .unwrap();
        assert!(!candidates(&root_in_dependencies, &db)[0].removable);
    }

    #[test]
    fn target_configuration_requires_review() {
        let (_temp, root, db) = cargo_fixture();
        fs::create_dir(root.join(".cargo")).unwrap();
        fs::write(
            root.join(".cargo/config.toml"),
            "[build]\ntarget-dir='shared'",
        )
        .unwrap();
        let rows = candidates(&root, &db);
        assert_eq!(rows[0].disposition, "review");
        assert!(!rows[0].removable);
    }

    #[cfg(unix)]
    #[test]
    fn partial_directory_cannot_be_a_candidate() {
        let (_temp, root, db) = cargo_fixture();
        std::os::unix::fs::symlink("/", root.join("target/debug/link")).unwrap();
        let rows = candidates(&root, &db);
        assert!(!rows[0].removable);
        assert!(rows[0]
            .evidence
            .iter()
            .any(|e| e == "Incomplete scan coverage"));
    }

    #[test]
    fn source_and_config_are_protected_outside_installed_dependencies() {
        assert!(protected(Path::new("/fixture/src/main.rs")));
        assert!(protected(Path::new("/fixture/settings.json")));
        assert!(protected(Path::new("/fixture/Cargo.toml")));
        assert!(!protected(Path::new(
            "/fixture/node_modules/example/index.js"
        )));
        assert!(protected(Path::new("/fixture/node_modules/example/.env")));
    }

    #[test]
    fn node_modules_stays_review_with_lockfile() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("project");
        fs::create_dir_all(root.join("node_modules/example")).unwrap();
        fs::write(root.join("package.json"), "{}").unwrap();
        fs::write(root.join("package-lock.json"), "{}").unwrap();
        fs::write(root.join("node_modules/.package-lock.json"), "{}").unwrap();
        fs::write(
            root.join("node_modules/example/index.js"),
            "module.exports=1",
        )
        .unwrap();
        let rows = candidates(&root, &temp.path().join("index.sqlite"));
        assert_eq!(rows[0].disposition, "review");
        assert!(rows[0].removable);
    }
}
