//! Regression guard for cas-2eb3 / cas-ed15.
//!
//! Before cas-ed15, `cas-cli/src/cli/cloud.rs::execute_pull` had its own inline
//! `ureq::get(format!("{}/api/sync/pull", ...))` URL builder that never appended
//! `project_id=`. The `cas cloud pull` CLI command therefore issued unscoped pulls
//! and imported cross-project rows into the local DB — the cas-2eb3 contamination
//! vector.
//!
//! These tests lock in two invariants on the `cas` crate:
//!
//! 1. **Source-level**: there is exactly one production URL builder for
//!    `/api/sync/pull`, and it lives in the scoped syncer
//!    (`cas-cli/src/cloud/syncer/pull.rs`). Any future regression that
//!    re-introduces a second inline builder will fail this test.
//!
//! 2. **Wire-level**: when `CloudSyncer::pull` is invoked, the URL on the wire
//!    includes a `project_id=` query parameter. This is the runtime contract
//!    the CLI now depends on.

use std::fs;
use std::path::{Path, PathBuf};

/// Roots that contain shipped source code. Tests, fixtures, and benches are
/// excluded — they are allowed to construct pull URLs freely.
fn production_source_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Recursively collect every `.rs` file under `dir`.
fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Tests/benches/fixtures live elsewhere; nothing to skip under src/.
            collect_rust_files(&path, out);
        } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
            out.push(path);
        }
    }
}

/// Strip line comments and block comments so we never match a literal that
/// only appears in a doc comment. This is a coarse pass — it does not
/// implement the full Rust tokenizer, but it correctly handles `// ...`
/// to end-of-line and `/* ... */` (non-nested). Inline strings are not
/// stripped because the bug is *about* a string literal.
fn strip_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < bytes.len() {
        // Line comment
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            // Skip to end of line; preserve the newline so line counts roughly align.
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Block comment
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = i.saturating_add(2).min(bytes.len());
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[test]
fn only_one_production_pull_url_builder_exists() {
    let root = production_source_root();
    let mut files = Vec::new();
    collect_rust_files(&root, &mut files);

    // Files that legitimately contain `/api/sync/pull` as code (not comments).
    let mut hits: Vec<(PathBuf, Vec<usize>)> = Vec::new();
    for path in &files {
        let Ok(src) = fs::read_to_string(path) else {
            continue;
        };
        let stripped = strip_comments(&src);
        let mut line_numbers = Vec::new();
        for (i, line) in stripped.lines().enumerate() {
            if line.contains("/api/sync/pull") {
                line_numbers.push(i + 1);
            }
        }
        if !line_numbers.is_empty() {
            hits.push((path.clone(), line_numbers));
        }
    }

    // The single allowed builder lives in the scoped syncer.
    let expected = root.join("cloud").join("syncer").join("pull.rs");

    let unexpected: Vec<_> = hits.iter().filter(|(p, _)| p != &expected).collect();

    assert!(
        unexpected.is_empty(),
        "Found unexpected production `/api/sync/pull` reference(s) outside the scoped syncer.\n\
         This is a cas-2eb3 / cas-ed15 regression: every code path that issues a\n\
         `/api/sync/pull` request MUST go through `CloudSyncer::pull`, which appends\n\
         `?project_id=`. A second builder will issue unscoped pulls and re-introduce\n\
         the cross-project contamination this guard exists to prevent.\n\
         Offenders:\n{}\n\n\
         Fix: route the new caller through `crate::cloud::CloudSyncer::pull` (see\n\
         `cas-cli/src/cli/cloud.rs::execute_pull` for the canonical pattern), or\n\
         extend the syncer surface if a new entity kind is needed.",
        unexpected
            .iter()
            .map(|(p, ls)| format!(
                "  - {} (lines: {})",
                p.display(),
                ls.iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    // And the scoped builder must still be present — if it was renamed or
    // moved we want this test to fail loudly rather than silently pass.
    assert!(
        hits.iter().any(|(p, _)| p == &expected),
        "Expected the scoped pull URL builder at {} to remain. If you moved it, \
         update this regression test to point at the new location.",
        expected.display(),
    );
}

#[test]
fn scoped_pull_builder_appends_project_id() {
    // Belt-and-suspenders source-level assertion: the one allowed builder
    // must, in the same file, also append `project_id=`. This catches a
    // regression where someone deletes the project_id line in pull.rs
    // without touching the URL format string.
    let pull_rs =
        production_source_root().join("cloud").join("syncer").join("pull.rs");
    let src = fs::read_to_string(&pull_rs).expect("read syncer/pull.rs");
    assert!(
        src.contains("project_id="),
        "{} must construct a `project_id=` query parameter — that's the scoping \
         contract every `/api/sync/pull` request depends on.",
        pull_rs.display(),
    );
    assert!(
        src.contains("get_project_canonical_id"),
        "{} must call `get_project_canonical_id()` to resolve the project scope.",
        pull_rs.display(),
    );
}

#[tokio::test]
async fn cloud_syncer_pull_request_carries_project_id_on_the_wire() {
    use std::sync::Arc;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Spin up a mock cloud, configure a syncer pointing at it, and assert
    // that the URL on the wire carries `project_id=<resolved>`. This locks
    // in the runtime contract `execute_pull` now depends on.
    let server = MockServer::start().await;

    // Resolve the canonical project ID using the same code path the syncer
    // does. From inside `cas-cli/tests/`, `find_cas_root_from_cas_worktree`
    // does not fire (no `.cas/worktrees/` segment), so the resolver walks
    // up until it finds the repo's `.cas/` and returns its folder name —
    // which is `cas-src` for this checkout.
    let expected_project_id = cas::cloud::get_project_canonical_id()
        .expect("get_project_canonical_id should succeed inside the cas-src checkout");

    Mock::given(method("GET"))
        .and(path("/api/sync/pull"))
        .and(query_param("project_id", expected_project_id.as_str()))
        .and(header("Authorization", "Bearer test-token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "entries": [],
                "tasks": [],
                "rules": [],
                "skills": [],
                "pulled_at": chrono::Utc::now().to_rfc3339(),
            })),
        )
        .expect(1) // exactly one matching request
        .mount(&server)
        .await;

    let cloud_config = cas::cloud::CloudConfig {
        endpoint: server.uri(),
        token: Some("test-token".to_string()),
        ..Default::default()
    };

    // Spin up an in-memory store set. The syncer needs trait objects but
    // we never actually need to upsert anything — the response body is empty.
    let tmp = tempfile::tempdir().expect("tempdir");
    let cas_root = tmp.path().join(".cas");
    std::fs::create_dir_all(&cas_root).expect("mkdir .cas");

    let store = cas::store::open_store(&cas_root).expect("open store");
    let task_store = cas::store::open_task_store(&cas_root).expect("open task store");
    let rule_store = cas::store::open_rule_store(&cas_root).expect("open rule store");
    let skill_store = cas::store::open_skill_store(&cas_root).expect("open skill store");

    let queue = cas::cloud::SyncQueue::open(&cas_root).expect("open queue");
    queue.init().expect("init queue");

    let syncer = cas::cloud::CloudSyncer::new(
        Arc::new(queue),
        cloud_config,
        cas::cloud::CloudSyncerConfig::default(),
    );

    let result = syncer
        .pull(
            store.as_ref(),
            task_store.as_ref(),
            rule_store.as_ref(),
            skill_store.as_ref(),
        )
        .expect("pull should succeed against the mock");

    assert!(
        result.errors.is_empty(),
        "Pull should not produce errors against the matching mock; got: {:?}",
        result.errors,
    );
    // wiremock's `.expect(1)` enforces that exactly one matching request fired.
    // If `project_id=` is missing or wrong, no mock matches → 404 → CasError.
}
