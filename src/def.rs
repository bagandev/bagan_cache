/// Directory name of the on-disk archive under the working directory.
pub const DEFAULT_ARCHIVE_DIR_NAME: &str = ".bagan_cache";
/// Default build output directory.
pub const DEFAULT_OUT_DIR: &str = ".bagan";
/// Directory under the current working directory holding theme packages
/// installed by a package manager (`node_modules/<theme>`).
pub const NODE_MODULES_DIR: &str = "node_modules";
/// Default LRU byte budget when none is configured: 64 MiB.
pub const DEFAULT_BYTE_BUDGET: u64 = 64 * 1024 * 1024;
/// Default pending-content shard threshold: 8 MiB.
pub const DEFAULT_SHARD_BYTES: u64 = 8 * 1024 * 1024;
/// Default archive flush interval: 2 seconds.
pub const DEFAULT_FLUSH_INTERVAL_MS: u64 = 2_000;
