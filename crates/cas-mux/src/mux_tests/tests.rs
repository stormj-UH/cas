use crate::mux::*;

// ── cas-d571: effort config flows through Mux::factory() to PTY args ─────────
// Tests the full MuxConfig → Mux::factory_pane_configs() → PtyConfig.args chain.
// Uses `factory_pane_configs` (config-only, no spawn) so tests run without a
// real `claude` or `codex` binary present.

#[test]
fn factory_pane_configs_supervisor_effort_reaches_pty_args() {
    use std::path::PathBuf;
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
    assert_eq!(
        sup_config.args[effort_idx + 1], "low",
        "MuxConfig::supervisor_effort must reach supervisor PTY --effort arg"
    );
}

#[test]
fn factory_pane_configs_worker_effort_reaches_pty_args() {
    use std::path::PathBuf;
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
        .find(|(name, _)| name != &config.supervisor_name)
        .expect("at least one worker config must be present");
    let effort_idx = worker_config
        .args
        .iter()
        .position(|a| a == "--effort")
        .expect("worker PTY args must contain --effort");
    assert_eq!(
        worker_config.args[effort_idx + 1], "high",
        "MuxConfig::worker_effort must reach worker PTY --effort arg"
    );
}

#[test]
fn factory_pane_configs_none_effort_uses_role_defaults() {
    use std::path::PathBuf;
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
    assert_eq!(
        sup_config.args[sup_effort_idx + 1], "xhigh",
        "supervisor with no effort override must default to xhigh"
    );

    let (_, worker_config) = configs
        .iter()
        .find(|(name, _)| name != &config.supervisor_name)
        .expect("worker config must be present");
    let worker_effort_idx = worker_config
        .args
        .iter()
        .position(|a| a == "--effort")
        .expect("worker PTY args must contain --effort");
    assert_eq!(
        worker_config.args[worker_effort_idx + 1], "high",
        "worker with no effort override must default to high"
    );
}

// ── end cas-d571 ──────────────────────────────────────────────────────────────

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
