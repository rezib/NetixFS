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
use axum::{Json, Router, routing::get};
use bytesize::ByteSize;
use clap::{ArgMatches, ValueEnum, command};
use eyre::{Result, WrapErr, bail, eyre};
use serde::{Deserialize, Serialize, ser::SerializeMap};
use serde_with::{DeserializeFromStr, SerializeDisplay};
use std::{
    ffi::OsString,
    fmt::Display,
    fs::Permissions,
    net::{IpAddr, SocketAddr},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    str::FromStr,
    sync::Arc,
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
    let config_file = parameters::CONFIG_FILE.resolve(&arguments, None)?;
    let file_config = config_file
        .value
        .as_ref()
        .map(|config_file_path| -> Result<toml::Table> {
            let content = std::fs::read_to_string(config_file_path)
                .wrap_err_with(|| format!("failed to read config file {:?}", config_file_path))?;
            toml::from_str::<toml::Table>(&content)
                .wrap_err_with(|| format!("failed to parse config file {:?}", config_file_path))
        })
        .transpose()?;
    Config::resolve(
        config_file,
        Resolver {
            arguments: &arguments,
            file_config: file_config.as_ref(),
        },
    )
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
    #[serde(flatten)]
    pub source: JwtSource,
    pub issuer: Value<Option<String>>,
    pub audience: Value<Option<String>>,
    pub username_claim: Value<String>,
    pub remote_key_refresh_interval: Value<Duration>,
}

impl JwtConfig {
    fn resolve(resolver: Resolver<'_>) -> Result<Self> {
        let public_key_path = resolver.resolve(&parameters::AUTH_JWT_PUBLIC_KEY_PATH)?;
        let public_key_url = resolver.resolve(&parameters::AUTH_JWT_PUBLIC_KEY_URL)?;
        let jwks_path = resolver.resolve(&parameters::AUTH_JWT_JWKS_PATH)?;
        let jwks_url = resolver.resolve(&parameters::AUTH_JWT_JWKS_URL)?;

        Ok(Self {
            source: JwtSource::resolve(public_key_path, public_key_url, jwks_path, jwks_url)?,
            issuer: resolver.resolve(&parameters::AUTH_JWT_ISSUER)?,
            audience: resolver.resolve(&parameters::AUTH_JWT_AUDIENCE)?,
            username_claim: resolver.resolve(&parameters::AUTH_JWT_USERNAME_CLAIM)?,
            remote_key_refresh_interval: resolver
                .resolve(&parameters::AUTH_JWT_REMOTE_KEY_REFRESH_INTERVAL)?
                .map(Into::into),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JwtSource {
    PublicKeyPath(Value<PathBuf>),
    PublicKeyUrl(Value<Url>),
    JwksPath(Value<PathBuf>),
    JwksUrl(Value<Url>),
}

impl JwtSource {
    fn resolve(
        public_key_path: Value<Option<PathBuf>>,
        public_key_url: Value<Option<Url>>,
        jwks_path: Value<Option<PathBuf>>,
        jwks_url: Value<Option<Url>>,
    ) -> Result<Self> {
        #[derive(Copy, Clone)]
        enum Candidate {
            PublicKeyPath(&'static str, ValueSource),
            PublicKeyUrl(&'static str, ValueSource),
            JwksPath(&'static str, ValueSource),
            JwksUrl(&'static str, ValueSource),
        }

        impl Candidate {
            fn value_source(self) -> ValueSource {
                match self {
                    Self::PublicKeyPath(_, value_source) => value_source,
                    Self::PublicKeyUrl(_, value_source) => value_source,
                    Self::JwksPath(_, value_source) => value_source,
                    Self::JwksUrl(_, value_source) => value_source,
                }
            }

            fn id(self) -> &'static str {
                match self {
                    Self::PublicKeyPath(id, _) => id,
                    Self::PublicKeyUrl(id, _) => id,
                    Self::JwksPath(id, _) => id,
                    Self::JwksUrl(id, _) => id,
                }
            }
        }

        let candidates = [
            Candidate::PublicKeyPath(public_key_path.id, public_key_path.source),
            Candidate::PublicKeyUrl(public_key_url.id, public_key_url.source),
            Candidate::JwksPath(jwks_path.id, jwks_path.source),
            Candidate::JwksUrl(jwks_url.id, jwks_url.source),
        ];

        let make_source_no_value_error =
            || eyre!("logic error: JWT source has a source but no value");

        for value_source in [
            ValueSource::Argument,
            ValueSource::Environment,
            ValueSource::ConfigFile,
        ] {
            let mut value_source_candidates = candidates
                .iter()
                .filter(|candidate| candidate.value_source() == value_source);
            if let Some(candidate) = value_source_candidates.next() {
                match value_source_candidates.next() {
                    None => {
                        let source = match candidate {
                            Candidate::PublicKeyPath(_, _) => Self::PublicKeyPath(
                                public_key_path
                                    .transpose()
                                    .ok_or_else(make_source_no_value_error)?,
                            ),
                            Candidate::PublicKeyUrl(_, _) => Self::PublicKeyUrl(
                                public_key_url
                                    .transpose()
                                    .ok_or_else(make_source_no_value_error)?,
                            ),
                            Candidate::JwksPath(_, _) => Self::JwksPath(
                                jwks_path
                                    .transpose()
                                    .ok_or_else(make_source_no_value_error)?,
                            ),
                            Candidate::JwksUrl(_, _) => Self::JwksUrl(
                                jwks_url
                                    .transpose()
                                    .ok_or_else(make_source_no_value_error)?,
                            ),
                        };
                        return Ok(source);
                    }
                    Some(other) => bail!(
                        "only one JWT source must be specified (found {:?} and {:?} in {})",
                        candidate.id(),
                        other.id(),
                        value_source.in_description()
                    ),
                }
            }
        }

        Err(eyre!(
            "one and only JWT source must be provided in command line arguments, environment or configuration file"
        ))
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
    pub id: &'static str,
    pub toml: Option<&'static str>,
    pub argument: &'static str,
    pub environment: &'static str,
    pub sensitive: bool,
}

impl<T> Value<T> {
    fn map<F, U>(self, f: F) -> Value<U>
    where
        F: Fn(T) -> U,
    {
        Value {
            value: f(self.value),
            source: self.source,
            id: self.id,
            toml: self.toml,
            argument: self.argument,
            environment: self.environment,
            sensitive: self.sensitive,
        }
    }
}

impl<T> Value<Option<T>> {
    fn transpose(self) -> Option<Value<T>> {
        self.value.map(|value| Value {
            value,
            source: self.source,
            id: self.id,
            toml: self.toml,
            argument: self.argument,
            environment: self.environment,
            sensitive: self.sensitive,
        })
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ValueSource {
    Default,
    ConfigFile,
    Environment,
    Argument,
}

impl ValueSource {
    fn in_description(self) -> &'static str {
        match self {
            ValueSource::ConfigFile => "configuration file",
            ValueSource::Environment => "environment",
            ValueSource::Argument => "command line arguments",
            ValueSource::Default => "default",
        }
    }
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
    fn resolve(config_file: Value<Option<PathBuf>>, resolver: Resolver<'_>) -> Result<Self> {
        Ok(Self {
            config_file,
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
                idle_timeout: resolver
                    .resolve(&parameters::STREAMING_IDLE_TIMEOUT)?
                    .map(Into::into),
                max_duration: resolver
                    .resolve(&parameters::STREAMING_MAX_DURATION)?
                    .map(Into::into),
                heartbeat_interval: resolver
                    .resolve(&parameters::STREAMING_HEARTBEAT_INTERVAL)?
                    .map(Into::into),
            },
            pool: PoolConfig {
                max_workers: resolver
                    .resolve(&parameters::POOL_MAX_WORKERS)?
                    .map(Into::into),
                idle_timeout: resolver
                    .resolve(&parameters::POOL_IDLE_TIMEOUT)?
                    .map(Into::into),
                request_timeout: resolver
                    .resolve(&parameters::POOL_REQUEST_TIMEOUT)?
                    .map(Into::into),
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
    file_config: Option<&'a toml::Table>,
}

impl<'a> Resolver<'a> {
    fn resolve<T>(&self, parameter: &Parameter<T>) -> Result<Value<T::Output>>
    where
        T: parameters::ValueSeed,
    {
        parameter.resolve(self.arguments, self.file_config)
    }
}

pub(crate) fn service(config: Arc<Config>) -> Router {
    Router::new().route("/configz", get(Json(Arc::clone(&config))))
}
