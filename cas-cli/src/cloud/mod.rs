//! Cloud sync module for CAS
//!
//! Provides automatic synchronization of CAS data with CAS Cloud.
//!
//! # Components
//!
//! - [`CloudConfig`] - Cloud authentication and endpoint configuration
//! - [`SyncQueue`] - Persistent queue for pending sync operations
//! - [`CloudSyncer`] - Push/pull synchronization logic
//! - [`CloudCoordinator`] - Multi-agent coordination via cloud
//!
//! # Architecture
//!
//! Auto-sync works by:
//! 1. Queueing changes on write operations (non-blocking)
//! 2. Processing the queue during idle periods (daemon)
//! 3. Pulling latest changes on MCP server startup

mod config;
mod coordinator;
pub mod device;
pub(crate) mod me;
mod sync_queue;
mod syncer;

pub use config::{
    CloudConfig, TeamInfo, canonical_id_from_config_toml, derive_canonical_id_from_git_remote,
    get_project_canonical_id, set_canonical_id_in_config_toml,
};
pub(crate) use config::{default_endpoint, is_acceptable_endpoint, user_level_cloud_json_path};
// T2: /api/me fetch helpers — `pub` so integration tests can call them directly.
pub use me::{FetchTeamsOutcome, fetch_and_cache_teams, fetch_and_cache_teams_inner,
    teams_cache_stale};
#[cfg(test)]
pub(crate) use config::CLOUD_ENV_LOCK;
pub use coordinator::CloudCoordinator;
pub use device::DeviceConfig;
pub use sync_queue::{EntityType, QueuedSync, SyncOperation, SyncQueue};
pub use syncer::{
    CloudSyncer, CloudSyncerConfig, ConflictAction, ConflictResolution, SyncConflict, SyncResult,
    TeamMemoriesResponse, TeamProject, TeamProjectsResponse,
};
