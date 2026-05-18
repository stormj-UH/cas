use std::path::Path;

pub(crate) const CAS_SECTION_BEGIN: &str =
    "<!-- CAS:BEGIN - This section is managed by CAS. Do not edit manually. -->";
pub(crate) const CAS_SECTION_END: &str = "<!-- CAS:END -->";

/// CAS directive content (MCP tools)
const CAS_DIRECTIVE_CONTENT: &str = r#"# IMPORTANT: USE CAS FOR TASK AND MEMORY MANAGEMENT

**DO NOT USE BUILT-IN TOOLS (TodoWrite, EnterPlanMode) FOR TASK TRACKING.**

Use CAS MCP tools instead:
First use each session — load MCP schemas: ToolSearch(query="select:mcp__cas__task,mcp__cas__memory,mcp__cas__search")
- `mcp__cas__task` with action: create - Create tasks (NOT TodoWrite)
- `mcp__cas__task` with action: start/close - Manage task status
- `mcp__cas__task` with action: ready - See ready tasks
- `mcp__cas__memory` with action: remember - Store memories and learnings
- `mcp__cas__search` with action: search - Search all context

CAS provides persistent context across sessions. Built-in tools are ephemeral."#;

/// Build the full CAS section with markers
pub(crate) fn build_cas_section() -> String {
    format!("{CAS_SECTION_BEGIN}\n{CAS_DIRECTIVE_CONTENT}\n{CAS_SECTION_END}")
}

/// Update or create CLAUDE.md with CAS directive section
/// Returns Ok(true) if file was modified, Ok(false) if no changes needed
pub fn update_claude_md(project_root: &Path) -> anyhow::Result<bool> {
    let claude_md_path = project_root.join("CLAUDE.md");
    let new_section = build_cas_section();

    if claude_md_path.exists() {
        let content = std::fs::read_to_string(&claude_md_path)?;

        // Check for marked section
        if let (Some(begin_pos), Some(end_pos)) = (
            content.find(CAS_SECTION_BEGIN),
            content.find(CAS_SECTION_END),
        ) {
            // Replace existing marked section
            let before = &content[..begin_pos];
            let after = &content[end_pos + CAS_SECTION_END.len()..];
            let new_content = format!(
                "{}{}{}",
                before.trim_end(),
                if before.is_empty() { "" } else { "\n" },
                new_section
            );
            let new_content = format!("{new_content}{after}");

            if new_content == content {
                return Ok(false);
            }
            std::fs::write(&claude_md_path, new_content)?;
            return Ok(true);
        }

        // Check for old-style directive (migration path)
        if content.contains("IMPORTANT: USE CAS FOR TASK AND MEMORY MANAGEMENT") {
            let new_content = if content.starts_with("# IMPORTANT: USE CAS") {
                if let Some(pos) = content.find("---\n\n") {
                    format!("{}\n\n{}", new_section, &content[pos + 5..])
                } else if let Some(pos) = content.find("---\n") {
                    format!("{}\n\n{}", new_section, &content[pos + 4..])
                } else {
                    format!("{new_section}\n\n{content}")
                }
            } else {
                format!("{new_section}\n\n{content}")
            };
            std::fs::write(&claude_md_path, new_content)?;
            return Ok(true);
        }

        // Prepend new section to existing content
        let new_content = format!("{new_section}\n\n{content}");
        std::fs::write(&claude_md_path, new_content)?;
        Ok(true)
    } else {
        std::fs::write(&claude_md_path, format!("{new_section}\n"))?;
        Ok(true)
    }
}

// ============================================================================
// CAS skill generation
// ============================================================================

pub(crate) const CAS_SKILL: &str = r#"---
name: cas
description: Coding Agent System - unified memory, tasks, rules, and skills. Use when you need to remember something, track work, search past context, or manage tasks. (project)
managed_by: cas
---

# CAS - Coding Agent System

**IMPORTANT: Use CAS MCP tools instead of built-in tools for task and memory management.**

CAS provides persistent memory and task management across sessions. Built-in tools like TodoWrite are ephemeral and don't persist.

## WHEN TO USE CAS (ALWAYS)

- **Task tracking**: Use `mcp__cas__task` with action: create instead of TodoWrite
- **Planning tasks**: Use `mcp__cas__task` with action: create and blocked_by for dependencies
- **Storing learnings**: Use `mcp__cas__memory` with action: remember to store context
- **Searching context**: Use `mcp__cas__search` with action: search to find past work

## Task Tools (USE INSTEAD OF TodoWrite)

### Creating Tasks

Use `mcp__cas__task` with action: create and parameters:
- `title` (required) - Task title
- `priority` - 0=critical, 1=high, 2=medium (default), 3=low, 4=backlog
- `start` - Set to true to start immediately (RECOMMENDED)
- `notes` - Initial working notes

### Managing Tasks

All task operations use `mcp__cas__task` with different actions:
- action: ready - Show tasks ready to work on
- action: blocked - Show blocked tasks
- action: list - List all tasks
- action: show - Show task details (requires id)
- action: update - Update notes as you work (requires id)
- action: close - Close with resolution (requires id)

### Task Dependencies

- action: dep_add - Add blocking dependency (requires id, to_id)
- action: dep_list - List dependencies (requires id)

## Memory Tools

All memory operations use `mcp__cas__memory` with different actions:
- action: remember - Store a memory entry (requires content)
- action: get - Get entry details (requires id)
- action: helpful - Mark as helpful (requires id)
- action: harmful - Mark as harmful (requires id)

## Search Tools

Use `mcp__cas__search` with different actions:
- action: search - Search memories (requires query)
- action: context - Get full session context

## Iteration Loops

Use loops for long-running repetitive tasks. The loop blocks session exit and re-injects your prompt until completion.

Use `mcp__cas__coordination` with different actions:
- action: loop_start - Start a loop (requires prompt, session_id, optional completion_promise and max_iterations)
- action: loop_status - Check current loop status (requires session_id)
- action: loop_cancel - Cancel active loop (requires session_id)

To complete a loop, output `<promise>DONE</promise>` (or your custom promise text).

## Rules & Skills

Use `mcp__cas__rule` and `mcp__cas__skill` with different actions:
- rule action: list - Show active rules
- rule action: helpful - Promote rule to proven (requires id)
- skill action: list - Show enabled skills
"#;

/// Check if a file is managed by CAS (has `managed_by: cas` in frontmatter)
pub(crate) fn is_skill_managed_by_cas(content: &str) -> bool {
    if let Some(stripped) = content.strip_prefix("---") {
        if let Some(end) = stripped.find("---") {
            let frontmatter = &content[3..3 + end];
            return frontmatter.contains("managed_by: cas")
                || frontmatter.contains("managed_by: \"cas\"");
        }
    }
    false
}

/// Check if a file is the old CAS skill (for migration)
pub(crate) fn is_old_cas_skill(content: &str) -> bool {
    if let Some(stripped) = content.strip_prefix("---") {
        if let Some(end) = stripped.find("---") {
            let frontmatter = &content[3..3 + end];
            return frontmatter.contains("name: cas") && !frontmatter.contains("managed_by:");
        }
    }
    false
}

/// Generate CAS skill file
pub fn generate_cas_skill(project_root: &Path) -> anyhow::Result<bool> {
    let skill_dir = project_root.join(".claude/skills/cas");
    let skill_path = skill_dir.join("SKILL.md");
    let skill_content = CAS_SKILL;

    std::fs::create_dir_all(&skill_dir)?;

    if skill_path.exists() {
        let existing = std::fs::read_to_string(&skill_path)?;

        if existing == skill_content {
            return Ok(false);
        }

        if !is_skill_managed_by_cas(&existing) && !is_old_cas_skill(&existing) {
            return Ok(false);
        }
    }

    std::fs::write(&skill_path, skill_content)?;
    Ok(true)
}

// ============================================================================
// Agent and command generation (using builtins)
// ============================================================================

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The managed block must document the ToolSearch bootstrap query so that
    /// Claude knows how to load MCP schemas before calling task/memory/search.
    #[test]
    fn template_documents_toolsearch_bootstrap() {
        let section = build_cas_section();
        assert!(
            section.contains(r#"ToolSearch(query="select:mcp__cas__task,mcp__cas__memory,mcp__cas__search")"#),
            "Managed block must contain the exact ToolSearch bootstrap query; got:\n{section}"
        );
    }

    /// The full managed block (markers + content) must stay ≤ 18 lines to
    /// keep the per-session context tax small (sister task cas-253e dedupes
    /// the block; this bounds its cost when it *does* appear).
    #[test]
    fn managed_block_line_count_within_budget() {
        let section = build_cas_section();
        let line_count = section.lines().count();
        assert!(
            line_count <= 18,
            "Managed CLAUDE.md block must be ≤ 18 lines (current: {line_count});\
             if you added content, trim elsewhere"
        );
    }
}
