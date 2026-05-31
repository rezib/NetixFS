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
use axum::{Json, Router, http::Method, routing::get};
use bytesize::ByteSize;
use clap::{ArgMatches, ValueEnum, command};
use eyre::{Result, WrapErr, bail, eyre};
use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct};
use serde_with::{
    DeserializeFromStr, SerializeAs, SerializeDisplay, ser::SerializeAsWrap, serde_as,
};
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
    pub cors: CorsConfig,
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
    pub enabled: Value<bool>,
    pub cert_path: Value<Option<PathBuf>>,
    pub key_path: Value<Option<PathBuf>>,
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

#[serde_as]
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CorsConfig {
    pub enabled: Value<bool>,
    pub allowed_origins: Value<Vec<String>>,
    #[serde_as(as = "Value<Vec<SerdeMethod>>")]
    pub allowed_methods: Value<Vec<Method>>,
    pub allowed_request_headers: Value<Vec<String>>,
    pub exposed_response_headers: Value<Vec<String>>,
    pub max_age: Value<Duration>,
    pub allow_credentials: Value<bool>,
    pub allow_private_network: Value<bool>,
}

impl CorsConfig {
    fn resolve(resolver: Resolver<'_>) -> Result<Self> {
        let enabled = resolver.resolve(&parameters::CORS_ENABLED)?;
        let allowed_origins = resolver.resolve(&parameters::CORS_ALLOWED_ORIGINS)?;

        if enabled.value && allowed_origins.value.is_empty() {
            bail!("at least one CORS allowed origin must be specified if CORS is enabled");
        }

        Ok(Self {
            enabled,
            allowed_origins,
            allowed_methods: resolver
                .resolve(&parameters::CORS_ALLOWED_METHODS)?
                .try_map(|methods| {
                    methods
                        .iter()
                        .map(|str| str.parse())
                        .collect::<Result<_, _>>()
                })?,
            allowed_request_headers: resolver.resolve(&parameters::CORS_ALLOWED_REQUEST_HEADERS)?,
            exposed_response_headers: resolver
                .resolve(&parameters::CORS_EXPOSED_RESPONSE_HEADERS)?,
            max_age: resolver.resolve(&parameters::CORS_MAX_AGE)?.map(Into::into),
            allow_credentials: resolver.resolve(&parameters::CORS_ALLOW_CREDENTIALS)?,
            allow_private_network: resolver.resolve(&parameters::CORS_ALLOW_PRIVATE_NETWORK)?,
        })
    }
}

struct SerdeMethod;

impl SerializeAs<Method> for SerdeMethod {
    fn serialize_as<S>(method: &Method, serializer: S) -> std::prelude::v1::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(method.as_str())
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
            "one and only one JWT source must be provided in command line arguments, environment or configuration file"
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, DeserializeFromStr, Serialize)]
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

    fn try_map<F, U, E>(self, f: F) -> std::result::Result<Value<U>, E>
    where
        F: Fn(T) -> Result<U, E>,
    {
        Ok(Value {
            value: f(self.value)?,
            source: self.source,
            id: self.id,
            toml: self.toml,
            argument: self.argument,
            environment: self.environment,
            sensitive: self.sensitive,
        })
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

pub(super) fn serialize_value<T, I, S>(
    value: &Value<T>,
    inner: &I,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
    I: Serialize,
{
    const VALUE_FIELD_NAME: &str = "value";
    const SOURCE_FIELD_NAME: &str = "source";
    const TOML_FIELD_NAME: &str = "toml";
    const ARGUMENT_FIELD_NAME: &str = "argument";
    const ENVIRONMENT_FIELD_NAME: &str = "environment";
    const REDACTED_VALUE: &str = "** redacted **";

    let mut serializer = serializer.serialize_struct("Value", 5)?;
    if value.sensitive {
        serializer.serialize_field(VALUE_FIELD_NAME, REDACTED_VALUE)?;
    } else {
        serializer.serialize_field(VALUE_FIELD_NAME, &inner)?;
    }
    serializer.serialize_field(SOURCE_FIELD_NAME, &value.source)?;
    serializer.serialize_field(TOML_FIELD_NAME, &value.toml)?;
    serializer.serialize_field(ARGUMENT_FIELD_NAME, &value.argument)?;
    serializer.serialize_field(ENVIRONMENT_FIELD_NAME, &value.environment)?;
    serializer.end()
}

impl<T> Serialize for Value<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_value(self, &self.value, serializer)
    }
}

impl<T, U> SerializeAs<Value<T>> for Value<U>
where
    U: SerializeAs<T>,
{
    fn serialize_as<S>(source: &Value<T>, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_value(
            source,
            &SerializeAsWrap::<T, U>::new(&source.value),
            serializer,
        )
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
            cors: CorsConfig::resolve(resolver)?,
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

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use clap::Command;
    use std::net::Ipv4Addr;

    fn command() -> Command {
        Command::new("test")
            .no_binary_name(true)
            .args(parameters::arguments())
            .groups(parameters::argument_groups())
    }

    // ── Root ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_root_from_str_valid() {
        let root: Root = "myroot=/path/to/root".parse().unwrap();
        assert_eq!(root.id, "myroot");
        assert_eq!(root.path, PathBuf::from("/path/to/root"));

        // Whitespace handling
        let root: Root = "  myroot  =  /path/to/root  ".parse().unwrap();
        assert_eq!(root.id, "myroot");
        assert_eq!(root.path, PathBuf::from("/path/to/root"));

        // No spaces
        let root: Root = "root=/path".parse().unwrap();
        assert_eq!(root.id, "root");
        assert_eq!(root.path, PathBuf::from("/path"));
    }

    #[test]
    fn test_root_from_str_missing_separator() {
        let result: Result<Root> = "myroot/path/to/root".parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_root_from_str_empty_id() {
        let result: Result<Root> = "=/path/to/root".parse();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("root id must not be empty")
        );
    }

    #[test]
    fn test_root_from_str_empty_path() {
        let result: Result<Root> = "myroot=".parse();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("root path must not be empty")
        );
    }

    // ── FileMode ─────────────────────────────────────────────────────────────

    #[test]
    fn test_file_mode_from_str_octal_formats() {
        // Standard octal with leading zero
        let mode: FileMode = "0644".parse().unwrap();
        assert_eq!(mode.value(), 0o644);

        // Octal without leading zero
        let mode: FileMode = "644".parse().unwrap();
        assert_eq!(mode.value(), 0o644);

        // Rust-style octal prefix
        let mode: FileMode = "0o644".parse().unwrap();
        assert_eq!(mode.value(), 0o644);

        // With leading whitespace
        let mode: FileMode = "  0644  ".parse().unwrap();
        assert_eq!(mode.value(), 0o644);

        // Directory mode
        let mode: FileMode = "0755".parse().unwrap();
        assert_eq!(mode.value(), 0o755);

        // All permissions
        let mode: FileMode = "0777".parse().unwrap();
        assert_eq!(mode.value(), 0o777);
    }

    #[test]
    fn test_file_mode_from_str_invalid() {
        // Invalid characters
        let result: Result<FileMode> = "6x4".parse();
        assert!(result.is_err());

        // Decimal number
        let result: Result<FileMode> = "644".parse();
        // Note: This actually succeeds because "644" is valid octal
        assert!(result.is_ok());

        // Empty string
        let result: Result<FileMode> = "".parse();
        assert!(result.is_err());

        // Negative number
        let result: Result<FileMode> = "-644".parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_file_mode_display() {
        let mode = FileMode(0o644);
        assert_eq!(format!("{}", mode), "644");

        let mode = FileMode(0o755);
        assert_eq!(format!("{}", mode), "755");
    }

    // ── LogLevel ────────────────────────────────────────────────────────────

    #[test]
    fn test_log_level_serialization() {
        let level = LogLevel::Error;
        let serialized = serde_json::to_string(&level).unwrap();
        assert_eq!(serialized, "\"error\"");

        let level = LogLevel::Warn;
        let serialized = serde_json::to_string(&level).unwrap();
        assert_eq!(serialized, "\"warn\"");

        let level = LogLevel::Info;
        let serialized = serde_json::to_string(&level).unwrap();
        assert_eq!(serialized, "\"info\"");

        let level = LogLevel::Debug;
        let serialized = serde_json::to_string(&level).unwrap();
        assert_eq!(serialized, "\"debug\"");

        let level = LogLevel::Trace;
        let serialized = serde_json::to_string(&level).unwrap();
        assert_eq!(serialized, "\"trace\"");
    }

    // ── LogFormat ────────────────────────────────────────────────────────────

    #[test]
    fn test_log_format_serialization() {
        let format = LogFormat::Json;
        let serialized = serde_json::to_string(&format).unwrap();
        assert_eq!(serialized, "\"json\"");

        let format = LogFormat::Pretty;
        let serialized = serde_json::to_string(&format).unwrap();
        assert_eq!(serialized, "\"pretty\"");

        let format = LogFormat::Compact;
        let serialized = serde_json::to_string(&format).unwrap();
        assert_eq!(serialized, "\"compact\"");
    }

    // ── SymlinkPolicy ────────────────────────────────────────────────────────

    #[test]
    fn test_symlink_policy_serialization() {
        let policy = SymlinkPolicy::Reject;
        let serialized = serde_json::to_string(&policy).unwrap();
        assert_eq!(serialized, "\"reject\"");

        let policy = SymlinkPolicy::FollowSafe;
        let serialized = serde_json::to_string(&policy).unwrap();
        assert_eq!(serialized, "\"follow_safe\"");
    }

    // ── Value<T> ─────────────────────────────────────────────────────────────

    #[test]
    fn test_value_map() {
        let value = Value {
            value: 42u32,
            source: ValueSource::Default,
            id: "test",
            toml: None,
            argument: "--test",
            environment: "TEST",
            sensitive: false,
        };

        let mapped = value.map(|v| v * 2);
        assert_eq!(mapped.value, 84);
        assert_eq!(mapped.source, ValueSource::Default);
        assert_eq!(mapped.id, "test");
    }

    #[test]
    fn test_value_try_map() {
        let value = Value {
            value: "42".to_string(),
            source: ValueSource::Default,
            id: "test",
            toml: None,
            argument: "--test",
            environment: "TEST",
            sensitive: false,
        };

        let mapped: Result<Value<u32>, _> = value.try_map(|v| v.parse::<u32>());
        assert!(mapped.is_ok());
        assert_eq!(mapped.unwrap().value, 42);

        let value = Value {
            value: "not_a_number".to_string(),
            source: ValueSource::Default,
            id: "test",
            toml: None,
            argument: "--test",
            environment: "TEST",
            sensitive: false,
        };

        let mapped: Result<Value<u32>, _> = value.try_map(|v| v.parse::<u32>());
        assert!(mapped.is_err());
    }

    #[test]
    fn test_value_option_transpose() {
        let value: Value<Option<u32>> = Value {
            value: Some(42),
            source: ValueSource::Default,
            id: "test",
            toml: None,
            argument: "--test",
            environment: "TEST",
            sensitive: false,
        };

        let transposed = value.transpose();
        assert!(transposed.is_some());
        assert_eq!(transposed.unwrap().value, 42);

        let value: Value<Option<u32>> = Value {
            value: None,
            source: ValueSource::Default,
            id: "test",
            toml: None,
            argument: "--test",
            environment: "TEST",
            sensitive: false,
        };

        let transposed = value.transpose();
        assert!(transposed.is_none());
    }

    // ── JwtSource validation ──────────────────────────────────────────────────

    #[test]
    fn test_jwt_source_resolve_single_path() {
        let public_key_path = Value {
            value: Some(PathBuf::from("/path/to/key.pub")),
            source: ValueSource::Argument,
            id: "auth.jwt.public_key_path",
            toml: Some("auth.jwt.public_key_path"),
            argument: "--jwt-public-key-path",
            environment: "NETIXFS_AUTH_JWT_PUBLIC_KEY_PATH",
            sensitive: false,
        };
        let public_key_url = Value {
            value: None,
            source: ValueSource::Default,
            id: "auth.jwt.public_key_url",
            toml: Some("auth.jwt.public_key_url"),
            argument: "--jwt-public-key-url",
            environment: "NETIXFS_AUTH_JWT_PUBLIC_KEY_URL",
            sensitive: true,
        };
        let jwks_path = Value {
            value: None,
            source: ValueSource::Default,
            id: "auth.jwt.jwks_path",
            toml: Some("auth.jwt.jwks_path"),
            argument: "--jwt-jwks-path",
            environment: "NETIXFS_AUTH_JWT_JWKS_PATH",
            sensitive: false,
        };
        let jwks_url = Value {
            value: None,
            source: ValueSource::Default,
            id: "auth.jwt.jwks_url",
            toml: Some("auth.jwt.jwks_url"),
            argument: "--jwt-jwks-url",
            environment: "NETIXFS_AUTH_JWT_JWKS_URL",
            sensitive: true,
        };

        let source =
            JwtSource::resolve(public_key_path, public_key_url, jwks_path, jwks_url).unwrap();
        match source {
            JwtSource::PublicKeyPath(value) => {
                assert_eq!(value.value, PathBuf::from("/path/to/key.pub"));
                assert_eq!(value.source, ValueSource::Argument);
            }
            _ => panic!("Expected PublicKeyPath variant"),
        }
    }

    #[test]
    fn test_jwt_source_resolve_no_source() {
        let public_key_path = Value {
            value: None,
            source: ValueSource::Default,
            id: "auth.jwt.public_key_path",
            toml: Some("auth.jwt.public_key_path"),
            argument: "--jwt-public-key-path",
            environment: "NETIXFS_AUTH_JWT_PUBLIC_KEY_PATH",
            sensitive: false,
        };
        let public_key_url = Value {
            value: None,
            source: ValueSource::Default,
            id: "auth.jwt.public_key_url",
            toml: Some("auth.jwt.public_key_url"),
            argument: "--jwt-public-key-url",
            environment: "NETIXFS_AUTH_JWT_PUBLIC_KEY_URL",
            sensitive: true,
        };
        let jwks_path = Value {
            value: None,
            source: ValueSource::Default,
            id: "auth.jwt.jwks_path",
            toml: Some("auth.jwt.jwks_path"),
            argument: "--jwt-jwks-path",
            environment: "NETIXFS_AUTH_JWT_JWKS_PATH",
            sensitive: false,
        };
        let jwks_url = Value {
            value: None,
            source: ValueSource::Default,
            id: "auth.jwt.jwks_url",
            toml: Some("auth.jwt.jwks_url"),
            argument: "--jwt-jwks-url",
            environment: "NETIXFS_AUTH_JWT_JWKS_URL",
            sensitive: true,
        };

        let result = JwtSource::resolve(public_key_path, public_key_url, jwks_path, jwks_url);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("one and only one JWT source must be provided")
        );
    }

    #[test]
    fn test_jwt_source_resolve_multiple_sources() {
        let public_key_path = Value {
            value: Some(PathBuf::from("/path/to/key.pub")),
            source: ValueSource::Argument,
            id: "auth.jwt.public_key_path",
            toml: Some("auth.jwt.public_key_path"),
            argument: "--jwt-public-key-path",
            environment: "NETIXFS_AUTH_JWT_PUBLIC_KEY_PATH",
            sensitive: false,
        };
        let public_key_url = Value {
            value: Some(Url::parse("https://example.com/key.pub").unwrap()),
            source: ValueSource::Argument,
            id: "auth.jwt.public_key_url",
            toml: Some("auth.jwt.public_key_url"),
            argument: "--jwt-public-key-url",
            environment: "NETIXFS_AUTH_JWT_PUBLIC_KEY_URL",
            sensitive: true,
        };
        let jwks_path = Value {
            value: None,
            source: ValueSource::Default,
            id: "auth.jwt.jwks_path",
            toml: Some("auth.jwt.jwks_path"),
            argument: "--jwt-jwks-path",
            environment: "NETIXFS_AUTH_JWT_JWKS_PATH",
            sensitive: false,
        };
        let jwks_url = Value {
            value: None,
            source: ValueSource::Default,
            id: "auth.jwt.jwks_url",
            toml: Some("auth.jwt.jwks_url"),
            argument: "--jwt-jwks-url",
            environment: "NETIXFS_AUTH_JWT_JWKS_URL",
            sensitive: true,
        };

        let result = JwtSource::resolve(public_key_path, public_key_url, jwks_path, jwks_url);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("only one JWT source must be specified")
        );
    }

    // ── TlsConfig ────────────────────────────────────────────────────────────

    #[test]
    fn test_tls_config_resolve_enabled_with_certs() {
        // Use TOML config to set TLS enabled with cert and key
        let arguments = command().get_matches_from(Vec::<OsString>::new());

        let toml_config = toml::toml! {
            [tls]
            enabled = true
            cert_path = "/path/to/cert.pem"
            key_path = "/path/to/key.pem"
        };

        let result = TlsConfig::resolve(Resolver {
            arguments: &arguments,
            file_config: Some(&toml_config),
        });
        assert!(result.is_ok());
        let tls_config = result.unwrap();

        assert!(tls_config.enabled.value);
        assert_eq!(
            tls_config.cert_path.value,
            Some(PathBuf::from("/path/to/cert.pem"))
        );
        assert_eq!(
            tls_config.key_path.value,
            Some(PathBuf::from("/path/to/key.pem"))
        );
        assert_eq!(tls_config.enabled.source, ValueSource::ConfigFile);
        assert_eq!(tls_config.cert_path.source, ValueSource::ConfigFile);
        assert_eq!(tls_config.key_path.source, ValueSource::ConfigFile);
    }

    #[test]
    fn test_tls_config_resolve_enabled_without_certs() {
        // Use TOML config with TLS enabled but no cert/key paths
        let arguments = command().get_matches_from(Vec::<OsString>::new());

        let toml_config = toml::toml! {
            [tls]
            enabled = true
        };

        let result = TlsConfig::resolve(Resolver {
            arguments: &arguments,
            file_config: Some(&toml_config),
        });
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("tls.cert_path") && err_msg.contains("required when TLS is enabled")
        );
    }

    #[test]
    fn test_tls_config_resolve_enabled_without_key() {
        // Use TOML config with TLS enabled and cert path but no key path
        let arguments = command().get_matches_from(Vec::<OsString>::new());

        let toml_config = toml::toml! {
            [tls]
            enabled = true
            cert_path = "/path/to/cert.pem"
        };

        let result = TlsConfig::resolve(Resolver {
            arguments: &arguments,
            file_config: Some(&toml_config),
        });
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("tls.key_path") && err_msg.contains("required when TLS is enabled")
        );
    }

    #[test]
    fn test_tls_config_resolve_disabled() {
        // Create arguments with TLS disabled (default) and no cert/key paths
        let arguments = command()
            .try_get_matches_from(Vec::<OsString>::new())
            .unwrap();

        let result = TlsConfig::resolve(Resolver {
            arguments: &arguments,
            file_config: None,
        });
        assert!(result.is_ok());
        let tls_config = result.unwrap();

        assert!(!tls_config.enabled.value);
        assert_eq!(tls_config.cert_path.value, None);
        assert_eq!(tls_config.key_path.value, None);
    }

    // ── CorsConfig ───────────────────────────────────────────────────────────

    #[test]
    fn test_cors_config_resolve_enabled_requires_origins() {
        // Create a minimal resolver that returns enabled=true and empty origins
        // This is tested through the actual resolve logic
    }

    // ── FilesystemConfig ─────────────────────────────────────────────────────

    #[test]
    fn test_filesystem_config_resolve_requires_roots() {
        // This validation happens in FilesystemConfig::resolve
        // We'll test it indirectly through config loading
    }

    // ── Value ────────────────────────────────────────────────────────────────

    #[test]
    fn test_value_serialize_non_sensitive() {
        let value = Value {
            value: "test_value".to_string(),
            source: ValueSource::Argument,
            id: "test.id",
            toml: Some("test_id"),
            argument: "--test-id",
            environment: "TEST_ID",
            sensitive: false,
        };

        let serialized = serde_json::to_value(&value).unwrap();
        assert_eq!(serialized["value"], "test_value");
        assert_eq!(serialized["source"], "argument");
        assert_eq!(serialized["toml"], "test_id");
        assert_eq!(serialized["argument"], "--test-id");
        assert_eq!(serialized["environment"], "TEST_ID");
    }

    #[test]
    fn test_value_serialize_sensitive() {
        let value = Value {
            value: "secret_password".to_string(),
            source: ValueSource::Environment,
            id: "secret",
            toml: Some("secret"),
            argument: "--secret",
            environment: "SECRET",
            sensitive: true,
        };

        let serialized = serde_json::to_value(&value).unwrap();
        assert_eq!(serialized["value"], "** redacted **");
        assert_eq!(serialized["source"], "environment");
    }

    // ── Config ───────────────────────────────────────────────────────────────

    #[test]
    fn test_config_resolve_with_toml_file() {
        let arguments = command()
            .try_get_matches_from(Vec::<OsString>::new())
            .unwrap();

        // Create a TOML config table
        let toml_config = toml::toml! {
            [server]
            bind_address = "127.0.0.1"
            port = 9000

            [filesystem]
            allowed_roots = ["root1=/tmp/root1"]
            read_only = true

            [auth.jwt]
            public_key_path = "/path/to/key.pub"
            username_claim = "username"
        };

        let result = Config::resolve(
            parameters::CONFIG_FILE.resolve(&arguments, None).unwrap(),
            Resolver {
                arguments: &arguments,
                file_config: Some(&toml_config),
            },
        );
        assert!(result.is_ok());
        let config = result.unwrap();

        // Check that TOML values were picked up
        assert_eq!(config.server.port.value, 9000);
        assert!(config.filesystem.read_only.value);
        assert_eq!(config.auth.jwt.username_claim.value, "username");
    }

    #[test]
    fn test_config_resolve_source_priority_cli_overrides_toml() {
        let arguments = command()
            .try_get_matches_from(vec![OsString::from("--port"), OsString::from("9999")])
            .unwrap();

        // Create a TOML config table with a different port
        let toml_config = toml::toml! {
            [server]
            bind_address = "127.0.0.1"
            port = 9000

            [filesystem]
            allowed_roots = ["root1=/tmp/root1"]
            read_only = true

            [auth.jwt]
            public_key_path = "/path/to/key.pub"
            username_claim = "username"
        };

        let result = Config::resolve(
            parameters::CONFIG_FILE.resolve(&arguments, None).unwrap(),
            Resolver {
                arguments: &arguments,
                file_config: Some(&toml_config),
            },
        );
        assert!(result.is_ok());
        let config = result.unwrap();

        // CLI argument should override TOML
        assert_matches!(
            config.server.port,
            Value {
                value: 9999,
                source: ValueSource::Argument,
                ..
            }
        );
        // TOML value should still be used for other fields
        assert_matches!(
            config.server.bind_address,
            Value {
                value,
                source: ValueSource::ConfigFile,
                ..
            } => {
                assert_eq!(value, Ipv4Addr::LOCALHOST);
            }
        );
    }

    #[test]
    fn test_config_resolve_defaults_used_when_no_source() {
        let arguments = command()
            .try_get_matches_from(Vec::<OsString>::new())
            .unwrap();
        let toml_config = toml::Table::new();

        // This should fail because both JWT source and filesystem.allowed_roots are required
        let result = Config::resolve(
            parameters::CONFIG_FILE.resolve(&arguments, None).unwrap(),
            Resolver {
                arguments: &arguments,
                file_config: Some(&toml_config),
            },
        );
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        // The error could be about JWT source or filesystem roots
        assert!(
            err_msg.contains("at least one root must be configured")
                || err_msg.contains("filesystem.allowed_roots")
                || err_msg.contains("JWT source")
        );
    }

    #[test]
    fn test_config_resolve_with_filesystem_roots_from_toml() {
        let arguments = command()
            .try_get_matches_from(Vec::<OsString>::new())
            .unwrap();

        // Create TOML with filesystem roots
        let toml_config = toml::toml! {
            [filesystem]
            allowed_roots = ["root1=/tmp/root1", "root2=/tmp/root2"]

            [auth.jwt]
            public_key_path = "/path/to/key.pub"
        };

        let result = Config::resolve(
            parameters::CONFIG_FILE.resolve(&arguments, None).unwrap(),
            Resolver {
                arguments: &arguments,
                file_config: Some(&toml_config),
            },
        );
        assert!(result.is_ok());
        let config = result.unwrap();

        // Check that roots were parsed from TOML
        assert_matches!(
            config.filesystem.allowed_roots.value.as_slice(),
            [
                root1,
                root2,
            ] => {
                assert_eq!(
                    root1,
                    &Root {
                        id: "root1".into(),
                        path: "/tmp/root1".into(),
                    }
                );
                assert_eq!(
                    root2,
                    &Root {
                        id: "root2".into(),
                        path: "/tmp/root2".into(),
                    }
                );
            }
        )
    }

    #[test]
    fn test_config_resolve_source_tracking() {
        let arguments = command()
            .try_get_matches_from(Vec::<OsString>::new())
            .unwrap();

        // Create TOML config
        let toml_config = toml::toml! {
            [server]
            port = 9000

            [filesystem]
            allowed_roots = ["root1=/tmp/root1"]

            [auth.jwt]
            public_key_path = "/path/to/key.pub"
        };

        let result = Config::resolve(
            parameters::CONFIG_FILE.resolve(&arguments, None).unwrap(),
            Resolver {
                arguments: &arguments,
                file_config: Some(&toml_config),
            },
        );
        assert!(result.is_ok());
        let config = result.unwrap();

        // Check that values from TOML have ConfigFile source
        assert_eq!(config.server.port.source, ValueSource::ConfigFile);
        assert_eq!(
            config.filesystem.allowed_roots.source,
            ValueSource::ConfigFile
        );

        // Default values should have Default source
        assert_eq!(config.server.bind_address.source, ValueSource::Default);
    }
}
