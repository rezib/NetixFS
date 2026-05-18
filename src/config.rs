//! Configuration system: TOML file < env vars < CLI args.
//!
//! Every knob has three possible sources. Precedence (lowest → cli_envest):
//!   1. TOML file  (pointed to by `--config-file` / `NETIXFS_CONFIG_FILE`)
//!   2. Environment variables
//!   3. CLI flags
//!
//! All three sources share one family of `Partial*` structs (every field is
//! `Option<T>` or `Vec<T>`). After the three layers are merged,
//! `PartialConfig::resolve` converts them into the fully-typed `Config` and its
//! sub-structs, applying defaults and validating mandatory constraints.

use clap::{Args, Parser};
use eyre::{Result, WrapErr, bail};
use serde::Deserialize;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

// ─────────────────────────────────────────────────────────────────────────────
// Enums
// ─────────────────────────────────────────────────────────────────────────────

/// Log verbosity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl From<LogLevel> for tracing::Level {
    fn from(l: LogLevel) -> Self {
        match l {
            LogLevel::Error => tracing::Level::ERROR,
            LogLevel::Warn => tracing::Level::WARN,
            LogLevel::Info => tracing::Level::INFO,
            LogLevel::Debug => tracing::Level::DEBUG,
            LogLevel::Trace => tracing::Level::TRACE,
        }
    }
}

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LogFormat {
    Json,
    Pretty,
    Compact,
}

/// How symbolic links are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SymlinkPolicy {
    Reject,
    FollowSafe,
}

// ─────────────────────────────────────────────────────────────────────────────
// Fully-resolved structs (no Options, fully typed)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct Root {
    pub id: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) enum JwtKeySource {
    PublicKeyPath(PathBuf),
    PublicKeyUrl(String),
    JwksPath(PathBuf),
    JwksUrl(String),
}

#[derive(Debug, Clone)]
pub(crate) struct JwtConfig {
    /// `None` is permitted here; a later step will enforce at least one source.
    pub source: Option<JwtKeySource>,
    pub issuer: Option<String>,
    pub audience: Option<String>,
    pub username_claim: String,
    pub remote_key_refresh_interval: Duration,
}

#[derive(Debug, Clone)]
pub(crate) struct AuthConfig {
    pub jwt: JwtConfig,
}

#[derive(Debug, Clone)]
pub(crate) struct ServerConfig {
    pub bind_address: IpAddr,
    pub port: u16,
    pub public_base_url: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct TlsConfig {
    pub enabled: bool,
    pub cert_path: Option<PathBuf>,
    pub key_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct FilesystemConfig {
    pub allowed_roots: Vec<Root>,
    pub read_only: bool,
    pub default_file_mode: u32,
    pub default_dir_mode: u32,
    pub umask: Option<u32>,
    pub symlink_policy: SymlinkPolicy,
    pub allow_mount_crossing: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct OperationsConfig {
    pub allow_recursive_delete: bool,
    pub allow_recursive_copy: bool,
    pub allow_chmod: bool,
    pub allow_hard_links: bool,
    pub allow_symlink_create: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct LimitsConfig {
    pub max_request_body_size: u64,
    pub max_read_size: u64,
    pub max_directory_entries: u32,
    pub max_concurrent_requests: u32,
    pub max_concurrent_streams: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct StreamingConfig {
    pub idle_timeout: Duration,
    pub max_duration: Duration,
    pub heartbeat_interval: Duration,
}

#[derive(Debug, Clone)]
pub(crate) struct PoolConfig {
    pub max_workers: u32,
    pub idle_timeout: Duration,
    pub request_timeout: Duration,
}

#[derive(Debug, Clone)]
pub(crate) struct LoggingConfig {
    pub level: LogLevel,
    pub format: LogFormat,
    pub redact_paths: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct MetricsConfig {
    pub enabled: bool,
    pub bind_address: IpAddr,
    pub port: u16,
}

#[derive(Debug, Clone)]
pub(crate) struct ConfigEndpointConfig {
    pub enabled: bool,
    pub bind_address: SocketAddr,
}

#[derive(Debug, Clone)]
pub(crate) struct DiagnosticsConfig {
    pub config_endpoint: ConfigEndpointConfig,
}

#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub server: ServerConfig,
    pub tls: TlsConfig,
    pub auth: AuthConfig,
    pub filesystem: FilesystemConfig,
    pub operations: OperationsConfig,
    pub limits: LimitsConfig,
    pub streaming: StreamingConfig,
    pub pool: PoolConfig,
    pub logging: LoggingConfig,
    pub metrics: MetricsConfig,
    pub diagnostics: DiagnosticsConfig,
}

// ─────────────────────────────────────────────────────────────────────────────
// Partial structs  (one per config section; every field Option<T> or Vec<T>)
//
// Each sub-struct derives:
//   - Debug, Clone, Default  – basic ergonomics
//   - serde::Deserialize     – TOML file parsing
//   - clap::Args             – CLI / env-var parsing (flattened into PartialConfig)
// ─────────────────────────────────────────────────────────────────────────────

// ── [server] ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize, Args)]
pub(crate) struct ServerPartial {
    #[arg(long = "bind-address", env = "NETIXFS_SERVER_BIND_ADDRESS")]
    pub bind_address: Option<IpAddr>,

    #[arg(long = "port", env = "NETIXFS_SERVER_PORT")]
    pub port: Option<u16>,

    #[arg(long = "public-base-url", env = "NETIXFS_SERVER_PUBLIC_BASE_URL")]
    pub public_base_url: Option<String>,
}

// ── [tls] ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize, Args)]
pub(crate) struct TlsPartial {
    #[arg(long = "tls-enabled", env = "NETIXFS_TLS_ENABLED")]
    pub enabled: Option<bool>,

    #[arg(long = "tls-cert-path", env = "NETIXFS_TLS_CERT_PATH")]
    pub cert_path: Option<PathBuf>,

    #[arg(long = "tls-key-path", env = "NETIXFS_TLS_KEY_PATH")]
    pub key_path: Option<PathBuf>,
}

// ── [auth.jwt] ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize, Args)]
pub(crate) struct JwtPartial {
    #[arg(long = "jwt-public-key-path", env = "NETIXFS_AUTH_JWT_PUBLIC_KEY_PATH")]
    pub public_key_path: Option<PathBuf>,

    #[arg(long = "jwt-public-key-url", env = "NETIXFS_AUTH_JWT_PUBLIC_KEY_URL")]
    pub public_key_url: Option<String>,

    #[arg(long = "jwt-jwks-path", env = "NETIXFS_AUTH_JWT_JWKS_PATH")]
    pub jwks_path: Option<PathBuf>,

    #[arg(long = "jwt-jwks-url", env = "NETIXFS_AUTH_JWT_JWKS_URL")]
    pub jwks_url: Option<String>,

    #[arg(long = "jwt-issuer", env = "NETIXFS_AUTH_JWT_ISSUER")]
    pub issuer: Option<String>,

    #[arg(long = "jwt-audience", env = "NETIXFS_AUTH_JWT_AUDIENCE")]
    pub audience: Option<String>,

    #[arg(long = "jwt-username-claim", env = "NETIXFS_AUTH_JWT_USERNAME_CLAIM")]
    pub username_claim: Option<String>,

    #[arg(
        long = "jwt-remote-key-refresh-interval",
        env = "NETIXFS_AUTH_JWT_REMOTE_KEY_REFRESH_INTERVAL"
    )]
    pub remote_key_refresh_interval: Option<String>,
}

// ── [auth] ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize, Args)]
pub(crate) struct AuthPartial {
    /// TOML: [auth.jwt]   CLI: flattened into parent
    #[command(flatten)]
    #[serde(default)]
    pub jwt: JwtPartial,
}

// ── [filesystem] ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize, Args)]
pub(crate) struct FilesystemPartial {
    /// Filesystem roots in "id=path" format. Repeatable or comma-separated.
    #[arg(
        long = "allowed-root",
        env = "NETIXFS_FILESYSTEM_ALLOWED_ROOTS",
        value_delimiter = ','
    )]
    pub allowed_roots: Vec<String>,

    #[arg(long = "read-only", env = "NETIXFS_FILESYSTEM_READ_ONLY")]
    pub read_only: Option<bool>,

    /// Octal mode string, e.g. "0644".
    #[arg(
        long = "default-file-mode",
        env = "NETIXFS_FILESYSTEM_DEFAULT_FILE_MODE"
    )]
    pub default_file_mode: Option<String>,

    /// Octal mode string, e.g. "0755".
    #[arg(long = "default-dir-mode", env = "NETIXFS_FILESYSTEM_DEFAULT_DIR_MODE")]
    pub default_dir_mode: Option<String>,

    /// Octal umask string, e.g. "0022". Defaults to the process umask (None).
    #[arg(long = "umask", env = "NETIXFS_FILESYSTEM_UMASK")]
    pub umask: Option<String>,

    #[arg(long = "symlink-policy", env = "NETIXFS_FILESYSTEM_SYMLINK_POLICY")]
    pub symlink_policy: Option<SymlinkPolicy>,

    #[arg(
        long = "allow-mount-crossing",
        env = "NETIXFS_FILESYSTEM_ALLOW_MOUNT_CROSSING"
    )]
    pub allow_mount_crossing: Option<bool>,
}

// ── [operations] ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize, Args)]
pub(crate) struct OperationsPartial {
    #[arg(
        long = "allow-recursive-delete",
        env = "NETIXFS_OPERATIONS_ALLOW_RECURSIVE_DELETE"
    )]
    pub allow_recursive_delete: Option<bool>,

    #[arg(
        long = "allow-recursive-copy",
        env = "NETIXFS_OPERATIONS_ALLOW_RECURSIVE_COPY"
    )]
    pub allow_recursive_copy: Option<bool>,

    #[arg(long = "allow-chmod", env = "NETIXFS_OPERATIONS_ALLOW_CHMOD")]
    pub allow_chmod: Option<bool>,

    #[arg(long = "allow-hard-links", env = "NETIXFS_OPERATIONS_ALLOW_HARD_LINKS")]
    pub allow_hard_links: Option<bool>,

    #[arg(
        long = "allow-symlink-create",
        env = "NETIXFS_OPERATIONS_ALLOW_SYMLINK_CREATE"
    )]
    pub allow_symlink_create: Option<bool>,
}

// ── [limits] ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize, Args)]
pub(crate) struct LimitsPartial {
    /// Human-readable byte size, e.g. "100MiB".
    #[arg(
        long = "max-request-body-size",
        env = "NETIXFS_LIMITS_MAX_REQUEST_BODY_SIZE"
    )]
    pub max_request_body_size: Option<String>,

    /// Human-readable byte size, e.g. "100MiB".
    #[arg(long = "max-read-size", env = "NETIXFS_LIMITS_MAX_READ_SIZE")]
    pub max_read_size: Option<String>,

    #[arg(
        long = "max-directory-entries",
        env = "NETIXFS_LIMITS_MAX_DIRECTORY_ENTRIES"
    )]
    pub max_directory_entries: Option<u32>,

    #[arg(
        long = "max-concurrent-requests",
        env = "NETIXFS_LIMITS_MAX_CONCURRENT_REQUESTS"
    )]
    pub max_concurrent_requests: Option<u32>,

    #[arg(
        long = "max-concurrent-streams",
        env = "NETIXFS_LIMITS_MAX_CONCURRENT_STREAMS"
    )]
    pub max_concurrent_streams: Option<u32>,
}

// ── [streaming] ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize, Args)]
pub(crate) struct StreamingPartial {
    /// Human-readable duration, e.g. "5m".
    #[arg(
        id = "stream-idle-timeout",
        long = "stream-idle-timeout",
        env = "NETIXFS_STREAMING_IDLE_TIMEOUT"
    )]
    pub idle_timeout: Option<String>,

    /// Human-readable duration, e.g. "1h".
    #[arg(long = "stream-max-duration", env = "NETIXFS_STREAMING_MAX_DURATION")]
    pub max_duration: Option<String>,

    /// Human-readable duration, e.g. "30s".
    #[arg(
        long = "stream-heartbeat-interval",
        env = "NETIXFS_STREAMING_HEARTBEAT_INTERVAL"
    )]
    pub heartbeat_interval: Option<String>,
}

// ── [pool] ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize, Args)]
pub(crate) struct PoolPartial {
    #[arg(long = "pool-max-workers", env = "NETIXFS_POOL_MAX_WORKERS")]
    pub max_workers: Option<u32>,

    /// Human-readable duration, e.g. "5m".
    #[arg(
        id = "pool-idle-timeout",
        long = "pool-idle-timeout",
        env = "NETIXFS_POOL_IDLE_TIMEOUT"
    )]
    pub idle_timeout: Option<String>,

    /// Human-readable duration, e.g. "30s".
    #[arg(long = "pool-request-timeout", env = "NETIXFS_POOL_REQUEST_TIMEOUT")]
    pub request_timeout: Option<String>,
}

// ── [logging] ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize, Args)]
pub(crate) struct LoggingPartial {
    #[arg(long = "log-level", env = "NETIXFS_LOGGING_LEVEL")]
    pub level: Option<LogLevel>,

    #[arg(long = "log-format", env = "NETIXFS_LOGGING_FORMAT")]
    pub format: Option<LogFormat>,

    #[arg(long = "log-redact-paths", env = "NETIXFS_LOGGING_REDACT_PATHS")]
    pub redact_paths: Option<bool>,
}

// ── [metrics] ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize, Args)]
pub(crate) struct MetricsPartial {
    #[arg(
        id = "metrics-enabled",
        long = "metrics-enabled",
        env = "NETIXFS_METRICS_ENABLED"
    )]
    pub enabled: Option<bool>,

    #[arg(
        id = "metrics-bind-address",
        long = "metrics-bind-address",
        env = "NETIXFS_METRICS_BIND_ADDRESS"
    )]
    pub bind_address: Option<IpAddr>,

    #[arg(
        id = "metrics-port",
        long = "metrics-port",
        env = "NETIXFS_METRICS_PORT"
    )]
    pub port: Option<u16>,
}

// ── [diagnostics.config_endpoint] ────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize, Args)]
pub(crate) struct ConfigEndpointPartial {
    #[arg(
        id = "config-endpoint-enabled",
        long = "config-endpoint-enabled",
        env = "NETIXFS_DIAGNOSTICS_CONFIG_ENDPOINT_ENABLED"
    )]
    pub enabled: Option<bool>,

    #[arg(
        id = "config-endpoint-bind-address",
        long = "config-endpoint-bind-address",
        env = "NETIXFS_DIAGNOSTICS_CONFIG_ENDPOINT_BIND_ADDRESS"
    )]
    pub bind_address: Option<SocketAddr>,
}

// ── [diagnostics] ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize, Args)]
pub(crate) struct DiagnosticsPartial {
    /// TOML: [diagnostics.config_endpoint]   CLI: flattened into parent
    #[command(flatten)]
    #[serde(default)]
    pub config_endpoint: ConfigEndpointPartial,
}

// ─────────────────────────────────────────────────────────────────────────────
// Top-level PartialConfig
// ─────────────────────────────────────────────────────────────────────────────

// All configuration knobs in their optional/partial form.
//
// Parsed three ways:
//   * TOML file         (lowest priority)  — via `serde::Deserialize`
//   * Environment vars  (middle priority)  — via clap `env` attributes
//   * CLI flags         (cli_envest priority) — via `clap::Parser`
//
// The three layers are merged by `merge()` then resolved into `Config`.
#[derive(Debug, Clone, Default, Deserialize, Parser)]
#[command(version, about, long_about = None)]
struct CliEnvConfig {
    /// Path to the TOML configuration file.
    #[arg(
        short = 'c',
        long = "config-file",
        env = "NETIXFS_CONFIG_FILE",
        default_value = "/etc/netixfs/config.toml"
    )]
    config_file: PathBuf,

    #[command(flatten)]
    config: PartialConfig,
}

#[derive(Debug, Clone, Default, Deserialize, Args)]
pub(crate) struct PartialConfig {
    // TOML: [server]
    #[command(flatten)]
    #[serde(default)]
    pub server: ServerPartial,

    // TOML: [tls]
    #[command(flatten)]
    #[serde(default)]
    pub tls: TlsPartial,

    // TOML: [auth]  (nested [auth.jwt] handled inside AuthPartial)
    #[command(flatten)]
    #[serde(default)]
    pub auth: AuthPartial,

    // TOML: [filesystem]
    #[command(flatten)]
    #[serde(default)]
    pub filesystem: FilesystemPartial,

    // TOML: [operations]
    #[command(flatten)]
    #[serde(default)]
    pub operations: OperationsPartial,

    // TOML: [limits]
    #[command(flatten)]
    #[serde(default)]
    pub limits: LimitsPartial,

    // TOML: [streaming]
    #[command(flatten)]
    #[serde(default)]
    pub streaming: StreamingPartial,

    // TOML: [pool]
    #[command(flatten)]
    #[serde(default)]
    pub pool: PoolPartial,

    // TOML: [logging]
    #[command(flatten)]
    #[serde(default)]
    pub logging: LoggingPartial,

    // TOML: [metrics]
    #[command(flatten)]
    #[serde(default)]
    pub metrics: MetricsPartial,

    // TOML: [diagnostics]  (nested [diagnostics.config_endpoint] inside DiagnosticsPartial)
    #[command(flatten)]
    #[serde(default)]
    pub diagnostics: DiagnosticsPartial,
}

// ─────────────────────────────────────────────────────────────────────────────
// Merge
// ─────────────────────────────────────────────────────────────────────────────

/// Merge two partial configs. `cli_env` takes precedence over `file`.
///
/// * `Option<T>` fields  → `cli_env.field.or(file.field)`
/// * `Vec<String>`       → use the non-empty side (`cli_env` first)
fn merge(cli_env: PartialConfig, file: PartialConfig) -> PartialConfig {
    PartialConfig {
        server: ServerPartial {
            bind_address: cli_env.server.bind_address.or(file.server.bind_address),
            port: cli_env.server.port.or(file.server.port),
            public_base_url: cli_env
                .server
                .public_base_url
                .or(file.server.public_base_url),
        },

        tls: TlsPartial {
            enabled: cli_env.tls.enabled.or(file.tls.enabled),
            cert_path: cli_env.tls.cert_path.or(file.tls.cert_path),
            key_path: cli_env.tls.key_path.or(file.tls.key_path),
        },

        auth: AuthPartial {
            jwt: JwtPartial {
                public_key_path: cli_env
                    .auth
                    .jwt
                    .public_key_path
                    .or(file.auth.jwt.public_key_path),
                public_key_url: cli_env
                    .auth
                    .jwt
                    .public_key_url
                    .or(file.auth.jwt.public_key_url),
                jwks_path: cli_env.auth.jwt.jwks_path.or(file.auth.jwt.jwks_path),
                jwks_url: cli_env.auth.jwt.jwks_url.or(file.auth.jwt.jwks_url),
                issuer: cli_env.auth.jwt.issuer.or(file.auth.jwt.issuer),
                audience: cli_env.auth.jwt.audience.or(file.auth.jwt.audience),
                username_claim: cli_env
                    .auth
                    .jwt
                    .username_claim
                    .or(file.auth.jwt.username_claim),
                remote_key_refresh_interval: cli_env
                    .auth
                    .jwt
                    .remote_key_refresh_interval
                    .or(file.auth.jwt.remote_key_refresh_interval),
            },
        },

        filesystem: FilesystemPartial {
            // For Vec fields: take whichever side is non-empty (cli_env wins).
            allowed_roots: if !cli_env.filesystem.allowed_roots.is_empty() {
                cli_env.filesystem.allowed_roots
            } else {
                file.filesystem.allowed_roots
            },
            read_only: cli_env.filesystem.read_only.or(file.filesystem.read_only),
            default_file_mode: cli_env
                .filesystem
                .default_file_mode
                .or(file.filesystem.default_file_mode),
            default_dir_mode: cli_env
                .filesystem
                .default_dir_mode
                .or(file.filesystem.default_dir_mode),
            umask: cli_env.filesystem.umask.or(file.filesystem.umask),
            symlink_policy: cli_env
                .filesystem
                .symlink_policy
                .or(file.filesystem.symlink_policy),
            allow_mount_crossing: cli_env
                .filesystem
                .allow_mount_crossing
                .or(file.filesystem.allow_mount_crossing),
        },

        operations: OperationsPartial {
            allow_recursive_delete: cli_env
                .operations
                .allow_recursive_delete
                .or(file.operations.allow_recursive_delete),
            allow_recursive_copy: cli_env
                .operations
                .allow_recursive_copy
                .or(file.operations.allow_recursive_copy),
            allow_chmod: cli_env
                .operations
                .allow_chmod
                .or(file.operations.allow_chmod),
            allow_hard_links: cli_env
                .operations
                .allow_hard_links
                .or(file.operations.allow_hard_links),
            allow_symlink_create: cli_env
                .operations
                .allow_symlink_create
                .or(file.operations.allow_symlink_create),
        },

        limits: LimitsPartial {
            max_request_body_size: cli_env
                .limits
                .max_request_body_size
                .or(file.limits.max_request_body_size),
            max_read_size: cli_env.limits.max_read_size.or(file.limits.max_read_size),
            max_directory_entries: cli_env
                .limits
                .max_directory_entries
                .or(file.limits.max_directory_entries),
            max_concurrent_requests: cli_env
                .limits
                .max_concurrent_requests
                .or(file.limits.max_concurrent_requests),
            max_concurrent_streams: cli_env
                .limits
                .max_concurrent_streams
                .or(file.limits.max_concurrent_streams),
        },

        streaming: StreamingPartial {
            idle_timeout: cli_env
                .streaming
                .idle_timeout
                .or(file.streaming.idle_timeout),
            max_duration: cli_env
                .streaming
                .max_duration
                .or(file.streaming.max_duration),
            heartbeat_interval: cli_env
                .streaming
                .heartbeat_interval
                .or(file.streaming.heartbeat_interval),
        },

        pool: PoolPartial {
            max_workers: cli_env.pool.max_workers.or(file.pool.max_workers),
            idle_timeout: cli_env.pool.idle_timeout.or(file.pool.idle_timeout),
            request_timeout: cli_env.pool.request_timeout.or(file.pool.request_timeout),
        },

        logging: LoggingPartial {
            level: cli_env.logging.level.or(file.logging.level),
            format: cli_env.logging.format.or(file.logging.format),
            redact_paths: cli_env.logging.redact_paths.or(file.logging.redact_paths),
        },

        metrics: MetricsPartial {
            enabled: cli_env.metrics.enabled.or(file.metrics.enabled),
            bind_address: cli_env.metrics.bind_address.or(file.metrics.bind_address),
            port: cli_env.metrics.port.or(file.metrics.port),
        },

        diagnostics: DiagnosticsPartial {
            config_endpoint: ConfigEndpointPartial {
                enabled: cli_env
                    .diagnostics
                    .config_endpoint
                    .enabled
                    .or(file.diagnostics.config_endpoint.enabled),
                bind_address: cli_env
                    .diagnostics
                    .config_endpoint
                    .bind_address
                    .or(file.diagnostics.config_endpoint.bind_address),
            },
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Parse helpers (private)
// ─────────────────────────────────────────────────────────────────────────────

/// Parse a human-readable duration string.
///
/// Accepted formats: `"0"`, `"30s"`, `"5m"`, `"1h"`, `"100ms"`.
fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s == "0" {
        return Ok(Duration::ZERO);
    }
    // Check "ms" before "s" so "100ms" is not mistakenly stripped of its "s".
    if let Some(n) = s.strip_suffix("ms") {
        let v: u64 = n
            .trim()
            .parse()
            .wrap_err_with(|| format!("invalid millisecond count in duration {:?}", s))?;
        return Ok(Duration::from_millis(v));
    }
    if let Some(n) = s.strip_suffix('s') {
        let v: u64 = n
            .trim()
            .parse()
            .wrap_err_with(|| format!("invalid second count in duration {:?}", s))?;
        return Ok(Duration::from_secs(v));
    }
    if let Some(n) = s.strip_suffix('m') {
        let v: u64 = n
            .trim()
            .parse()
            .wrap_err_with(|| format!("invalid minute count in duration {:?}", s))?;
        return Ok(Duration::from_secs(v * 60));
    }
    if let Some(n) = s.strip_suffix('h') {
        let v: u64 = n
            .trim()
            .parse()
            .wrap_err_with(|| format!("invalid hour count in duration {:?}", s))?;
        return Ok(Duration::from_secs(v * 3_600));
    }
    bail!(
        "invalid duration {:?}: expected e.g. \"0\", \"30s\", \"5m\", \"1h\", \"100ms\"",
        s
    )
}

/// Parse a human-readable byte-size string.
///
/// Accepted formats: `"1024"`, `"1024B"`, `"1KiB"`, `"100MiB"`, `"1GiB"`,
/// `"1KB"`, `"100MB"`, `"1GB"`.
fn parse_byte_size(s: &str) -> Result<u64> {
    let s = s.trim();
    // Longest suffixes first so that e.g. "GiB" is matched before "B".
    const SUFFIXES: &[(&str, u64)] = &[
        ("GiB", 1u64 << 30),
        ("MiB", 1u64 << 20),
        ("KiB", 1u64 << 10),
        ("GB", 1_000_000_000u64),
        ("MB", 1_000_000u64),
        ("KB", 1_000u64),
        ("B", 1u64),
    ];
    for &(suffix, factor) in SUFFIXES {
        if let Some(n) = s.strip_suffix(suffix) {
            let v: u64 = n
                .trim()
                .parse()
                .wrap_err_with(|| format!("invalid numeric prefix in byte-size {:?}", s))?;
            return Ok(v * factor);
        }
    }
    // No recognised suffix → treat the whole string as a plain byte count.
    s.parse::<u64>().wrap_err_with(|| {
        format!(
            "invalid byte size {:?}: expected e.g. \"100MiB\", \"1GiB\", \"1024\"",
            s
        )
    })
}

/// Parse a Unix file-mode string as octal.
///
/// Accepted formats: `"0644"`, `"644"`, `"0o644"`.
/// All are interpreted in base-8.
fn parse_unix_mode(s: &str) -> Result<u32> {
    let s = s.trim();
    // Strip the Rust-style "0o" prefix when present; `from_str_radix` then
    // handles any remaining leading zeros as ordinary octal digits.
    let digits = s.strip_prefix("0o").unwrap_or(s);
    u32::from_str_radix(digits, 8)
        .wrap_err_with(|| format!("invalid Unix mode {:?}: expected octal e.g. \"0644\"", s))
}

/// Parse a filesystem root entry in `"id=path"` format.
///
/// Both the `id` and `path` portions must be non-empty.
fn parse_root(s: &str) -> Result<Root> {
    let (id, path) = s
        .split_once('=')
        .ok_or_else(|| eyre::eyre!("invalid root {:?}: expected \"id=path\" format", s))?;
    let id = id.trim().to_string();
    let path = path.trim();
    if id.is_empty() {
        bail!("root id must not be empty in {:?}", s);
    }
    if path.is_empty() {
        bail!("root path must not be empty in {:?}", s);
    }
    Ok(Root {
        id,
        path: PathBuf::from(path),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// resolve / load
// ─────────────────────────────────────────────────────────────────────────────

impl PartialConfig {
    /// Consume the merged partial config, apply defaults, validate constraints,
    /// and produce a fully-typed `Config`.
    fn resolve(self) -> Result<Config> {
        // ── Server ────────────────────────────────────────────────────────────
        let server = ServerConfig {
            bind_address: self
                .server
                .bind_address
                .unwrap_or(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
            port: self.server.port.unwrap_or(8080),
            public_base_url: self.server.public_base_url,
        };

        // ── TLS ───────────────────────────────────────────────────────────────
        let tls_enabled = self.tls.enabled.unwrap_or(false);
        if tls_enabled {
            if self.tls.cert_path.is_none() {
                bail!(
                    "tls.cert_path is required when TLS is enabled \
                     (set --tls-cert-path or NETIXFS_TLS_CERT_PATH)"
                );
            }
            if self.tls.key_path.is_none() {
                bail!(
                    "tls.key_path is required when TLS is enabled \
                     (set --tls-key-path or NETIXFS_TLS_KEY_PATH)"
                );
            }
        }
        let tls = TlsConfig {
            enabled: tls_enabled,
            cert_path: self.tls.cert_path,
            key_path: self.tls.key_path,
        };

        // ── Auth / JWT ────────────────────────────────────────────────────────
        let jp = &self.auth.jwt;
        let mut sources: Vec<JwtKeySource> = Vec::new();
        if let Some(p) = jp.public_key_path.clone() {
            sources.push(JwtKeySource::PublicKeyPath(p));
        }
        if let Some(u) = jp.public_key_url.clone() {
            sources.push(JwtKeySource::PublicKeyUrl(u));
        }
        if let Some(p) = jp.jwks_path.clone() {
            sources.push(JwtKeySource::JwksPath(p));
        }
        if let Some(u) = jp.jwks_url.clone() {
            sources.push(JwtKeySource::JwksUrl(u));
        }
        if sources.len() > 1 {
            bail!(
                "auth.jwt: at most one key source may be configured, \
                 but {} were provided \
                 (public-key-path, public-key-url, jwks-path, jwks-url)",
                sources.len()
            );
        }
        let jwt = JwtConfig {
            source: sources.into_iter().next(),
            issuer: jp.issuer.clone(),
            audience: jp.audience.clone(),
            username_claim: jp
                .username_claim
                .clone()
                .unwrap_or_else(|| "sub".to_string()),
            remote_key_refresh_interval: parse_duration(
                jp.remote_key_refresh_interval.as_deref().unwrap_or("5m"),
            )
            .wrap_err("auth.jwt.remote_key_refresh_interval")?,
        };
        let auth = AuthConfig { jwt };

        // ── Filesystem ────────────────────────────────────────────────────────
        if self.filesystem.allowed_roots.is_empty() {
            bail!("filesystem.allowed_roots: at least one root must be configured");
        }
        let allowed_roots = self
            .filesystem
            .allowed_roots
            .iter()
            .map(|s| parse_root(s))
            .collect::<Result<Vec<_>>>()
            .wrap_err("filesystem.allowed_roots")?;

        let default_file_mode = parse_unix_mode(
            self.filesystem
                .default_file_mode
                .as_deref()
                .unwrap_or("0644"),
        )
        .wrap_err("filesystem.default_file_mode")?;

        let default_dir_mode = parse_unix_mode(
            self.filesystem
                .default_dir_mode
                .as_deref()
                .unwrap_or("0755"),
        )
        .wrap_err("filesystem.default_dir_mode")?;

        let umask = self
            .filesystem
            .umask
            .as_deref()
            .map(parse_unix_mode)
            .transpose()
            .wrap_err("filesystem.umask")?;

        let filesystem = FilesystemConfig {
            allowed_roots,
            read_only: self.filesystem.read_only.unwrap_or(false),
            default_file_mode,
            default_dir_mode,
            umask,
            symlink_policy: self
                .filesystem
                .symlink_policy
                .unwrap_or(SymlinkPolicy::Reject),
            allow_mount_crossing: self.filesystem.allow_mount_crossing.unwrap_or(false),
        };

        // ── Operations ────────────────────────────────────────────────────────
        let operations = OperationsConfig {
            allow_recursive_delete: self.operations.allow_recursive_delete.unwrap_or(true),
            allow_recursive_copy: self.operations.allow_recursive_copy.unwrap_or(true),
            allow_chmod: self.operations.allow_chmod.unwrap_or(true),
            allow_hard_links: self.operations.allow_hard_links.unwrap_or(true),
            allow_symlink_create: self.operations.allow_symlink_create.unwrap_or(true),
        };

        // ── Limits ────────────────────────────────────────────────────────────
        let limits = LimitsConfig {
            max_request_body_size: parse_byte_size(
                self.limits
                    .max_request_body_size
                    .as_deref()
                    .unwrap_or("100MiB"),
            )
            .wrap_err("limits.max_request_body_size")?,
            max_read_size: parse_byte_size(
                self.limits.max_read_size.as_deref().unwrap_or("100MiB"),
            )
            .wrap_err("limits.max_read_size")?,
            max_directory_entries: self.limits.max_directory_entries.unwrap_or(10_000),
            max_concurrent_requests: self.limits.max_concurrent_requests.unwrap_or(1_024),
            max_concurrent_streams: self.limits.max_concurrent_streams.unwrap_or(128),
        };

        // ── Streaming ─────────────────────────────────────────────────────────
        let streaming = StreamingConfig {
            idle_timeout: parse_duration(self.streaming.idle_timeout.as_deref().unwrap_or("5m"))
                .wrap_err("streaming.idle_timeout")?,
            max_duration: parse_duration(self.streaming.max_duration.as_deref().unwrap_or("1h"))
                .wrap_err("streaming.max_duration")?,
            heartbeat_interval: parse_duration(
                self.streaming
                    .heartbeat_interval
                    .as_deref()
                    .unwrap_or("30s"),
            )
            .wrap_err("streaming.heartbeat_interval")?,
        };

        // ── Pool ──────────────────────────────────────────────────────────────
        let pool = PoolConfig {
            max_workers: self.pool.max_workers.unwrap_or(64),
            idle_timeout: parse_duration(self.pool.idle_timeout.as_deref().unwrap_or("5m"))
                .wrap_err("pool.idle_timeout")?,
            request_timeout: parse_duration(self.pool.request_timeout.as_deref().unwrap_or("30s"))
                .wrap_err("pool.request_timeout")?,
        };

        // ── Logging ───────────────────────────────────────────────────────────
        let level = self.logging.level.unwrap_or(LogLevel::Info);
        let logging = LoggingConfig {
            level,
            format: self.logging.format.unwrap_or(LogFormat::Json),
            redact_paths: self.logging.redact_paths.unwrap_or(false),
        };

        // ── Metrics ───────────────────────────────────────────────────────────
        let metrics = MetricsConfig {
            enabled: self.metrics.enabled.unwrap_or(false),
            bind_address: self
                .metrics
                .bind_address
                .unwrap_or(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
            port: self.metrics.port.unwrap_or(9090),
        };

        // ── Diagnostics ───────────────────────────────────────────────────────
        let config_endpoint = ConfigEndpointConfig {
            enabled: self.diagnostics.config_endpoint.enabled.unwrap_or(false),
            bind_address: self
                .diagnostics
                .config_endpoint
                .bind_address
                .unwrap_or_else(|| {
                    "127.0.0.1:8081"
                        .parse()
                        .expect("hard-coded default is a valid SocketAddr")
                }),
        };
        let diagnostics = DiagnosticsConfig { config_endpoint };

        Ok(Config {
            server,
            tls,
            auth,
            filesystem,
            operations,
            limits,
            streaming,
            pool,
            logging,
            metrics,
            diagnostics,
        })
    }
}

impl Config {
    /// Load configuration from (optional) TOML file, environment variables,
    /// and CLI flags, merge them in precedence order (file < env < CLI), and
    /// resolve into a fully-typed `Config`.
    pub(crate) fn load() -> Result<Self> {
        let cli = CliEnvConfig::parse();
        let file_config = {
            let path = &cli.config_file;
            let content = std::fs::read_to_string(path)
                .wrap_err_with(|| format!("failed to read config file {:?}", path))?;
            toml::from_str::<PartialConfig>(&content)
                .wrap_err_with(|| format!("failed to parse config file {:?}", path))?
        };

        merge(cli.config, file_config).resolve()
    }
}
