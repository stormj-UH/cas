//! Factory MCP Tool Integration Tests
//!
//! Tests the factory MCP tool handlers (`mcp__cas__coordination`) by constructing
//! a `CasService` with a temp CAS directory and calling `factory()` directly.
//! Verifies input validation, queue side effects, and response formatting.
//!
//! # Running
//! Some tests modify process-global environment variables (`CAS_AGENT_ROLE`,
//! `CAS_FACTORY_WORKER_NAMES`). Run single-threaded to avoid races:
//! ```bash
//! cargo test --test factory_mcp_ops_test -- --nocapture --test-threads=1
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use cas::mcp::{CasCore, CasService};
use cas::store::{
    AgentStore, PromptQueueStore, SpawnQueueStore, TaskStore, init_cas_dir, open_agent_store,
    open_prompt_queue_store, open_spawn_queue_store, open_task_store,
};
use cas::types::{Agent, Task, TaskStatus, TaskType};
use cas_mcp::types::FactoryRequest;
use cas_types::AgentRole;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::RawContent;
use tempfile::TempDir;

// =============================================================================
// Test Fixture
// =============================================================================

struct FactoryTestEnv {
    _temp: TempDir,
    cas_root: PathBuf,
    service: CasService,
}

impl FactoryTestEnv {
    fn new() -> Self {
        Self::with_agent_id("test-agent-id")
    }

    fn with_agent_id(agent_id: &str) -> Self {
        let temp = TempDir::new().expect("Failed to create temp dir");
        let cas_root = init_cas_dir(temp.path()).expect("Failed to init CAS dir");

        let core = CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing(agent_id.to_string());
        let service = CasService::new(core, None);

        Self {
            _temp: temp,
            cas_root,
            service,
        }
    }

    fn create_epic(&self, title: &str) -> String {
        let store = self.task_store();
        let id = store.generate_id().expect("generate_id");
        let mut task = Task::new(id.clone(), title.to_string());
        task.task_type = TaskType::Epic;
        store.add(&task).expect("add epic");
        id
    }

    fn register_worker(&self, name: &str) -> String {
        let store = self.agent_store();
        let id = Agent::generate_fallback_id();
        let mut agent = Agent::new(id.clone(), name.to_string());
        agent.role = AgentRole::Worker;
        store.register(&agent).expect("register worker");
        id
    }

    fn register_worker_with_metadata(
        &self,
        name: &str,
        metadata: HashMap<String, String>,
    ) -> String {
        let store = self.agent_store();
        let id = Agent::generate_fallback_id();
        let mut agent = Agent::new(id.clone(), name.to_string());
        agent.role = AgentRole::Worker;
        agent.metadata = metadata;
        store.register(&agent).expect("register worker");
        id
    }

    /// Register a worker with its `last_heartbeat` backdated so
    /// `factory_worker_status` classifies it as DEAD (elapsed > 30s).
    ///
    /// Used by the cas-5b1c worker_status integration test to drive the
    /// `[DEAD]` label + transcript-path surfacing branch without waiting
    /// 30 seconds of real time.
    fn register_stale_worker_with_clone_path(
        &self,
        name: &str,
        clone_path: &str,
        stale_secs: i64,
    ) -> String {
        let store = self.agent_store();
        let id = Agent::generate_fallback_id();
        let mut agent = Agent::new(id.clone(), name.to_string());
        agent.role = AgentRole::Worker;
        agent
            .metadata
            .insert("clone_path".to_string(), clone_path.to_string());
        // Backdate BOTH last_heartbeat and registered_at so this fixture
        // survives any future change that adds `registered_at` to the
        // stale-criteria set (adversarial cas-5b1c review A5). Current
        // `list_stale(threshold_secs)` keys on last_heartbeat only, but the
        // fixture is a test-stability anchor — backdating both is cheap
        // insurance against silent regression of the prune criteria.
        let staleness = chrono::Duration::seconds(stale_secs);
        agent.last_heartbeat = chrono::Utc::now() - staleness;
        agent.registered_at = chrono::Utc::now() - staleness;
        store.register(&agent).expect("register stale worker");
        id
    }

    fn register_supervisor(&self, name: &str) -> String {
        let store = self.agent_store();
        let id = Agent::generate_fallback_id();
        let mut agent = Agent::new(id.clone(), name.to_string());
        agent.role = AgentRole::Supervisor;
        store.register(&agent).expect("register supervisor");
        id
    }

    fn agent_store(&self) -> Arc<dyn AgentStore> {
        open_agent_store(&self.cas_root).expect("open agent store")
    }

    fn task_store(&self) -> Arc<dyn TaskStore> {
        open_task_store(&self.cas_root).expect("open task store")
    }

    fn spawn_queue(&self) -> Arc<dyn SpawnQueueStore> {
        open_spawn_queue_store(&self.cas_root).expect("open spawn queue")
    }

    fn prompt_queue(&self) -> Arc<dyn PromptQueueStore> {
        open_prompt_queue_store(&self.cas_root).expect("open prompt queue")
    }
}

/// Mutex to serialize tests that modify environment variables.
/// Env vars are process-global, so concurrent tests would interfere.
static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// RAII guard for environment variables. Acquires ENV_MUTEX.
struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn set(vars: &[(&str, &str)]) -> Self {
        let lock = ENV_MUTEX.lock().unwrap();
        let mut saved = Vec::with_capacity(vars.len());
        for (key, value) in vars {
            let key = (*key).to_string();
            let prev = std::env::var(&key).ok();
            unsafe { std::env::set_var(&key, value) };
            saved.push((key, prev));
        }
        Self { saved, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, prev) in self.saved.drain(..) {
            match prev {
                Some(val) => unsafe { std::env::set_var(&key, val) },
                None => unsafe { std::env::remove_var(&key) },
            }
        }
        // _lock drops here, releasing the mutex
    }
}

fn factory_req(action: &str) -> FactoryRequest {
    FactoryRequest {
        action: action.to_string(),
        id: None,
        count: None,
        worker_names: None,
        target: None,
        message: None,
        force: None,
        branch: None,
        older_than_secs: None,
        isolate: None,
        remind_message: None,
        remind_delay_secs: None,
        remind_event: None,
        remind_filter: None,
        remind_id: None,
        remind_ttl_secs: None,
    }
}

fn get_text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match &c.raw {
            RawContent::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// =============================================================================
// spawn_workers tests
// =============================================================================

#[tokio::test]
async fn test_spawn_workers_requires_epic() {
    let env = FactoryTestEnv::new();

    let req = factory_req("spawn_workers");
    let result = env.service.factory(Parameters(req)).await;

    assert!(result.is_err(), "Should fail without epic");
    let err = result.unwrap_err();
    assert!(
        err.message.contains("No active EPIC"),
        "Error should mention missing EPIC: {}",
        err.message
    );
}

#[tokio::test]
async fn test_spawn_workers_enqueues_with_epic() {
    let env = FactoryTestEnv::new();
    env.create_epic("Test Epic");

    let mut req = factory_req("spawn_workers");
    req.count = Some(3);
    req.worker_names = Some("alpha,beta,gamma".to_string());

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok(), "Should succeed with epic");

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("alpha, beta, gamma"),
        "Should list worker names: {text}"
    );

    // Verify queue
    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 1, "Should have 1 spawn queue entry");
    assert_eq!(entries[0].action, cas_store::SpawnAction::Spawn);
    assert_eq!(entries[0].worker_names, vec!["alpha", "beta", "gamma"]);
}

#[tokio::test]
async fn test_spawn_workers_isolate_flag() {
    let env = FactoryTestEnv::new();
    env.create_epic("Test Epic");

    let mut req = factory_req("spawn_workers");
    req.count = Some(2);
    req.isolate = Some(true);

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 1);
    assert!(entries[0].isolate, "Should have isolate=true");
}

#[tokio::test]
async fn test_spawn_workers_closed_epic_not_counted() {
    let env = FactoryTestEnv::new();

    // Create an epic and close it
    let epic_id = env.create_epic("Closed Epic");
    let store = env.task_store();
    let mut task = store.get(&epic_id).expect("get epic");
    task.status = TaskStatus::Closed;
    store.update(&task).expect("close epic");

    let req = factory_req("spawn_workers");
    let result = env.service.factory(Parameters(req)).await;

    assert!(result.is_err(), "Closed epic should not count as active");
}

// =============================================================================
// shutdown_workers tests
// =============================================================================

#[tokio::test]
async fn test_shutdown_workers_validates_existence() {
    let env = FactoryTestEnv::new();
    env.register_worker("alice");

    let mut req = factory_req("shutdown_workers");
    req.worker_names = Some("alice,charlie".to_string());

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_err(), "Should fail for nonexistent worker");

    let err = result.unwrap_err();
    assert!(
        err.message.contains("charlie"),
        "Error should mention missing worker: {}",
        err.message
    );
}

#[tokio::test]
async fn test_shutdown_workers_enqueues() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_worker("alice");
    env.register_worker("bob");

    let mut req = factory_req("shutdown_workers");
    req.worker_names = Some("alice,bob".to_string());

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(text.contains("alice, bob"), "Should list workers: {text}");

    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].action, cas_store::SpawnAction::Shutdown);
    assert!(entries[0].worker_names.contains(&"alice".to_string()));
    assert!(entries[0].worker_names.contains(&"bob".to_string()));
}

#[tokio::test]
async fn test_shutdown_workers_all() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_worker("alice");

    let mut req = factory_req("shutdown_workers");
    req.count = Some(0);

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(text.contains("ALL workers"), "Should say ALL: {text}");
}

#[tokio::test]
async fn test_shutdown_workers_supervisor_scoping() {
    let env = FactoryTestEnv::new();
    env.register_worker("owned-1");
    env.register_worker("other-1");

    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_FACTORY_WORKER_NAMES", "owned-1"),
    ]);

    // Empty worker_names should auto-scope to owned workers
    let req = factory_req("shutdown_workers");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].worker_names, vec!["owned-1"]);
}

// =============================================================================
// worker_status tests
// =============================================================================

#[tokio::test]
async fn test_worker_status_empty() {
    let env = FactoryTestEnv::new();

    let req = factory_req("worker_status");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("No active agents"),
        "Should report no agents: {text}"
    );
}

#[tokio::test]
async fn test_worker_status_shows_agents() {
    // Acquire env mutex to prevent concurrent tests from setting CAS_AGENT_ROLE=supervisor
    // which would activate supervisor scoping and filter out our test workers.
    let _guard = EnvGuard::set(&[]);

    let env = FactoryTestEnv::new();
    env.register_supervisor("sup-1");

    let mut meta = HashMap::new();
    meta.insert("clone_path".to_string(), "/tmp/worktree/wolf".to_string());
    env.register_worker_with_metadata("wolf", meta);
    env.register_worker("fox");

    let req = factory_req("worker_status");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("Workers (2)"),
        "Should show 2 workers: {text}"
    );
    assert!(text.contains("wolf"), "Should list wolf: {text}");
    assert!(text.contains("fox"), "Should list fox: {text}");
    assert!(
        text.contains("/tmp/worktree/wolf"),
        "Should show clone path: {text}"
    );
}

/// cas-5b1c integration coverage: a worker whose heartbeat is older than
/// `WORKER_STALE_SECS` (30s) is pruned out of the Active listing on the
/// next `factory_worker_status` call and reported in the "Filtered stale
/// agent record(s)" footer, while a live worker from the same call stays
/// visible. This pins the supervisor-facing UX contract that stale
/// workers disappear promptly once past the threshold.
///
/// Implementation note: `factory_worker_status` does its opportunistic
/// prune BEFORE rendering the Active list, so in the common path a
/// stale Worker transitions out of Active and never hits the `[DEAD]`
/// label / transcript-path render branch. The render-time DEAD branch
/// only fires when `mark_stale` fails (DB lock, etc.) — that code path
/// is cheap unit coverage at the `resolve_transcript` / `render_transcript_block`
/// level (see the `mcp::tools::service::factory_ops::tests` module),
/// now with glob-based resolution landed via cas-900b. Here we test the
/// prune-success integration.
#[tokio::test]
async fn test_worker_status_prunes_stale_worker_and_keeps_live_one() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();

    // Live worker: default heartbeat = now, stays in Active.
    env.register_worker("live-fox");
    // Stale worker: heartbeat backdated 40s so list_stale(30) catches it.
    let stale_id = env.register_stale_worker_with_clone_path(
        "dead-wolf",
        "/tmp/cas-worktrees/dead-wolf",
        40,
    );

    let req = factory_req("worker_status");
    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("worker_status call should succeed");
    let text = get_text(&result);

    // Live worker must appear; stale must not.
    assert!(
        text.contains("live-fox"),
        "live worker must appear in Active listing. Got:\n{text}"
    );
    assert!(
        !text.contains("dead-wolf"),
        "stale worker must be pruned out of the Active listing. Got:\n{text}"
    );
    assert!(
        !text.contains(&stale_id),
        "stale worker's id must not appear in render. Got:\n{text}"
    );

    // The footer must account for the prune so operators can see the
    // pruned count at a glance.
    assert!(
        text.contains("Filtered stale agent record(s): 1"),
        "prune summary must report exactly 1 stale record filtered. Got:\n{text}"
    );
    assert!(
        text.contains("30s heartbeat age"),
        "footer must reference the 30s worker threshold. Got:\n{text}"
    );
}

// =============================================================================
// worker_activity tests
// =============================================================================

#[tokio::test]
async fn test_worker_activity_empty() {
    let env = FactoryTestEnv::new();

    let req = factory_req("worker_activity");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("No recent worker activity"),
        "Should report no activity: {text}"
    );
}

// =============================================================================
// clear_context tests
// =============================================================================

#[tokio::test]
async fn test_clear_context_enqueues() {
    let env = FactoryTestEnv::with_agent_id("test-sup");

    let store = env.agent_store();
    let agent = Agent::new("test-sup".to_string(), "supervisor".to_string());
    store.register(&agent).expect("register");

    let mut req = factory_req("clear_context");
    req.target = Some("wolf".to_string());

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let prompts = env.prompt_queue().peek_all(10).expect("peek");
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].target, "wolf");
    assert_eq!(prompts[0].prompt, "/clear");
}

#[tokio::test]
async fn test_clear_context_all_workers() {
    let env = FactoryTestEnv::with_agent_id("test-sup");

    let store = env.agent_store();
    let agent = Agent::new("test-sup".to_string(), "supervisor".to_string());
    store.register(&agent).expect("register");

    let mut req = factory_req("clear_context");
    req.target = Some("all_workers".to_string());

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("all workers"),
        "Should mention all workers: {text}"
    );

    let prompts = env.prompt_queue().peek_all(10).expect("peek");
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].target, "all_workers");
    assert_eq!(prompts[0].prompt, "/clear");
}

// =============================================================================
// my_context tests
// =============================================================================

#[tokio::test]
async fn test_my_context_shows_agent_info() {
    let env = FactoryTestEnv::with_agent_id("ctx-agent-id");

    let store = env.agent_store();
    let mut agent = Agent::new("ctx-agent-id".to_string(), "ctx-supervisor".to_string());
    agent.role = AgentRole::Supervisor;
    store.register(&agent).expect("register");

    let req = factory_req("my_context");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(text.contains("ctx-supervisor"), "Should show name: {text}");
    assert!(text.contains("Supervisor"), "Should show role: {text}");
    assert!(text.contains("ctx-agent-id"), "Should show ID: {text}");
    assert!(text.contains("None (idle)"), "Should show no tasks: {text}");
}

// =============================================================================
// gc_report tests
// =============================================================================

#[tokio::test]
async fn test_gc_report_empty() {
    let env = FactoryTestEnv::new();

    let req = factory_req("gc_report");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("Stale agents: 0"),
        "Should show 0 stale: {text}"
    );
    assert!(
        text.contains("Pending prompts: 0"),
        "Should show 0 prompts: {text}"
    );
}

#[tokio::test]
async fn test_gc_report_shows_pending_prompts() {
    let env = FactoryTestEnv::new();

    // Add some pending prompts
    let pq = env.prompt_queue();
    pq.enqueue("src", "wolf", "do stuff").expect("enqueue");
    pq.enqueue("src", "fox", "do other stuff").expect("enqueue");

    let req = factory_req("gc_report");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("Pending prompts: 2"),
        "Should show 2 prompts: {text}"
    );
}

// =============================================================================
// gc_cleanup tests
// =============================================================================

#[tokio::test]
async fn test_gc_cleanup_without_force() {
    let env = FactoryTestEnv::new();

    // Add pending prompts
    let pq = env.prompt_queue();
    pq.enqueue("src", "wolf", "test").expect("enqueue");

    let req = factory_req("gc_cleanup");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("Prompt queue entries cleared: 0"),
        "Should NOT clear prompts without force: {text}"
    );

    // Prompts should still be pending
    assert_eq!(pq.pending_count().expect("count"), 1);
}

#[tokio::test]
async fn test_gc_cleanup_with_force() {
    let env = FactoryTestEnv::new();

    let pq = env.prompt_queue();
    pq.enqueue("src", "wolf", "test1").expect("enqueue");
    pq.enqueue("src", "fox", "test2").expect("enqueue");

    let mut req = factory_req("gc_cleanup");
    req.force = Some(true);

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("Prompt queue entries cleared: 2"),
        "Should clear prompts with force: {text}"
    );

    assert_eq!(pq.pending_count().expect("count"), 0);
}

// =============================================================================
// Sequence tests
// =============================================================================

#[tokio::test]
async fn test_spawn_then_shutdown_sequence() {
    let env = FactoryTestEnv::new();
    env.create_epic("Sequence Epic");
    env.register_worker("alpha");

    // Spawn
    let mut req = factory_req("spawn_workers");
    req.count = Some(2);
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    // Shutdown
    let mut req = factory_req("shutdown_workers");
    req.worker_names = Some("alpha".to_string());
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    // Both should be in queue
    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 2, "Should have 2 queue entries");
    assert_eq!(entries[0].action, cas_store::SpawnAction::Spawn);
    assert_eq!(entries[1].action, cas_store::SpawnAction::Shutdown);
}

#[tokio::test]
async fn test_unknown_action() {
    let env = FactoryTestEnv::new();

    let req = factory_req("invalid_action");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert!(
        err.message.contains("Unknown factory action"),
        "Should report unknown action: {}",
        err.message
    );
}
