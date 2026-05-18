use crate::mcp::tools::core::imports::*;

impl CasCore {
    pub async fn cas_task_dep_add(
        &self,
        Parameters(req): Parameters<DependencyRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        // Verify both tasks exist
        task_store.get(&req.from_id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("From task not found: {e}")),
            data: None,
        })?;
        task_store.get(&req.to_id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("To task not found: {e}")),
            data: None,
        })?;

        let dep_type = match req.dep_type.to_lowercase().as_str() {
            "related" => DependencyType::Related,
            "parent" | "parentchild" => DependencyType::ParentChild,
            "discovered" | "discoveredfrom" => DependencyType::DiscoveredFrom,
            _ => DependencyType::Blocks,
        };

        let dep = Dependency {
            from_id: req.from_id.clone(),
            to_id: req.to_id.clone(),
            dep_type,
            created_at: chrono::Utc::now(),
            created_by: Some("mcp".to_string()),
        };

        task_store.add_dependency(&dep).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to add dependency: {e}")),
            data: None,
        })?;

        Ok(Self::success(format!(
            "Added {:?} dependency: {} -> {}",
            dep.dep_type, req.from_id, req.to_id
        )))
    }

    /// Remove a dependency of a specific type between tasks (cas-6009).
    ///
    /// Uses `dep_type` to target exactly one dependency row, leaving any
    /// other-typed deps between the same pair untouched.
    pub async fn cas_task_dep_remove(
        &self,
        Parameters(req): Parameters<DependencyRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        let dep_type = match req.dep_type.to_lowercase().as_str() {
            "related" => DependencyType::Related,
            "parent" | "parentchild" | "parent-child" => DependencyType::ParentChild,
            "discovered" | "discoveredfrom" | "discovered-from" => DependencyType::DiscoveredFrom,
            "extracted" | "extractedfrom" | "extracted-from" => DependencyType::ExtractedFrom,
            _ => DependencyType::Blocks,
        };

        let found = task_store
            .remove_dependency_of_type(&req.from_id, &req.to_id, dep_type)
            .map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to remove dependency: {e}")),
                data: None,
            })?;

        if !found {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "No {:?} dependency found from {} to {}",
                    dep_type, req.from_id, req.to_id
                )),
                data: None,
            });
        }

        Ok(Self::success(format!(
            "Removed {:?} dependency: {} -> {}",
            dep_type, req.from_id, req.to_id
        )))
    }
}
