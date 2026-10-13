# Cleanup boundaries

The UI submits indexed IDs. Rust builds a short-lived, single-use plan containing known native paths, identities, metadata, sizes and local reasons. Typed confirmation refers to this exact plan. Selecting a root, parent overlap, protected entry, partial candidate, stale scan, unknown path, expired plan or unsupported action is refused.

Native path bytes/UTF-16 are stored separately from display text. Display paths never become deletion identities. Unix directory traversal uses no-follow descriptor-relative APIs; Windows uses opened reparse-aware handles and ancestor locks. Mounted filesystems, links and special objects are skipped. System directories, credentials, persistent databases, backups and sources/configurations have protection rules. Protected descendants prevent an entire cleanup unit being removable.

Cleanup atomically captures a selected item into a unique local staging directory before checking its complete subtree against the index. Unexpected names, removed children, identity changes, metadata changes, protected data, reparse points and mount crossings cause refusal and restoration where possible. Staging on the same disk does not reclaim space. Restoration never overwrites a concurrently created replacement; if it cannot restore, the result supplies the retained recovery location. Interrupted permanent deletion can be partial and cannot be undone.

macOS Trash uses a native file reference URL to bind the operation to the captured identity instead of re-canonicalizing an untrusted string. Windows uses native filesystem handles for permanent operations and the system Shell recycling API with explicit recycling/no-reparse flags for supported Trash operations. Unsupported or uncertain operations must fail closed. No Trash error automatically invokes permanent deletion. A third-party process changing the captured namespace during the OS recycling handoff is a platform-specific limitation to test, not a claim of absolute safety.

Duplicate analysis captures metadata before content reads, filters by size and partial hash, then verifies full SHA-256 in streaming reads with identity/metadata checks. Independent inode/file-ID membership excludes hardlink aliases. Cleanup rehashes selected duplicates and at least one independently retained member. It refuses selecting all verified copies, including a retained copy inside a selected parent folder.

No privilege elevation, project hooks, shell cleanup commands, generated commands, automatic provider selection, global Trash emptying, or background service is used. Process-handle activity is not proof of future use; currently the UI reports usage unknown. Allocation and free-space prediction do not account exactly for clones, compression, snapshots or unrelated filesystem activity.

Tests mutate only generated temporary fixtures. HTTP contract tests use a loopback mock and synthetic keys; no personal vault credentials or paid provider calls are needed for CI.
