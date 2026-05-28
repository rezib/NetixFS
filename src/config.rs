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

use self::parameters::Parameter;
use bytesize::ByteSize;
use clap::{ArgMatches, ValueEnum, command};
use eyre::{Result, WrapErr, bail, eyre};
use serde::{Deserialize, Serialize, de::DeserializeOwned, ser::SerializeMap};
use serde_with::{DeserializeFromStr, SerializeDisplay};
use std::{
    ffi::OsString,
    fmt::Display,
    fs::Permissions,
    net::{IpAddr, SocketAddr},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};
use url::Url;

mod parameters;

/// Load configuration from (optional) TOML file, environment variables,
/// and CLI flags, merge them in precedence order (file < env < CLI), and
/// resolve into a fully-typed `Config`.
pub(crate) fn load<I>(args: I) -> Result<Config>
where
    I: IntoIterator,
    I::Item: Into<OsString> + Clone,
{
    let arguments = command!()
        .args(parameters::arguments())
        .groups(parameters::argument_groups())
        .get_matches_from(args);
    let file_config = arguments
        .get_one::<&Path>(parameters::CONFIG_FILE.id)
        .map(|path| -> Result<toml::Table> {
            let content = std::fs::read_to_string(path)
                .wrap_err_with(|| format!("failed to read config file {:?}", path))?;
            toml::from_str::<toml::Table>(&content)
                .wrap_err_with(|| format!("failed to parse config file {:?}", path))
        })
        .unwrap_or_else(|| Ok(toml::Table::default()))?;
    Config::resolve(Resolver {
        arguments: &arguments,
        file_config: &file_config,
    })
}

// ── Config structs ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Config {
    pub config_file: Value<Option<PathBuf>>,
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

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ServerConfig {
    pub bind_address: Value<IpAddr>,
    pub port: Value<u16>,
    pub public_base_url: Value<Option<Url>>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TlsConfig {
    enabled: Value<bool>,
    cert_path: Value<Option<PathBuf>>,
    key_path: Value<Option<PathBuf>>,
}

impl TlsConfig {
    fn resolve(resolver: Resolver<'_>) -> Result<Self> {
        let enabled = resolver.resolve(&parameters::TLS_ENABLED)?;
        let cert_path = resolver.resolve(&parameters::TLS_CERT_PATH)?;
        let key_path = resolver.resolve(&parameters::TLS_KEY_PATH)?;
        if enabled.value {
            if cert_path.value.is_none() {
                let param = &parameters::TLS_CERT_PATH;
                bail!(
                    "{:?} is required when TLS is enabled (set from command line argument {:?} or from environment variable {:?})",
                    param.id,
                    param.argument,
                    param.environment
                );
            }
            if key_path.value.is_none() {
                let param = &parameters::TLS_KEY_PATH;
                bail!(
                    "\"{}\" is required when TLS is enabled (set from command line argument {:?} or from environment variable {:?})",
                    param.id,
                    param.argument,
                    param.environment
                );
            }
        }
        Ok(TlsConfig {
            enabled,
            cert_path,
            key_path,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AuthConfig {
    pub jwt: JwtConfig,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct JwtConfig {
    pub public_key_path: Value<Option<PathBuf>>,
    pub public_key_url: Value<Option<Url>>,
    pub jwks_path: Value<Option<PathBuf>>,
    pub jwks_url: Value<Option<Url>>,
    pub issuer: Value<Option<String>>,
    pub audience: Value<Option<String>>,
    pub username_claim: Value<String>,
    pub remote_key_refresh_interval: Value<Duration>,
}

impl JwtConfig {
    fn resolve(resolver: Resolver<'_>) -> Result<Self> {
        // TODO: Check that jwt sources are exclusive when they come from the same config value source.
        Ok(Self {
            public_key_path: resolver.resolve(&parameters::AUTH_JWT_PUBLIC_KEY_PATH)?,
            public_key_url: resolver.resolve(&parameters::AUTH_JWT_PUBLIC_KEY_URL)?,
            jwks_path: resolver.resolve(&parameters::AUTH_JWT_JWKS_PATH)?,
            jwks_url: resolver.resolve(&parameters::AUTH_JWT_JWKS_URL)?,
            issuer: resolver.resolve(&parameters::AUTH_JWT_ISSUER)?,
            audience: resolver.resolve(&parameters::AUTH_JWT_AUDIENCE)?,
            username_claim: resolver.resolve(&parameters::AUTH_JWT_USERNAME_CLAIM)?,
            remote_key_refresh_interval: resolver
                .resolve(&parameters::AUTH_JWT_REMOTE_KEY_REFRESH_INTERVAL)?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FilesystemConfig {
    pub allowed_roots: Value<Vec<Root>>,
    pub read_only: Value<bool>,
    pub default_file_mode: Value<FileMode>,
    pub default_dir_mode: Value<FileMode>,
    pub umask: Value<Option<FileMode>>,
    pub symlink_policy: Value<SymlinkPolicy>,
    pub allow_mount_crossing: Value<bool>,
}

impl FilesystemConfig {
    fn resolve(resolver: Resolver<'_>) -> Result<Self> {
        let allowed_roots = resolver.resolve(&parameters::FILESYSTEM_ALLOWED_ROOTS)?;
        if allowed_roots.value.is_empty() {
            let param = &parameters::FILESYSTEM_ALLOWED_ROOTS;
            bail!(
                "at least one root must be configured in {:?} (set from command line argument {:?} or from environment variable {:?})",
                param.id,
                param.argument,
                param.environment
            );
        }
        Ok(Self {
            allowed_roots,
            read_only: resolver.resolve(&parameters::FILESYSTEM_READ_ONLY)?,
            default_file_mode: resolver.resolve(&parameters::FILESYSTEM_DEFAULT_FILE_MODE)?,
            default_dir_mode: resolver.resolve(&parameters::FILESYSTEM_DEFAULT_DIR_MODE)?,
            umask: resolver.resolve(&parameters::FILESYSTEM_UMASK)?,
            symlink_policy: resolver.resolve(&parameters::FILESYSTEM_SYMLINK_POLICY)?,
            allow_mount_crossing: resolver.resolve(&parameters::FILESYSTEM_ALLOW_MOUNT_CROSSING)?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct OperationsConfig {
    pub allow_recursive_delete: Value<bool>,
    pub allow_recursive_copy: Value<bool>,
    pub allow_chmod: Value<bool>,
    pub allow_hard_links: Value<bool>,
    pub allow_symlink_create: Value<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LimitsConfig {
    pub max_request_body_size: Value<ByteSize>,
    pub max_read_size: Value<ByteSize>,
    pub max_directory_entries: Value<u32>,
    pub max_concurrent_requests: Value<u32>,
    pub max_concurrent_streams: Value<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StreamingConfig {
    pub idle_timeout: Value<Duration>,
    pub max_duration: Value<Duration>,
    pub heartbeat_interval: Value<Duration>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PoolConfig {
    pub max_workers: Value<u32>,
    pub idle_timeout: Value<Duration>,
    pub request_timeout: Value<Duration>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LoggingConfig {
    pub level: Value<LogLevel>,
    pub format: Value<LogFormat>,
    pub redact_paths: Value<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MetricsConfig {
    pub enabled: Value<bool>,
    pub bind_address: Value<IpAddr>,
    pub port: Value<u16>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConfigEndpointConfig {
    pub enabled: Value<bool>,
    pub bind_address: Value<SocketAddr>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DiagnosticsConfig {
    pub config_endpoint: ConfigEndpointConfig,
}

#[derive(Debug, Clone, DeserializeFromStr, Serialize)]
pub(crate) struct Root {
    pub id: String,
    pub path: PathBuf,
}

impl FromStr for Root {
    type Err = eyre::Error;

    /// Parse a filesystem root entry in `"id=path"` format.
    ///
    /// Both the `id` and `path` portions must be non-empty.
    fn from_str(s: &str) -> Result<Root> {
        let (id, path) = s
            .split_once('=')
            .ok_or_else(|| eyre!("invalid root {:?}: expected \"id=path\" format", s))?;
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
}

#[derive(
    Default,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    SerializeDisplay,
    DeserializeFromStr,
)]
pub struct FileMode(u32);

impl From<FileMode> for Permissions {
    fn from(mode: FileMode) -> Self {
        Permissions::from_mode(mode.0)
    }
}

impl FileMode {
    pub fn value(self) -> u32 {
        self.0
    }
}

impl FromStr for FileMode {
    type Err = eyre::Error;

    /// Parse a Unix file-mode string as octal.
    ///
    /// Accepted formats: `"0644"`, `"644"`, `"0o644"`.
    /// All are interpreted in base-8.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let s = s.trim();
        // Strip the Rust-style "0o" prefix when present; `from_str_radix` then
        // handles any remaining leading zeros as ordinary octal digits.
        let digits = s.strip_prefix("0o").unwrap_or(s);
        u32::from_str_radix(digits, 8)
            .map(Self)
            .wrap_err_with(|| format!("invalid Unix mode {:?}: expected octal e.g. \"0644\"", s))
    }
}

impl Display for FileMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:o}", self.0)
    }
}

/// Log verbosity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ValueEnum)]
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

impl From<LogLevel> for tracing::level_filters::LevelFilter {
    fn from(level: LogLevel) -> Self {
        Self::from_level(level.into())
    }
}

/// Log output format.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, ValueEnum,
)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LogFormat {
    Json,
    Pretty,
    Compact,
}

/// How symbolic links are handled.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, ValueEnum,
)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SymlinkPolicy {
    Reject,
    FollowSafe,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Value<T> {
    pub value: T,
    pub source: ValueSource,
    pub toml: Option<&'static str>,
    pub argument: &'static str,
    pub environment: &'static str,
    pub sensitive: bool,
}

impl<T> Serialize for Value<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut serializer = serializer.serialize_map(None)?;
        const VALUE_FIELD_NAME: &str = "value";
        if self.sensitive {
            serializer.serialize_entry(VALUE_FIELD_NAME, "********")?;
        } else {
            serializer.serialize_entry(VALUE_FIELD_NAME, &self.value)?;
        }
        serializer.serialize_entry("source", &self.source)?;
        serializer.serialize_entry("toml", &self.toml)?;
        serializer.serialize_entry("argument", &self.argument)?;
        serializer.serialize_entry("environment", &self.environment)?;
        serializer.end()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ValueSource {
    Default,
    ConfigFile,
    Environment,
    Argument,
}

impl From<clap::parser::ValueSource> for ValueSource {
    fn from(source: clap::parser::ValueSource) -> Self {
        use clap::parser::ValueSource as Source;
        match source {
            Source::EnvVariable => Self::Environment,
            Source::CommandLine => Self::Argument,
            Source::DefaultValue | _ => Self::Default,
        }
    }
}

impl Config {
    /// Get either from CLI, environment or configuration file, apply defaults, validate constraints, and produce a fully-typed `Config`.
    fn resolve(resolver: Resolver<'_>) -> Result<Self> {
        Ok(Self {
            config_file: resolver.resolve(&parameters::CONFIG_FILE)?,
            server: ServerConfig {
                bind_address: resolver.resolve(&parameters::SERVER_BIND_ADDRESS)?,
                port: resolver.resolve(&parameters::SERVER_PORT)?,
                public_base_url: resolver.resolve(&parameters::SERVER_PUBLIC_BASE_URL)?,
            },
            tls: TlsConfig::resolve(resolver)?,
            auth: AuthConfig {
                jwt: JwtConfig::resolve(resolver)?,
            },
            filesystem: FilesystemConfig::resolve(resolver)?,
            operations: OperationsConfig {
                allow_recursive_delete: resolver
                    .resolve(&parameters::OPERATIONS_ALLOW_RECURSIVE_DELETE)?,
                allow_recursive_copy: resolver
                    .resolve(&parameters::OPERATIONS_ALLOW_RECURSIVE_COPY)?,
                allow_chmod: resolver.resolve(&parameters::OPERATIONS_ALLOW_CHMOD)?,
                allow_hard_links: resolver.resolve(&parameters::OPERATIONS_ALLOW_HARD_LINKS)?,
                allow_symlink_create: resolver
                    .resolve(&parameters::OPERATIONS_ALLOW_SYMLINK_CREATE)?,
            },
            limits: LimitsConfig {
                max_request_body_size: resolver
                    .resolve(&parameters::LIMITS_MAX_REQUEST_BODY_SIZE)?,
                max_read_size: resolver.resolve(&parameters::LIMITS_MAX_READ_SIZE)?,
                max_directory_entries: resolver
                    .resolve(&parameters::LIMITS_MAX_DIRECTORY_ENTRIES)?,
                max_concurrent_requests: resolver
                    .resolve(&parameters::LIMITS_MAX_CONCURRENT_REQUESTS)?,
                max_concurrent_streams: resolver
                    .resolve(&parameters::LIMITS_MAX_CONCURRENT_STREAMS)?,
            },
            streaming: StreamingConfig {
                idle_timeout: resolver.resolve(&parameters::STREAMING_IDLE_TIMEOUT)?,
                max_duration: resolver.resolve(&parameters::STREAMING_MAX_DURATION)?,
                heartbeat_interval: resolver.resolve(&parameters::STREAMING_HEARTBEAT_INTERVAL)?,
            },
            pool: PoolConfig {
                max_workers: resolver.resolve(&parameters::POOL_MAX_WORKERS)?,
                idle_timeout: resolver.resolve(&parameters::POOL_IDLE_TIMEOUT)?,
                request_timeout: resolver.resolve(&parameters::POOL_REQUEST_TIMEOUT)?,
            },
            logging: LoggingConfig {
                level: resolver.resolve(&parameters::LOGGING_LEVEL)?,
                format: resolver.resolve(&parameters::LOGGING_FORMAT)?,
                redact_paths: resolver.resolve(&parameters::LOGGING_REDACT_PATHS)?,
            },
            metrics: MetricsConfig {
                enabled: resolver.resolve(&parameters::METRICS_ENABLED)?,
                bind_address: resolver.resolve(&parameters::METRICS_BIND_ADDRESS)?,
                port: resolver.resolve(&parameters::METRICS_PORT)?,
            },
            diagnostics: DiagnosticsConfig {
                config_endpoint: ConfigEndpointConfig {
                    enabled: resolver.resolve(&parameters::DIAGNOSTICS_CONFIG_ENDPOINT_ENABLED)?,
                    bind_address: resolver
                        .resolve(&parameters::DIAGNOSTICS_CONFIG_ENDPOINT_BIND_ADDRESS)?,
                },
            },
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Resolver<'a> {
    arguments: &'a ArgMatches,
    file_config: &'a toml::Table,
}

impl<'a> Resolver<'a> {
    fn resolve<T>(&self, parameter: &Parameter<T>) -> Result<Value<T>>
    where
        T: DeserializeOwned + Clone + Send + Sync + 'static,
    {
        let (value, source) = parameter
            .try_resolve_from_args(self.arguments)
            .transpose()
            .or_else(|| {
                parameter
                    .try_resolve_from_file(self.file_config)
                    .transpose()
            })
            .transpose()?
            .unwrap_or_else(|| parameter.resolve_to_default());
        Ok(Value {
            value,
            source,
            argument: parameter.argument,
            environment: parameter.environment,
            toml: parameter.toml,
            sensitive: parameter.sensitive,
        })
    }
}
