use crate::support::*;
use cas::mcp::tools::*;
use rmcp::handler::server::wrapper::Parameters;

#[tokio::test]
async fn test_context() {
    let (_temp, service) = setup_cas();

    // Create some content first
    let req = RememberRequest {
        scope: "project".to_string(),
        content: "Context test memory".to_string(),
        entry_type: "learning".to_string(),
        tags: None,
        title: None,
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        personal: None,
    };

    service
        .cas_remember(Parameters(req))
        .await
        .expect("remember should succeed");

    // Get context
    let ctx_req = LimitRequest {
        scope: "all".to_string(),
        limit: Some(5),
        sort: None,
        sort_order: None,
        team_id: None,
    };
    let result = service
        .cas_context(Parameters(ctx_req))
        .await
        .expect("context should succeed");

    let text = extract_text(result);
    // Context should return something (may be empty if no helpful memories)
    assert!(!text.is_empty() || text.contains("No context"));
}

#[tokio::test]
async fn test_stats() {
    let (_temp, service) = setup_cas();

    let result = service.cas_stats().await.expect("stats should succeed");

    let text = extract_text(result);
    assert!(text.contains("CAS Statistics") || text.contains("entries") || text.contains("0"));
}

#[tokio::test]
async fn test_doctor() {
    let (_temp, service) = setup_cas();

    let result = service.cas_doctor().await.expect("doctor should succeed");

    let text = extract_text(result);
    assert!(text.contains("CAS Diagnostics") || text.contains("OK") || text.contains("healthy"));
}

#[tokio::test]
async fn test_observe() {
    let (_temp, service) = setup_cas();

    let req = ObserveRequest {
        scope: "project".to_string(),
        content: "Test observation".to_string(),
        observation_type: "decision".to_string(),
        tags: Some("test".to_string()),
        source_tool: Some("test".to_string()),
    };

    let result = service
        .cas_observe(Parameters(req))
        .await
        .expect("observe should succeed");

    let text = extract_text(result);
    let text_lower = text.to_lowercase();
    assert!(
        text_lower.contains("observation")
            || text_lower.contains("recorded")
            || text.contains("ID")
    );
}

#[tokio::test]
async fn test_maintenance_status() {
    let (_temp, service) = setup_cas();

    let result = service
        .cas_maintenance_status()
        .await
        .expect("maintenance_status should succeed");

    let text = extract_text(result);
    // Without daemon, should indicate no daemon
    assert!(
        text.contains("Daemon not running")
            || text.contains("status")
            || text.contains("Maintenance")
    );
}
