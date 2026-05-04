use crate::mux::*;
use crate::spec::{Effort, WorkerSpec};
use std::path::PathBuf;

// ── cas-d571: effort config flows through Mux::factory() to PTY args ─────────
// Tests the full MuxConfig → Mux::factory_pane_configs() → PtyConfig.args chain.
// Uses `factory_pane_configs` (config-only, no spawn) so tests run without a
// real `claude` or `codex` binary present.

#[test]
fn factory_pane_configs_supervisor_effort_reaches_pty_args() {
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        supervisor_effort: Some("low".to_string()),
        worker_effort: Some("high".to_string()),
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Claude,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    let (_, sup_config) = configs
        .iter()
        .find(|(name, _)| name == &config.supervisor_name)
        .expect("supervisor config must be present");
    let effort_idx = sup_config
        .args
        .iter()
        .position(|a| a == "--effort")
        .expect("supervisor PTY args must contain --effort");
    let effort_val = sup_config
        .args
        .get(effort_idx + 1)
        .expect("--effort must be followed by a value in supervisor PTY args");
    assert_eq!(
        effort_val, "low",
        "MuxConfig::supervisor_effort must reach supervisor PTY --effort arg"
    );
}

#[test]
fn factory_pane_configs_worker_effort_reaches_pty_args() {
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        supervisor_effort: Some("low".to_string()),
        worker_effort: Some("high".to_string()),
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Claude,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    let (_, worker_config) = configs
        .iter()
        .find(|(name, _)| name == "worker-1")
        .expect("worker-1 config must be present");
    let effort_idx = worker_config
        .args
        .iter()
        .position(|a| a == "--effort")
        .expect("worker PTY args must contain --effort");
    let effort_val = worker_config
        .args
        .get(effort_idx + 1)
        .expect("--effort must be followed by a value in worker PTY args");
    assert_eq!(
        effort_val, "high",
        "MuxConfig::worker_effort must reach worker PTY --effort arg"
    );
    // supervisor must be last in the returned vec (workers-first ordering)
    assert_eq!(
        configs.last().unwrap().0,
        config.supervisor_name,
        "supervisor must be the last entry in factory_pane_configs output"
    );
}

#[test]
fn factory_pane_configs_none_effort_uses_role_defaults() {
    // When MuxConfig effort fields are None, PtyConfig::claude defaults fire:
    // supervisor → "xhigh", worker → "high"
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        supervisor_effort: None,
        worker_effort: None,
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Claude,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    let (_, sup_config) = configs
        .iter()
        .find(|(name, _)| name == &config.supervisor_name)
        .expect("supervisor config must be present");
    let sup_effort_idx = sup_config
        .args
        .iter()
        .position(|a| a == "--effort")
        .expect("supervisor PTY args must contain --effort");
    let sup_effort_val = sup_config
        .args
        .get(sup_effort_idx + 1)
        .expect("--effort must be followed by a value in supervisor PTY args");
    assert_eq!(
        sup_effort_val, "xhigh",
        "supervisor with no effort override must default to xhigh"
    );

    let (_, worker_config) = configs
        .iter()
        .find(|(name, _)| name == "worker-1")
        .expect("worker-1 config must be present");
    let worker_effort_idx = worker_config
        .args
        .iter()
        .position(|a| a == "--effort")
        .expect("worker PTY args must contain --effort");
    let worker_effort_val = worker_config
        .args
        .get(worker_effort_idx + 1)
        .expect("--effort must be followed by a value in worker PTY args");
    assert_eq!(
        worker_effort_val, "high",
        "worker with no effort override must default to high"
    );
}

// ── end cas-d571 ──────────────────────────────────────────────────────────────

// ── cas-3fed: per-worker spec storage + factory wiring ────────────────────────
// Tests the MuxConfig.resolved_worker_specs → factory_pane_configs per-worker
// CLI selection path, and the Mux::add_worker explicit spec override path.

#[test]
fn factory_pane_configs_uses_per_worker_specs() {
    // worker-1 → Codex, worker-2 → Claude, but MuxConfig.worker_cli is Claude.
    // resolved_worker_specs must override the singular default per worker.
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 2,
        worker_names: vec!["worker-1".to_string(), "worker-2".to_string()],
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Claude,
        resolved_worker_specs: vec![
            WorkerSpec {
                name: Some("worker-1".to_string()),
                cli: crate::harness::SupervisorCli::Codex,
                model: None,
                effort: None,
            },
            WorkerSpec {
                name: Some("worker-2".to_string()),
                cli: crate::harness::SupervisorCli::Claude,
                model: None,
                effort: None,
            },
        ],
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    let (_, w1) = configs
        .iter()
        .find(|(n, _)| n == "worker-1")
        .expect("worker-1 must be present");
    let (_, w2) = configs
        .iter()
        .find(|(n, _)| n == "worker-2")
        .expect("worker-2 must be present");

    assert_eq!(
        w1.command, "codex",
        "worker-1 with Codex spec must use codex binary"
    );
    assert_eq!(
        w2.command, "claude",
        "worker-2 with Claude spec must use claude binary"
    );
}

#[test]
fn factory_pane_configs_falls_back_to_singular_when_specs_empty() {
    // resolved_worker_specs is empty → all workers use worker_cli = Codex.
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 2,
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Codex,
        resolved_worker_specs: vec![],
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    for (name, pty_config) in &configs {
        if name == &config.supervisor_name {
            assert_eq!(
                pty_config.command, "claude",
                "supervisor must use claude binary"
            );
        } else {
            assert_eq!(
                pty_config.command, "codex",
                "worker {name} with empty resolved_worker_specs must fall back to worker_cli=Codex"
            );
        }
    }
}

#[test]
fn add_worker_uses_explicit_spec() {
    // Mux default is Claude (builtin_default), but build_add_worker_config with
    // an explicit Codex spec must produce a codex PtyConfig.
    let mux = Mux::new(24, 80);

    let codex_spec = WorkerSpec {
        name: Some("dynamic-worker".to_string()),
        cli: crate::harness::SupervisorCli::Codex,
        model: None,
        effort: Some(Effort::High),
    };

    let pty_config = mux.build_add_worker_config(
        "dynamic-worker",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        Some(codex_spec),
    );

    assert_eq!(
        pty_config.command, "codex",
        "explicit Codex spec must override Claude default in dynamic add_worker path"
    );

    // Without explicit spec, the default (Claude) must be used.
    let claude_config = mux.build_add_worker_config(
        "another-worker",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        None,
    );
    assert_eq!(
        claude_config.command, "claude",
        "no explicit spec must fall back to Mux default (Claude)"
    );
}

// ── end cas-3fed ──────────────────────────────────────────────────────────────

#[test]
fn test_mux_new() {
    let mux = Mux::new(24, 80);
    assert_eq!(mux.size(), (24, 80));
    assert!(mux.focused().is_none());
}

#[test]
fn test_mux_add_pane() {
    let mut mux = Mux::new(24, 80);
    let pane = Pane::director("test", 24, 80).unwrap();
    mux.add_pane(pane);

    assert!(mux.get("test").is_some());
    assert_eq!(mux.focused_id(), Some("test"));
}

#[test]
fn test_mux_focus_navigation() {
    let mut mux = Mux::new(24, 80);
    mux.add_pane(Pane::director("pane1", 24, 40).unwrap());
    mux.add_pane(Pane::director("pane2", 24, 40).unwrap());

    assert_eq!(mux.focused_id(), Some("pane1"));

    mux.focus_next();
    assert_eq!(mux.focused_id(), Some("pane2"));

    mux.focus_next();
    assert_eq!(mux.focused_id(), Some("pane1")); // Wraps around

    mux.focus_prev();
    assert_eq!(mux.focused_id(), Some("pane2"));
}

#[test]
fn test_pane_count() {
    let mut mux = Mux::new(24, 80);
    assert_eq!(mux.pane_count(), 0);

    mux.add_pane(Pane::director("pane1", 24, 40).unwrap());
    assert_eq!(mux.pane_count(), 1);

    mux.add_pane(Pane::director("pane2", 24, 40).unwrap());
    assert_eq!(mux.pane_count(), 2);

    mux.remove_pane("pane1");
    assert_eq!(mux.pane_count(), 1);
}

#[test]
fn test_remove_pane_focus_transfer() {
    let mut mux = Mux::new(24, 80);
    mux.add_pane(Pane::director("pane1", 24, 40).unwrap());
    mux.add_pane(Pane::director("pane2", 24, 40).unwrap());

    // Focus is on pane1 (first added)
    assert_eq!(mux.focused_id(), Some("pane1"));

    // Remove focused pane, focus should transfer to next
    mux.remove_pane("pane1");
    assert_eq!(mux.focused_id(), Some("pane2"));
    assert_eq!(mux.pane_count(), 1);
}
