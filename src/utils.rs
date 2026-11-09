use super::base_url::{BaseDirInfo, HostingProvider};
use super::def::{DEFAULT_BYTE_BUDGET, DEFAULT_FLUSH_INTERVAL_MS, DEFAULT_SHARD_BYTES};
use napi_derive::napi;
use std::time::Duration;
/// A file-change event delivered to change callbacks
/// (see [`BaganCache::on_change`]).
#[napi(object)]
#[derive(Debug, Clone)]
pub struct CacheChangeEvent {
    /// Absolute path of the changed file.
    pub path: String,
    /// `"create"`, `"modify"`, `"remove"`, or `"other"`.
    pub kind: String,
    /// `true` when the file is under the markdown entry directory.
    pub is_markdown: bool,
}

/// An entry stored in the cache: raw UTF-8 text (markdown and textual
/// assets) or raw bytes (binary assets). Exposed to JavaScript so JS
/// consumers can branch on the variant. The serde derives back the
/// bincode round-trip through the disk archive.
#[napi]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CachedContent {
    /// UTF-8 text content (markdown files and textual assets).
    Text(String),
    /// Raw bytes (binary assets like images or fonts).
    Binary(Vec<u8>),
}

/// LRU budgets for the memory tier.
#[napi(object)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryConfig {
    /// Maximum total bytes of raw content kept in memory (`0` =
    /// unlimited). When the budget is exceeded, the least recently used
    /// entries are evicted (they remain readable from the disk archive).
    pub byte_budget: Option<i64>,
    /// Maximum number of entries in memory (`0` = unlimited).
    pub entry_budget: Option<u32>,
}

impl MemoryConfig {
    /// Resolved byte budget (`0` = unlimited).
    pub fn byte_budget(&self) -> u64 {
        self.byte_budget
            .unwrap_or(DEFAULT_BYTE_BUDGET as i64)
            .max(0) as u64
    }

    /// Resolved entry budget (`0` = unlimited).
    pub fn entry_budget(&self) -> usize {
        self.entry_budget.unwrap_or(0) as usize
    }
}

/// Flush thresholds for the disk-archive tier.
#[napi(object)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ArchiveConfig {
    /// Pending-content threshold (bytes) that triggers writing a new
    /// shard; `0` disables size-based flushing (interval/drop only).
    pub shard_bytes: Option<i64>,
    /// Maximum time (ms) pending entries may sit in the worker buffer
    /// before being flushed. `0` disables the timer.
    pub flush_interval_ms: Option<i64>,
}

impl ArchiveConfig {
    /// Resolved shard threshold.
    pub fn shard_bytes(&self) -> u64 {
        self.shard_bytes
            .unwrap_or(DEFAULT_SHARD_BYTES as i64)
            .max(0) as u64
    }

    /// Resolved flush interval.
    pub fn flush_interval(&self) -> Duration {
        Duration::from_millis(
            self.flush_interval_ms
                .unwrap_or(DEFAULT_FLUSH_INTERVAL_MS as i64)
                .max(0) as u64,
        )
    }
}

/// Static-hosting configuration for baseUrl resolution.
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct HostingConfig {
    /// Hosting provider (see [`HostingProvider`]).
    #[napi(ts_type = "BaganHostingProvider")]
    pub provider: Option<HostingProvider>,
    /// GitHub repository name for project pages (`/<repo>/` prefix).
    pub repo: Option<String>,
    /// Custom site URL (overrides provider defaults when set).
    pub site_url: Option<String>,
    /// Extra path prefix (e.g. `"subpath"` for `example.com/subpath/`).
    pub path_prefix: Option<String>,
}

impl HostingConfig {
    pub fn resolve(&self) -> BaseDirInfo {
        let mut info = BaseDirInfo::new(
            self.provider.unwrap_or(HostingProvider::Custom),
            self.repo.clone(),
            self.site_url.clone(),
        );
        info.path_prefix = self.path_prefix.clone();
        info
    }
}

impl Default for BaseDirInfo {
    fn default() -> Self {
        Self::new(HostingProvider::Custom, None, None)
    }
}
