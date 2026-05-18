use crate::support::*;
use cas::mcp::tools::*;
use rmcp::handler::server::wrapper::Parameters;
use rusqlite::Connection;

#[tokio::test]
async fn test_task_show() {
    let (_temp, service) = setup_cas();

    // Create task
    let req = TaskCreateRequest {
        title: "Show task".to_string(),
        description: Some("Detailed description".to_string()),
        priority: 1,
        task_type: "bug".to_string(),
        labels: Some("urgent".to_string()),
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Show task
    let show_req = TaskShowRequest {
        id: id.to_string(),
        with_deps: true,
    };
    let result = service
        .cas_task_show(Parameters(show_req))
        .await
        .expect("task_show should succeed");

    let text = extract_text(result);
    assert!(text.contains("Show task"));
    assert!(text.contains("Detailed description") || text.contains("bug"));
}

// =============================================================================
// cas-7fc1: execution_note field end-to-end coverage
// =============================================================================

fn basic_create(title: &str, execution_note: Option<String>) -> TaskCreateRequest {
    TaskCreateRequest {
        title: title.to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note,
        epic: None,
    }
}

/// Happy path: create a task with an accepted execution_note value and
/// verify it is persisted + surfaced by `action=show`.
#[tokio::test]
async fn test_execution_note_create_and_show_happy_path() {
    let (_temp, service) = setup_cas();

    let created = service
        .cas_task_create(Parameters(basic_create(
            "Task with execution note",
            Some("test-first".to_string()),
        )))
        .await
        .expect("create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: id.clone(),
            with_deps: false,
        }))
        .await
        .expect("show should succeed");
    let text = extract_text(shown);
    assert!(
        text.contains("Execution Note: test-first"),
        "show output must include execution_note line when set, got: {text}"
    );
}

/// Null path: create a task WITHOUT execution_note and verify `action=show`
/// omits the line entirely.
#[tokio::test]
async fn test_execution_note_null_omitted_from_show() {
    let (_temp, service) = setup_cas();

    let created = service
        .cas_task_create(Parameters(basic_create("Task without execution note", None)))
        .await
        .expect("create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id,
            with_deps: false,
        }))
        .await
        .expect("show should succeed");
    let text = extract_text(shown);
    assert!(
        !text.contains("Execution Note"),
        "show output must omit execution_note line when unset, got: {text}"
    );
}

/// Invalid enum: reject unknown values at the MCP tool layer with a clear
/// error that lists the allowed values.
#[tokio::test]
async fn test_execution_note_invalid_enum_rejected() {
    let (_temp, service) = setup_cas();

    let err = service
        .cas_task_create(Parameters(basic_create(
            "Task with garbage execution note",
            Some("garbage".to_string()),
        )))
        .await
        .expect_err("invalid enum must be rejected at MCP layer");
    let msg = err.message.to_string();
    assert!(
        msg.contains("Invalid execution_note"),
        "error must name the bad field, got: {msg}"
    );
    assert!(
        msg.contains("test-first")
            && msg.contains("characterization-first")
            && msg.contains("additive-only"),
        "error must list allowed values, got: {msg}"
    );
}

/// Update path: create without execution_note, then set it via update.
#[tokio::test]
async fn test_execution_note_update_sets_value() {
    let (_temp, service) = setup_cas();

    let created = service
        .cas_task_create(Parameters(basic_create("Update target", None)))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    let updated = service
        .cas_task_update(Parameters(TaskUpdateRequest {
            id: id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: Some("additive-only".to_string()),
            external_ref: None,
            assignee: None,
            status: None,
            epic: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("update");
    assert!(
        extract_text(updated).contains("execution_note"),
        "update response must list changed field"
    );

    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id,
            with_deps: false,
        }))
        .await
        .expect("show");
    assert!(extract_text(shown).contains("Execution Note: additive-only"));
}

/// Unset path: passing an empty string on update clears the field back to None.
#[tokio::test]
async fn test_execution_note_update_empty_string_clears() {
    let (_temp, service) = setup_cas();

    let created = service
        .cas_task_create(Parameters(basic_create(
            "Clear target",
            Some("characterization-first".to_string()),
        )))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    service
        .cas_task_update(Parameters(TaskUpdateRequest {
            id: id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: Some(String::new()),
            external_ref: None,
            assignee: None,
            status: None,
            epic: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("update clear");

    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id,
            with_deps: false,
        }))
        .await
        .expect("show");
    assert!(
        !extract_text(shown).contains("Execution Note"),
        "empty string must clear the field"
    );
}

#[tokio::test]
async fn test_task_update() {
    let (_temp, service) = setup_cas();

    // Create task
    let req = TaskCreateRequest {
        title: "Update task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Update task
    let update_req = TaskUpdateRequest {
        id: id.to_string(),
        title: Some("Updated title".to_string()),
        notes: Some("Added note".to_string()),
        priority: Some(1),
        labels: None,
        description: None,
        design: None,
        acceptance_criteria: None,
        demo_statement: None,
        execution_note: None,
        external_ref: None,
        assignee: None,
        status: None,
        epic: None,
        epic_verification_owner: None,
    };

    let result = service
        .cas_task_update(Parameters(update_req))
        .await
        .expect("task_update should succeed");

    let text = extract_text(result);
    assert!(text.contains("Updated") || text.contains("updated"));
}

#[tokio::test]
async fn test_task_update_design_and_acceptance_criteria() {
    let (_temp, service) = setup_cas();

    // Create task
    let req = TaskCreateRequest {
        title: "Spec task".to_string(),
        description: None,
        priority: 2,
        task_type: "epic".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Update design and acceptance_criteria
    let update_req = TaskUpdateRequest {
        id: id.to_string(),
        title: None,
        notes: None,
        priority: None,
        labels: None,
        description: None,
        design: Some("## Technical Spec\nThis is the design.".to_string()),
        acceptance_criteria: Some("- [ ] Criterion 1\n- [ ] Criterion 2".to_string()),
        demo_statement: None,
        execution_note: None,
        external_ref: None,
        assignee: None,
        status: None,
        epic: None,
        epic_verification_owner: None,
    };

    let result = service
        .cas_task_update(Parameters(update_req))
        .await
        .expect("task_update should succeed");

    let text = extract_text(result);
    assert!(
        text.contains("Updated") || text.contains("updated") || text.contains("design"),
        "Update should succeed: {text}"
    );

    // Verify via show
    let show_req = TaskShowRequest {
        id: id.to_string(),
        with_deps: false,
    };

    let result = service
        .cas_task_show(Parameters(show_req))
        .await
        .expect("task_show should succeed");

    let text = extract_text(result);
    assert!(
        text.contains("Technical Spec"),
        "Show should include design: {text}"
    );
    assert!(
        text.contains("Criterion 1"),
        "Show should include acceptance_criteria: {text}"
    );
}

#[tokio::test]
async fn test_task_notes() {
    let (_temp, service) = setup_cas();

    // Create task
    let req = TaskCreateRequest {
        title: "Notes task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Add notes
    let notes_req = TaskNotesRequest {
        id: id.to_string(),
        note: "Making progress on implementation".to_string(),
        note_type: "progress".to_string(),
    };

    let result = service
        .cas_task_notes(Parameters(notes_req))
        .await
        .expect("task_notes should succeed");

    let text = extract_text(result);
    assert!(text.contains("Added note") || text.contains("note"));
}

#[tokio::test]
async fn test_task_list() {
    let (_temp, service) = setup_cas();

    // Create tasks
    for i in 0..3 {
        let req = TaskCreateRequest {
            title: format!("List task {i}"),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        };
        service
            .cas_task_create(Parameters(req))
            .await
            .expect("task_create should succeed");
    }

    // List tasks
    let list_req = TaskListRequest {
        scope: "all".to_string(),
        limit: Some(10),
        status: None,
        task_type: None,
        label: None,
        assignee: None,
        epic: None,
        sort: None,
        sort_order: None,
    };
    let result = service
        .cas_task_list(Parameters(list_req))
        .await
        .expect("task_list should succeed");

    let text = extract_text(result);
    assert!(text.contains("List task") || text.contains("Tasks"));
}

#[tokio::test]
async fn test_task_ready() {
    let (_temp, service) = setup_cas();

    // Create ready tasks
    for i in 0..3 {
        let req = TaskCreateRequest {
            title: format!("Ready task {i}"),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        };
        service
            .cas_task_create(Parameters(req))
            .await
            .expect("task_create should succeed");
    }

    // List ready tasks
    let ready_req = TaskReadyBlockedRequest {
        scope: "all".to_string(),
        limit: Some(10),
        sort: None,
        sort_order: None,
        epic: None,
    };
    let result = service
        .cas_task_ready(Parameters(ready_req))
        .await
        .expect("task_ready should succeed");

    let text = extract_text(result);
    assert!(text.contains("Ready task") || text.contains("ready") || text.contains("Tasks"));
}

#[tokio::test]
async fn test_task_delete() {
    let (_temp, service) = setup_cas();

    // Create task
    let req = TaskCreateRequest {
        title: "Delete task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Delete task
    let delete_req = IdRequest { id: id.to_string() };
    let result = service
        .cas_task_delete(Parameters(delete_req))
        .await
        .expect("task_delete should succeed");

    let text = extract_text(result);
    assert!(text.contains("Deleted"));
}

#[tokio::test]
async fn test_task_dependencies() {
    let (_temp, service) = setup_cas();

    // Create two tasks
    let req1 = TaskCreateRequest {
        title: "Blocker task".to_string(),
        description: None,
        priority: 1,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result1 = service
        .cas_task_create(Parameters(req1))
        .await
        .expect("task_create should succeed");

    let text1 = extract_text(result1);
    let blocker_id = extract_task_id(&text1).expect("should have task ID");

    let req2 = TaskCreateRequest {
        title: "Blocked task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result2 = service
        .cas_task_create(Parameters(req2))
        .await
        .expect("task_create should succeed");

    let text2 = extract_text(result2);
    let blocked_id = extract_task_id(&text2).expect("should have task ID");

    // Add dependency
    let dep_req = DependencyRequest {
        from_id: blocked_id.to_string(),
        to_id: blocker_id.to_string(),
        dep_type: "blocks".to_string(),
    };

    let result = service
        .cas_task_dep_add(Parameters(dep_req))
        .await
        .expect("task_dep_add should succeed");

    let text = extract_text(result);
    assert!(text.contains("dependency") || text.contains("Added") || text.contains("blocks"));

    // List dependencies
    let dep_list_req = IdRequest {
        id: blocked_id.to_string(),
    };
    let result = service
        .cas_task_dep_list(Parameters(dep_list_req))
        .await
        .expect("task_dep_list should succeed");

    let text = extract_text(result);
    assert!(text.contains(blocker_id) || text.contains("blocks"));
}

#[tokio::test]
async fn test_task_show_dependency_direction_labels() {
    let (_temp, service) = setup_cas();

    let blocker = service
        .cas_task_create(Parameters(TaskCreateRequest {
            title: "Direction blocker".to_string(),
            description: None,
            priority: 1,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("blocker create should succeed");
    let blocker_id = extract_task_id(&extract_text(blocker))
        .expect("blocker id")
        .to_string();

    let blocked = service
        .cas_task_create(Parameters(TaskCreateRequest {
            title: "Direction blocked".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: Some(blocker_id.clone()),
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("blocked create should succeed");
    let blocked_id = extract_task_id(&extract_text(blocked))
        .expect("blocked id")
        .to_string();

    let show = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: blocked_id.clone(),
            with_deps: true,
        }))
        .await
        .expect("task_show should succeed");
    let text = extract_text(show);
    assert!(
        text.contains("BlockedBy:") && text.contains(&blocker_id),
        "Blocked task should display inbound blockers clearly: {text}"
    );

    let blocker_show = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: blocker_id.clone(),
            with_deps: true,
        }))
        .await
        .expect("task_show should succeed");
    let blocker_text = extract_text(blocker_show);
    assert!(
        blocker_text.contains("Blocks:") && blocker_text.contains(&blocked_id),
        "Blocker task should show downstream dependent tasks: {blocker_text}"
    );
}

#[tokio::test]
async fn test_close_auto_unblocks_blocked_dependents() {
    let (_temp, service) = setup_cas();

    let blocker = service
        .cas_task_create(Parameters(TaskCreateRequest {
            title: "Auto unblock blocker".to_string(),
            description: None,
            priority: 1,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("blocker create should succeed");
    let blocker_id = extract_task_id(&extract_text(blocker))
        .expect("blocker id")
        .to_string();

    let blocked = service
        .cas_task_create(Parameters(TaskCreateRequest {
            title: "Auto unblock dependent".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: Some(blocker_id.clone()),
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("blocked task create should succeed");
    let blocked_id = extract_task_id(&extract_text(blocked))
        .expect("blocked id")
        .to_string();

    let _ = service
        .cas_task_update(Parameters(TaskUpdateRequest {
            id: blocked_id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: None,
            status: Some("blocked".to_string()),
            epic: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("blocked task update should succeed");

    let _ = service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: blocker_id.clone(),
            status: "approved".to_string(),
            summary: "approved for close".to_string(),
            confidence: Some(0.9),
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
        }))
        .await
        .expect("verification add should succeed");

    let close = service
        .cas_task_close(Parameters(TaskCloseRequest {
            id: blocker_id,
            reason: Some("done".to_string()),
            bypass_code_review: None,
code_review_findings: None,
        }))
        .await
        .expect("task close should succeed");
    let close_text = extract_text(close);
    assert!(
        close_text.contains("Auto-unblocked"),
        "Close output should mention auto-unblocked tasks: {close_text}"
    );

    let show = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: blocked_id,
            with_deps: false,
        }))
        .await
        .expect("task_show should succeed");
    let text = extract_text(show);
    assert!(
        text.contains("Status: Open"),
        "Blocked dependent should auto-transition to Open: {text}"
    );
}

#[tokio::test]
async fn test_task_update_invalid_epic_keeps_original_parent_dependency() {
    let (_temp, service) = setup_cas();

    let epic_1 = service
        .cas_task_create(Parameters(TaskCreateRequest {
            title: "Epic 1".to_string(),
            description: None,
            priority: 1,
            task_type: "epic".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("epic 1 create should succeed");
    let epic_1_id = extract_task_id(&extract_text(epic_1))
        .expect("epic 1 id")
        .to_string();

    let subtask = service
        .cas_task_create(Parameters(TaskCreateRequest {
            title: "Child task".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: Some(epic_1_id.clone()),
        }))
        .await
        .expect("subtask create should succeed");
    let subtask_id = extract_task_id(&extract_text(subtask))
        .expect("subtask id")
        .to_string();

    let update_result = service
        .cas_task_update(Parameters(TaskUpdateRequest {
            id: subtask_id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: None,
            status: None,
            epic: Some("cas-does-not-exist".to_string()),
            epic_verification_owner: None,
        }))
        .await;
    assert!(
        update_result.is_err(),
        "Invalid epic reassignment should fail"
    );

    let list_result = service
        .cas_task_list(Parameters(TaskListRequest {
            scope: "all".to_string(),
            limit: Some(20),
            status: None,
            task_type: None,
            label: None,
            assignee: None,
            epic: Some(epic_1_id),
            sort: None,
            sort_order: None,
        }))
        .await
        .expect("task list by epic should succeed");
    let text = extract_text(list_result);
    assert!(
        text.contains(&subtask_id),
        "Original ParentChild dependency should be preserved on failed reassignment: {text}"
    );
}

#[tokio::test]
async fn test_task_update_surfaces_epic_dependency_delete_failure() {
    let (temp, service) = setup_cas();

    let epic = service
        .cas_task_create(Parameters(TaskCreateRequest {
            title: "Epic".to_string(),
            description: None,
            priority: 1,
            task_type: "epic".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("epic create should succeed");
    let epic_id = extract_task_id(&extract_text(epic))
        .expect("epic id")
        .to_string();

    let subtask = service
        .cas_task_create(Parameters(TaskCreateRequest {
            title: "Subtask".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: Some(epic_id),
        }))
        .await
        .expect("subtask create should succeed");
    let subtask_id = extract_task_id(&extract_text(subtask))
        .expect("subtask id")
        .to_string();

    let db_path = temp.path().join(".cas").join("cas.db");
    let conn = Connection::open(&db_path).expect("open sqlite db");
    conn.execute(
        "CREATE TRIGGER fail_dependency_delete
         BEFORE DELETE ON dependencies
         BEGIN
             SELECT RAISE(FAIL, 'forced dependency delete failure');
         END;",
        [],
    )
    .expect("create delete failure trigger");

    let update_result = service
        .cas_task_update(Parameters(TaskUpdateRequest {
            id: subtask_id,
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: None,
            status: None,
            epic: Some(String::new()),
            epic_verification_owner: None,
        }))
        .await;
    assert!(
        update_result.is_err(),
        "Dependency delete failure should be returned to caller"
    );
}

#[tokio::test]
async fn test_subtask_start_auto_starts_epic() {
    let (_temp, service) = setup_cas();

    // Create an epic
    let epic_req = TaskCreateRequest {
        title: "Test Epic".to_string(),
        description: Some("An epic with subtasks".to_string()),
        priority: 1,
        task_type: "epic".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(epic_req))
        .await
        .expect("epic create should succeed");

    let text = extract_text(result);
    let epic_id = extract_task_id(&text).expect("should have epic ID");

    // Verify epic is NOT in progress
    let show_req = TaskShowRequest {
        id: epic_id.to_string(),
        with_deps: false,
    };
    let result = service
        .cas_task_show(Parameters(show_req))
        .await
        .expect("task show should succeed");
    let text = extract_text(result);
    assert!(
        text.contains("open") || text.contains("Open"),
        "Epic should be open initially: {text}"
    );

    // Create a subtask linked to the epic
    let subtask_req = TaskCreateRequest {
        title: "Subtask 1".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: Some(epic_id.to_string()),
    };

    let result = service
        .cas_task_create(Parameters(subtask_req))
        .await
        .expect("subtask create should succeed");

    let text = extract_text(result);
    let subtask_id = extract_task_id(&text).expect("should have subtask ID");

    // Start the subtask - this should auto-start the epic
    let start_req = IdRequest {
        id: subtask_id.to_string(),
    };
    let result = service
        .cas_task_start(Parameters(start_req))
        .await
        .expect("subtask start should succeed");

    let text = extract_text(result);
    assert!(
        text.contains("EPIC OWNERSHIP"),
        "Should show epic ownership message: {text}"
    );
    assert!(text.contains(epic_id), "Should reference epic ID: {text}");
    assert!(
        text.contains("auto-started"),
        "Should indicate epic was auto-started: {text}"
    );
    // Workflow guidance should be included when starting a task
    assert!(
        text.contains("Workflow Guidance"),
        "Task start should include workflow guidance: {text}"
    );
    assert!(
        text.contains("mcp__cas__search"),
        "Workflow guidance should mention CAS search: {text}"
    );

    // Verify the epic is now in progress
    let show_req2 = TaskShowRequest {
        id: epic_id.to_string(),
        with_deps: false,
    };
    let result = service
        .cas_task_show(Parameters(show_req2))
        .await
        .expect("task show should succeed");
    let text = extract_text(result);
    assert!(
        text.contains("in_progress") || text.contains("InProgress") || text.contains("In Progress"),
        "Epic should be in progress after subtask start: {text}"
    );
}

// ============================================================================
// cas-5572 (EPIC cas-9508): Spawn-time `action=mine` race regression
//
// Reproduces the factory-session friction described in
// docs/requests/BUG-factory-session-observations-2026-04-22.md §1: after
// `coordination spawn_workers` + `task update assignee=<worker-name>`, a
// freshly-spawned worker's first `action=mine` call was returning "no open
// tasks" even when `task show` on the supervisor side immediately confirmed
// the assignment.
//
// Root cause: `cas_tasks_mine` previously matched only `assignee == agent_id
// || agent_name` where `agent_name` was read from the agent-store row. When
// the worker's agent row has not yet been populated with the final friendly
// name — or the lookup transiently falls back to `agent_id` — the filter
// missed name-based assignments. The fix widens the match to also consider
// `CAS_AGENT_NAME` / `CAS_SESSION_ID` env vars and compares case-insensitively
// on trimmed values.
// ============================================================================

#[tokio::test]
async fn test_task_mine_matches_env_worker_name_during_spawn_race() {
    let (_temp, service) = setup_cas();

    // Simulate the spawn-race condition: the agent-store row still shows
    // the default "test-agent" name, but the supervisor has already assigned
    // the task to the worker's *friendly* name (e.g. "warm-gopher-85"). In
    // the real factory flow the friendly name arrives via CAS_AGENT_NAME in
    // the worker process's env.
    let worker_name = "warm-gopher-85";

    // Acquire the env lock since we're mutating CAS_AGENT_NAME.
    let _env_guard = env_test_lock();
    let prev_name = std::env::var("CAS_AGENT_NAME").ok();
    // SAFETY: env lock is held for the duration of this test body.
    unsafe {
        std::env::set_var("CAS_AGENT_NAME", worker_name);
    }

    // Create a task, then update its assignee to the worker's friendly name —
    // exactly what a supervisor does via `task update assignee=<worker-name>`.
    let create_req = TaskCreateRequest {
        title: "Spawn-race assignment".to_string(),
        description: None,
        priority: 1,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let created = service
        .cas_task_create(Parameters(create_req))
        .await
        .expect("task_create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();

    let update_req = TaskUpdateRequest {
        id: id.clone(),
        title: None,
        notes: None,
        priority: None,
        labels: None,
        description: None,
        design: None,
        acceptance_criteria: None,
        demo_statement: None,
        execution_note: None,
        external_ref: None,
        assignee: Some(worker_name.to_string()),
        status: None,
        epic: None,
        epic_verification_owner: None,
    };
    service
        .cas_task_update(Parameters(update_req))
        .await
        .expect("task_update should succeed");

    // The worker's first `action=mine` poll — the assignee on the task row
    // is the friendly `worker_name`, but the agent_store row still carries
    // the default "test-agent" name from setup_cas(). Before the fix this
    // returned "No open tasks"; after the fix, CAS_AGENT_NAME bridges the
    // gap and the task surfaces.
    let mine_req = LimitRequest {
        limit: Some(20),
        scope: "all".to_string(),
        sort: None,
        sort_order: None,
        team_id: None,
    };
    let result = service
        .cas_tasks_mine(Parameters(mine_req))
        .await
        .expect("tasks_mine should succeed");
    let text = extract_text(result);

    // Restore env before any assertion to avoid poisoning sibling tests on
    // panic. SAFETY: still holding env lock.
    unsafe {
        match prev_name {
            Some(v) => std::env::set_var("CAS_AGENT_NAME", v),
            None => std::env::remove_var("CAS_AGENT_NAME"),
        }
    }

    assert!(
        text.contains(&id),
        "cas_tasks_mine must surface tasks assigned by friendly worker-name \
         (via CAS_AGENT_NAME env) even when the agent-store row still holds \
         the default name. Got: {text}"
    );
    assert!(
        !text.starts_with("No open tasks"),
        "cas_tasks_mine should not report empty during spawn-race window. Got: {text}"
    );
}

// ============================================================================
// cas-1a7c (EPIC cas-9508): task lease + status divergence recovery.
//
// Acceptance criteria:
//   - `action=release` on a lease-less InProgress task clears status to open
//     with an audit trail.
//   - `action=reset` verb exists and is tested for dead-session recovery.
//   - `action=show` called immediately after `action=update` reflects the
//     updated status.
// ============================================================================

#[tokio::test]
async fn test_release_autorecovers_lease_less_in_progress_task() {
    let (_temp, service) = setup_cas();

    // Seed: create task and move it to InProgress without a live lease
    // (simulating a dead-session orphan where status diverged from lease).
    let created = service
        .cas_task_create(Parameters(TaskCreateRequest {
            title: "Orphaned in-progress".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: Some("dead-worker".to_string()),
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    service
        .cas_task_update(Parameters(TaskUpdateRequest {
            id: id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: None,
            status: Some("in_progress".to_string()),
            epic: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("status update");

    // Call release — no active lease exists for this agent, and the task is
    // InProgress. The handler must auto-recover instead of surfacing the raw
    // "No active lease found" error.
    let released = service
        .cas_task_release(Parameters(cas::mcp::tools::TaskReleaseRequest {
            task_id: id.clone(),
        }))
        .await
        .expect("release auto-recovery must succeed for lease-less InProgress");
    let text = extract_text(released);
    assert!(
        text.contains("auto-recovered") || text.contains("Released"),
        "release output should acknowledge auto-recovery: {text}"
    );

    // Show must reflect Open status immediately after release.
    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: id.clone(),
            with_deps: false,
        }))
        .await
        .expect("show");
    let text = extract_text(shown);
    assert!(
        text.contains("Open") || text.contains("open"),
        "task must be Open after release auto-recovery: {text}"
    );
    assert!(
        text.contains("auto-recovered") || text.contains("assumed orphaned"),
        "task notes must contain audit trail: {text}"
    );
}

#[tokio::test]
async fn test_release_still_errors_when_no_lease_and_task_already_open() {
    let (_temp, service) = setup_cas();

    // Baseline: no lease, status=Open. Release should NOT silently succeed —
    // there's nothing to recover, surface the underlying error.
    let created = service
        .cas_task_create(Parameters(TaskCreateRequest {
            title: "Plain open task".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    let res = service
        .cas_task_release(Parameters(cas::mcp::tools::TaskReleaseRequest {
            task_id: id.clone(),
        }))
        .await;
    assert!(
        res.is_err(),
        "release on a plain Open task without a lease should error"
    );
}

#[tokio::test]
async fn test_reset_clears_lease_assignee_and_forces_open() {
    let (_temp, service) = setup_cas();

    let created = service
        .cas_task_create(Parameters(TaskCreateRequest {
            title: "Needs reset".to_string(),
            description: None,
            priority: 1,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: Some("dead-worker".to_string()),
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    service
        .cas_task_update(Parameters(TaskUpdateRequest {
            id: id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: None,
            status: Some("in_progress".to_string()),
            epic: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("status update");

    let res = service
        .cas_task_reset(Parameters(cas::mcp::tools::TaskReleaseRequest {
            task_id: id.clone(),
        }))
        .await
        .expect("reset must succeed");
    let text = extract_text(res);
    assert!(
        text.contains("Reset task"),
        "reset output must confirm: {text}"
    );

    // Show must reflect the reset: Open, no assignee, audit note present.
    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: id.clone(),
            with_deps: false,
        }))
        .await
        .expect("show");
    let text = extract_text(shown);
    assert!(
        text.contains("Open") || text.contains("open"),
        "status must be Open after reset: {text}"
    );
    assert!(
        text.contains("reset:") || text.contains("dead-session"),
        "reset audit note must be present: {text}"
    );
}

#[tokio::test]
async fn test_reset_refuses_closed_task() {
    let (_temp, service) = setup_cas();

    let created = service
        .cas_task_create(Parameters(TaskCreateRequest {
            title: "Already closed".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    service
        .cas_task_update(Parameters(TaskUpdateRequest {
            id: id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: None,
            status: Some("closed".to_string()),
            epic: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("close via update");

    let err = service
        .cas_task_reset(Parameters(cas::mcp::tools::TaskReleaseRequest {
            task_id: id.clone(),
        }))
        .await;
    assert!(
        err.is_err(),
        "reset must refuse to operate on closed tasks — use reopen instead"
    );
}

/// cas-1a7c AC3: `action=show` immediately after `action=update` must reflect
/// the updated status. Asserts there's no read-after-write snapshot lag in
/// the MCP task store path.
#[tokio::test]
async fn test_show_after_update_reflects_new_status_without_lag() {
    let (_temp, service) = setup_cas();

    let created = service
        .cas_task_create(Parameters(TaskCreateRequest {
            title: "Status readback".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    // Move to InProgress.
    service
        .cas_task_update(Parameters(TaskUpdateRequest {
            id: id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: None,
            status: Some("in_progress".to_string()),
            epic: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("update to in_progress");

    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: id.clone(),
            with_deps: false,
        }))
        .await
        .expect("show");
    let text = extract_text(shown);
    assert!(
        text.contains("InProgress")
            || text.contains("In Progress")
            || text.contains("in_progress"),
        "show immediately after update must return InProgress: {text}"
    );

    // Now flip back to Open. Show must reflect Open, not a cached InProgress.
    service
        .cas_task_update(Parameters(TaskUpdateRequest {
            id: id.clone(),
            title: None,
            notes: None,
            priority: None,
            labels: None,
            description: None,
            design: None,
            acceptance_criteria: None,
            demo_statement: None,
            execution_note: None,
            external_ref: None,
            assignee: Some("new-worker".to_string()),
            status: Some("open".to_string()),
            epic: None,
            epic_verification_owner: None,
        }))
        .await
        .expect("update back to open");

    let shown = service
        .cas_task_show(Parameters(TaskShowRequest {
            id: id.clone(),
            with_deps: false,
        }))
        .await
        .expect("show");
    let text = extract_text(shown);
    assert!(
        text.contains("Open") || text.contains("open"),
        "show immediately after update back to open must not return stale InProgress: {text}"
    );
    assert!(
        !text.contains("InProgress") && !text.contains("In Progress"),
        "show output must not contain stale InProgress status after update to Open: {text}"
    );
}

#[tokio::test]
async fn test_task_mine_matches_case_insensitive_and_trimmed() {
    let (_temp, service) = setup_cas();

    // Exercise the defensive matching path: assignee spelled with differing
    // case and surrounding whitespace still matches the current agent.
    let create_req = TaskCreateRequest {
        title: "Case-trim mine match".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let created = service
        .cas_task_create(Parameters(create_req))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    let update_req = TaskUpdateRequest {
        id: id.clone(),
        title: None,
        notes: None,
        priority: None,
        labels: None,
        description: None,
        design: None,
        acceptance_criteria: None,
        demo_statement: None,
        execution_note: None,
        external_ref: None,
        // The default test-agent name is "test-agent"; assert we still
        // match when the supervisor sprays mixed case + whitespace.
        assignee: Some("  TEST-Agent  ".to_string()),
        status: None,
        epic: None,
        epic_verification_owner: None,
    };
    service
        .cas_task_update(Parameters(update_req))
        .await
        .expect("update");

    let mine_req = LimitRequest {
        limit: Some(20),
        scope: "all".to_string(),
        sort: None,
        sort_order: None,
        team_id: None,
    };
    let result = service
        .cas_tasks_mine(Parameters(mine_req))
        .await
        .expect("mine");
    let text = extract_text(result);
    assert!(
        text.contains(&id),
        "mine should tolerate case + whitespace drift in assignee: {text}"
    );
}

// ============================================================================
// cas-85bf: Task ownership errors surface worker name (not just UUID)
// ============================================================================

/// When a task is locked by another worker, the "locked by" error must include
/// the holding worker's friendly name alongside the session UUID so the
/// supervisor can identify who has the task without cross-referencing
/// worker_status output.
#[tokio::test]
async fn test_task_start_locked_error_includes_worker_name() {
    use cas::store::open_agent_store;
    use cas::types::{Agent, AgentRole};

    let (temp, service) = setup_cas();
    let cas_dir = service.project_path().to_path_buf();

    // Register a "blocker" worker with a recognizable name.
    const BLOCKER_SESSION: &str = "blocker-session-0000-0000-000000000001";
    const BLOCKER_NAME: &str = "worker-backfill";

    let blocker = Agent::new_with_role(
        BLOCKER_SESSION.to_string(),
        BLOCKER_NAME.to_string(),
        AgentRole::Worker,
    );
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    agent_store.register(&blocker).expect("register blocker");

    // Create a task.
    let created = service
        .cas_task_create(Parameters(TaskCreateRequest {
            title: "Locked task for name-in-error test".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();

    // Have the blocker claim the task directly at store level.
    agent_store
        .try_claim(&id, BLOCKER_SESSION, 600, Some("blocking for test"))
        .expect("blocker claim");

    // Now try to start the same task via the test service — should fail
    // because our test agent doesn't own the lease.
    let start_err = service
        .cas_task_start(Parameters(cas::mcp::tools::IdRequest { id: id.clone() }))
        .await
        .expect_err("start must fail when another agent holds the lease");

    let msg = start_err.message.to_string();
    assert!(
        msg.contains(BLOCKER_NAME),
        "error must contain holder's name '{BLOCKER_NAME}': {msg}"
    );
    assert!(
        msg.contains(BLOCKER_SESSION),
        "error must contain holder's session UUID '{BLOCKER_SESSION}': {msg}"
    );
    assert!(
        msg.contains("locked"),
        "error must mention 'locked': {msg}"
    );

    drop(temp);
}

// worker_status UUID surfacing is verified by code inspection + build:
// factory_ops.rs emits "    session: {uuid}" for every active worker entry.
// The format is tested indirectly via the lib unit test
// `test_worker_status_format_includes_session_uuid` in factory_ops.rs.
