use crate::config::{FileMode, LogFormat, LogLevel, Root, SymlinkPolicy, ValueSource};
use bon::Builder;
use bytesize::ByteSize;
use clap::{
    Arg, ArgAction, ArgGroup, ArgMatches,
    builder::{BoolishValueParser, ValueParser},
    value_parser,
};
use eyre::{Context, OptionExt, Result};
use lazy_static::lazy_static;
use serde::de::DeserializeOwned;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    path::PathBuf,
    time::Duration,
};
use url::Url;

type GetArgFn<T> = fn(&ArgMatches, &str) -> Result<Option<T>>;

/// Descriptor for a single configuration parameter knob.
#[derive(Debug, Clone, Builder)]
#[builder(derive(Debug))]
pub(crate) struct Parameter<T> {
    #[builder(start_fn)]
    pub(super) id: &'static str,

    pub(super) argument: &'static str,

    pub(super) environment: &'static str,

    pub(super) toml: Option<&'static str>,

    default: T,

    #[builder(default, with=|| true)]
    pub(super) sensitive: bool,

    #[builder(into)]
    arg_value_parser: Option<ValueParser>,

    #[builder(default = ArgAction::Set)]
    arg_action: ArgAction,

    get_arg: Option<GetArgFn<T>>,
}

impl<T> Parameter<T> {
    fn to_arg(&self) -> Arg {
        let mut arg = Arg::new(self.id)
            .long(self.argument.trim_start_matches('-'))
            .env(self.environment)
            .required(false)
            .action(self.arg_action.clone());
        if let Some(ref parser) = self.arg_value_parser {
            arg = arg.value_parser(parser.clone());
        }
        arg
    }
}

impl<T> Parameter<T> {
    pub(super) fn try_resolve_from_args(
        &self,
        arguments: &ArgMatches,
    ) -> Result<Option<(T, ValueSource)>>
    where
        T: Clone + Send + Sync + 'static,
    {
        (self.get_arg.unwrap_or(get_one::<T>))(arguments, self.id)?
            .map(|value| {
                Ok((
                    value.clone(),
                    arguments
                        .value_source(self.id)
                        .ok_or_eyre(format!(
                            "logic error: argument {} has a value but has no source",
                            self.id
                        ))?
                        .into(),
                ))
            })
            .transpose()
    }

    pub(super) fn try_resolve_from_file(
        &self,
        file_config: &toml::Table,
    ) -> Result<Option<(T, ValueSource)>>
    where
        T: DeserializeOwned,
    {
        self.toml
            .and_then(|key| file_config.get(key))
            .map(|value| {
                value.clone().try_into().wrap_err_with(|| {
                    format!(
                        "failed to convert value of {}",
                        self.toml.unwrap_or("<undefined>")
                    )
                })
            })
            .transpose()
    }

    pub(super) fn resolve_to_default(&self) -> (T, ValueSource)
    where
        T: Clone,
    {
        (self.default.clone(), ValueSource::Default)
    }
}

impl<T> From<&Parameter<T>> for Arg {
    fn from(param: &Parameter<T>) -> Self {
        param.to_arg()
    }
}

fn get_one<T>(args: &ArgMatches, id: &str) -> Result<Option<T>>
where
    T: Clone + Send + Sync + 'static,
{
    Ok(args.try_get_one::<T>(id)?.cloned())
}

lazy_static! {
    // ── General ──────────────────────────────────────────────────────────────────────────
    pub(super) static ref CONFIG_FILE: Parameter<Option<PathBuf>> =
        Parameter::builder("config_file")
            .argument("--config-file")
            .environment("NETIXFS_CONFIG_FILE")
            .default(None)
            .arg_value_parser(ValueParser::path_buf())
            .build();

    // ── Server ───────────────────────────────────────────────────────────────────────────
    pub(super) static ref SERVER_BIND_ADDRESS: Parameter<IpAddr> =
        Parameter::builder("server.bind_address")
            .argument("--bind-address")
            .environment("NETIXFS_SERVER_BIND_ADDRESS")
            .toml("server.bind_address")
            .default(IpAddr::V4(Ipv4Addr::LOCALHOST))
            .arg_value_parser(value_parser!(IpAddr))
            .build();
    pub(super) static ref SERVER_PORT: Parameter<u16> =
        Parameter::builder("server.port")
            .argument("--port")
            .environment("NETIXFS_SERVER_PORT")
            .toml("server.port")
            .default(8080)
            .arg_value_parser(value_parser!(u16))
            .build();

    pub(super) static ref SERVER_PUBLIC_BASE_URL: Parameter<Option<Url>> =
        Parameter::builder("server.public_base_url")
            .argument("--public-base-url")
            .environment("NETIXFS_SERVER_PUBLIC_BASE_URL")
            .toml("server.public_base_url")
            .default(None)
            .arg_value_parser(value_parser!(Url))
            .build();

    // ── TLS ──────────────────────────────────────────────────────────────────────────────
    pub(super) static ref TLS_ENABLED: Parameter<bool> =
        Parameter::builder("tls.enabled")
            .argument("--tls-enabled")
            .environment("NETIXFS_TLS_ENABLED")
            .toml("tls.enabled")
            .default(false)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    pub(super) static ref TLS_CERT_PATH: Parameter<Option<PathBuf>> =
        Parameter::builder("tls.cert_path")
            .argument("--tls-cert-path")
            .environment("NETIXFS_TLS_CERT_PATH")
            .toml("tls.cert_path")
            .default(None)
            .arg_value_parser(value_parser!(PathBuf))
            .build();

    pub(super) static ref TLS_KEY_PATH: Parameter<Option<PathBuf>> =
        Parameter::builder("tls.key_path")
            .argument("--tls-key-path")
            .environment("NETIXFS_TLS_KEY_PATH")
            .toml("tls.key_path")
            .default(None)
            .sensitive()
            .arg_value_parser(value_parser!(PathBuf))
            .build();

    // ── Authentication ───────────────────────────────────────────────────────────────────
    pub(super) static ref AUTH_JWT_PUBLIC_KEY_PATH: Parameter<Option<PathBuf>> =
        Parameter::builder("auth.jwt.public_key_path")
            .argument("--jwt-public-key-path")
            .environment("NETIXFS_AUTH_JWT_PUBLIC_KEY_PATH")
            .toml("auth.jwt.public_key_path")
            .default(None)
            .arg_value_parser(value_parser!(PathBuf))
            .build();

    pub(super) static ref AUTH_JWT_PUBLIC_KEY_URL: Parameter<Option<Url>> =
        Parameter::builder("auth.jwt.public_key_url")
            .argument("--jwt-public-key-url")
            .environment("NETIXFS_AUTH_JWT_PUBLIC_KEY_URL")
            .toml("auth.jwt.public_key_url")
            .default(None)
            .sensitive() // secret-bearing URL: may contain embedded credentials
            .arg_value_parser(value_parser!(Url))
            .build();

    pub(super) static ref AUTH_JWT_JWKS_PATH: Parameter<Option<PathBuf>> =
        Parameter::builder("auth.jwt.jwks_path")
            .argument("--jwt-jwks-path")
            .environment("NETIXFS_AUTH_JWT_JWKS_PATH")
            .toml("auth.jwt.jwks_path")
            .default(None)
            .arg_value_parser(value_parser!(PathBuf))
            .build();

    pub(super) static ref AUTH_JWT_JWKS_URL: Parameter<Option<Url>> =
        Parameter::builder("auth.jwt.jwks_url")
            .argument("--jwt-jwks-url")
            .environment("NETIXFS_AUTH_JWT_JWKS_URL")
            .toml("auth.jwt.jwks_url")
            .default(None)
            .sensitive() // secret-bearing URL: may contain embedded credentials
            .arg_value_parser(value_parser!(PathBuf))
            .build();

    pub(super) static ref AUTH_JWT_ISSUER: Parameter<Option<String>> =
        Parameter::builder("auth.jwt.issuer")
            .argument("--jwt-issuer")
            .environment("NETIXFS_AUTH_JWT_ISSUER")
            .toml("auth.jwt.issuer")
            .default(None)
            .arg_value_parser(value_parser!(String))
            .build();

    pub(super) static ref AUTH_JWT_AUDIENCE: Parameter<Option<String>> =
        Parameter::builder("auth.jwt.audience")
            .argument("--jwt-audience")
            .environment("NETIXFS_AUTH_JWT_AUDIENCE")
            .toml("auth.jwt.audience")
            .default(None)
            .arg_value_parser(value_parser!(String))
            .build();

    pub(super) static ref AUTH_JWT_USERNAME_CLAIM: Parameter<String> =
        Parameter::builder("auth.jwt.username_claim")
            .argument("--jwt-username-claim")
            .environment("NETIXFS_AUTH_JWT_USERNAME_CLAIM")
            .toml("auth.jwt.username_claim")
            .default("sub".to_owned())
            .arg_value_parser(value_parser!(String))
            .build();

    pub(super) static ref AUTH_JWT_REMOTE_KEY_REFRESH_INTERVAL: Parameter<Duration> =
        Parameter::builder("auth.jwt.remote_key_refresh_interval")
            .argument("--jwt-remote-key-refresh-interval")
            .environment("NETIXFS_AUTH_JWT_REMOTE_KEY_REFRESH_INTERVAL")
            .toml("auth.jwt.remote_key_refresh_interval")
            .default(Duration::from_mins(5))
            .arg_value_parser(ValueParser::new(humantime::parse_duration))
            .build();

    // ── Filesystem ───────────────────────────────────────────────────────────────────────
    pub(super) static ref FILESYSTEM_ALLOWED_ROOTS: Parameter<Vec<Root>> =
        Parameter::builder("filesystem.allowed_roots")
            .argument("--allowed-root")
            .environment("NETIXFS_FILESYSTEM_ALLOWED_ROOTS")
            .toml("filesystem.allowed_roots")
            .default(Vec::new())
            .arg_value_parser(vec![ValueParser::from(value_parser!(Root))])
            .arg_action(ArgAction::Append)
            .build();

    pub(super) static ref FILESYSTEM_READ_ONLY: Parameter<bool> =
        Parameter::builder("filesystem.read_only")
            .argument("--read-only")
            .environment("NETIXFS_FILESYSTEM_READ_ONLY")
            .toml("filesystem.read_only")
            .default(false)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    pub(super) static ref FILESYSTEM_DEFAULT_FILE_MODE: Parameter<FileMode> =
        Parameter::builder("filesystem.default_file_mode")
            .argument("--default-file-mode")
            .environment("NETIXFS_FILESYSTEM_DEFAULT_FILE_MODE")
            .toml("filesystem.default_file_mode")
            .default(FileMode(0o0644))
            .arg_value_parser(value_parser!(FileMode))
            .build();

    pub(super) static ref FILESYSTEM_DEFAULT_DIR_MODE: Parameter<FileMode> =
        Parameter::builder("filesystem.default_dir_mode")
            .argument("--default-dir-mode")
            .environment("NETIXFS_FILESYSTEM_DEFAULT_DIR_MODE")
            .toml("filesystem.default_dir_mode")
            .default(FileMode(0o0755))
            .arg_value_parser(value_parser!(FileMode))
            .build();

    pub(super) static ref FILESYSTEM_UMASK: Parameter<Option<FileMode>> =
        Parameter::builder("filesystem.umask")
            .argument("--umask")
            .environment("NETIXFS_FILESYSTEM_UMASK")
            .toml("filesystem.umask")
            .default(None) // inherits process umask at runtime
            .arg_value_parser(value_parser!(FileMode))
            .build();

    pub(super) static ref FILESYSTEM_SYMLINK_POLICY: Parameter<SymlinkPolicy> =
        Parameter::builder("filesystem.symlink_policy")
            .argument("--symlink-policy")
            .environment("NETIXFS_FILESYSTEM_SYMLINK_POLICY")
            .toml("filesystem.symlink_policy")
            .default(SymlinkPolicy::Reject)
            .arg_value_parser(value_parser!(SymlinkPolicy))
            .build();

    pub(super) static ref FILESYSTEM_ALLOW_MOUNT_CROSSING: Parameter<bool> =
        Parameter::builder("filesystem.allow_mount_crossing")
            .argument("--allow-mount-crossing")
            .environment("NETIXFS_FILESYSTEM_ALLOW_MOUNT_CROSSING")
            .toml("filesystem.allow_mount_crossing")
            .default(false)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    // ── Operations ───────────────────────────────────────────────────────────────────────
    pub(super) static ref OPERATIONS_ALLOW_RECURSIVE_DELETE: Parameter<bool> =
        Parameter::builder("operations.allow_recursive_delete")
            .argument("--allow-recursive-delete")
            .environment("NETIXFS_OPERATIONS_ALLOW_RECURSIVE_DELETE")
            .toml("operations.allow_recursive_delete")
            .default(true) .arg_value_parser(BoolishValueParser::new())
            .build();

    pub(super) static ref OPERATIONS_ALLOW_RECURSIVE_COPY: Parameter<bool> =
        Parameter::builder("operations.allow_recursive_copy")
            .argument("--allow-recursive-copy")
            .environment("NETIXFS_OPERATIONS_ALLOW_RECURSIVE_COPY")
            .toml("operations.allow_recursive_copy")
            .default(true)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    pub(super) static ref OPERATIONS_ALLOW_CHMOD: Parameter<bool> =
        Parameter::builder("operations.allow_chmod")
            .argument("--allow-chmod")
            .environment("NETIXFS_OPERATIONS_ALLOW_CHMOD")
            .toml("operations.allow_chmod")
            .default(true)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    pub(super) static ref OPERATIONS_ALLOW_HARD_LINKS: Parameter<bool> =
        Parameter::builder("operations.allow_hard_links")
            .argument("--allow-hard-links")
            .environment("NETIXFS_OPERATIONS_ALLOW_HARD_LINKS")
            .toml("operations.allow_hard_links")
            .default(true)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    pub(super) static ref OPERATIONS_ALLOW_SYMLINK_CREATE: Parameter<bool> =
        Parameter::builder("operations.allow_symlink_create")
            .argument("--allow-symlink-create")
            .environment("NETIXFS_OPERATIONS_ALLOW_SYMLINK_CREATE")
            .toml("operations.allow_symlink_create")
            .default(true)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    // ── Limits ───────────────────────────────────────────────────────────────────────────
    pub(super) static ref LIMITS_MAX_REQUEST_BODY_SIZE: Parameter<ByteSize> =
        Parameter::builder("limits.max_request_body_size")
            .argument("--max-request-body-size")
            .environment("NETIXFS_LIMITS_MAX_REQUEST_BODY_SIZE")
            .toml("limits.max_request_body_size")
            .default(ByteSize::mib(100))
            .arg_value_parser(value_parser!(ByteSize))
            .build();
    pub(super) static ref LIMITS_MAX_READ_SIZE: Parameter<ByteSize> =
        Parameter::builder("limits.max_read_size")
            .argument("--max-read-size")
            .environment("NETIXFS_LIMITS_MAX_READ_SIZE")
            .toml("limits.max_read_size")
            .default(ByteSize::mib(100))
            .arg_value_parser(value_parser!(ByteSize))
            .build();
    pub(super) static ref LIMITS_MAX_DIRECTORY_ENTRIES: Parameter<u32> =
        Parameter::builder("limits.max_directory_entries")
            .argument("--max-directory-entries")
            .environment("NETIXFS_LIMITS_MAX_DIRECTORY_ENTRIES")
            .toml("limits.max_directory_entries")
            .default(10000)
            .arg_value_parser(value_parser!(u32))
            .build();
    pub(super) static ref LIMITS_MAX_CONCURRENT_REQUESTS: Parameter<u32> =
        Parameter::builder("limits.max_concurrent_requests")
            .argument("--max-concurrent-requests")
            .environment("NETIXFS_LIMITS_MAX_CONCURRENT_REQUESTS")
            .toml("limits.max_concurrent_requests")
            .default(1024)
            .arg_value_parser(value_parser!(u32))
            .build();
    pub(super) static ref LIMITS_MAX_CONCURRENT_STREAMS: Parameter<u32> =
        Parameter::builder("limits.max_concurrent_streams")
            .argument("--max-concurrent-streams")
            .environment("NETIXFS_LIMITS_MAX_CONCURRENT_STREAMS")
            .toml("limits.max_concurrent_streams")
            .default(128)
            .arg_value_parser(value_parser!(u32))
            .build();

    // ── Streaming ────────────────────────────────────────────────────────────────────────
    pub(super) static ref STREAMING_IDLE_TIMEOUT: Parameter<Duration> =
        Parameter::builder("streaming.idle_timeout")
            .argument("--stream-idle-timeout")
            .environment("NETIXFS_STREAMING_IDLE_TIMEOUT")
            .toml("streaming.idle_timeout")
            .default(Duration::from_mins(5))
            .arg_value_parser(ValueParser::new(humantime::parse_duration))
            .build();
    pub(super) static ref STREAMING_MAX_DURATION: Parameter<Duration> =
        Parameter::builder("streaming.max_duration")
            .argument("--stream-max-duration")
            .environment("NETIXFS_STREAMING_MAX_DURATION")
            .toml("streaming.max_duration")
            .default(Duration::from_hours(1))
            .arg_value_parser(ValueParser::new(humantime::parse_duration))
            .build();
    pub(super) static ref STREAMING_HEARTBEAT_INTERVAL: Parameter<Duration> =
        Parameter::builder("streaming.heartbeat_interval")
            .argument("--stream-heartbeat-interval")
            .environment("NETIXFS_STREAMING_HEARTBEAT_INTERVAL")
            .toml("streaming.heartbeat_interval")
            .default(Duration::from_secs(30))
            .arg_value_parser(ValueParser::new(humantime::parse_duration))
            .build();

    // ── Worker Pool ──────────────────────────────────────────────────────────────────────
    pub(super) static ref POOL_MAX_WORKERS: Parameter<u32> =
        Parameter::builder("pool.max_workers")
            .argument("--pool-max-workers")
            .environment("NETIXFS_POOL_MAX_WORKERS")
            .toml("pool.max_workers")
            .default(64)
            .arg_value_parser(value_parser!(u32))
            .build();
    pub(super) static ref POOL_IDLE_TIMEOUT: Parameter<Duration> =
        Parameter::builder("pool.idle_timeout")
            .argument("--pool-idle-timeout")
            .environment("NETIXFS_POOL_IDLE_TIMEOUT")
            .toml("pool.idle_timeout")
            .default(Duration::from_mins(5))
            .arg_value_parser(ValueParser::new(humantime::parse_duration))
            .build();
    pub(super) static ref POOL_REQUEST_TIMEOUT: Parameter<Duration> =
        Parameter::builder("pool.request_timeout")
            .argument("--pool-request-timeout")
            .environment("NETIXFS_POOL_REQUEST_TIMEOUT")
            .toml("pool.request_timeout")
            .default(Duration::from_secs(30))
            .arg_value_parser(ValueParser::new(humantime::parse_duration))
            .build();

    // ── Logging ──────────────────────────────────────────────────────────────────────────
    pub(super) static ref LOGGING_LEVEL: Parameter<LogLevel> =
        Parameter::builder("logging.level")
            .argument("--log-level")
            .environment("NETIXFS_LOGGING_LEVEL")
            .toml("logging.level")
            .default(LogLevel::Info)
            .arg_value_parser(value_parser!(LogLevel))
            .build();

    pub(super) static ref LOGGING_FORMAT: Parameter<LogFormat> =
        Parameter::builder("logging.format")
            .argument("--log-format")
            .environment("NETIXFS_LOGGING_FORMAT")
            .toml("logging.format")
            .default(LogFormat::Json)
            .arg_value_parser(value_parser!(LogFormat))
            .build();

    pub(super) static ref LOGGING_REDACT_PATHS: Parameter<bool> =
        Parameter::builder("logging.redact_paths")
            .argument("--log-redact-paths")
            .environment("NETIXFS_LOGGING_REDACT_PATHS")
            .toml("logging.redact_paths")
            .default(false)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    // ── Metrics ──────────────────────────────────────────────────────────────────────────
    pub(super) static ref METRICS_ENABLED: Parameter<bool> =
        Parameter::builder("metrics.enabled")
            .argument("--metrics-enabled")
            .environment("NETIXFS_METRICS_ENABLED")
            .toml("metrics.enabled")
            .default(false)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    pub(super) static ref METRICS_BIND_ADDRESS: Parameter<IpAddr> =
        Parameter::builder("metrics.bind_address")
            .argument("--metrics-bind-address")
            .environment("NETIXFS_METRICS_BIND_ADDRESS")
            .toml("metrics.bind_address")
            .default(IpAddr::V4(Ipv4Addr::LOCALHOST))
            .arg_value_parser(value_parser!(IpAddr))
            .build();

    pub(super) static ref METRICS_PORT: Parameter<u16> =
        Parameter::builder("metrics.port")
            .argument("--metrics-port")
            .environment("NETIXFS_METRICS_PORT")
            .toml("metrics.port")
            .default(9090)
            .arg_value_parser(value_parser!(u16))
            .build();

    // ── Diagnostics ───────────────────────────────────────────────────────────────────────
    pub(super) static ref DIAGNOSTICS_CONFIG_ENDPOINT_ENABLED: Parameter<bool> =
        Parameter::builder("diagnostics.config_endpoint.enabled")
            .argument("--config-endpoint-enabled")
            .environment("NETIXFS_DIAGNOSTICS_CONFIG_ENDPOINT_ENABLED")
            .toml("diagnostics.config_endpoint.enabled")
            .default(false)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    pub(super) static ref DIAGNOSTICS_CONFIG_ENDPOINT_BIND_ADDRESS: Parameter<SocketAddr> =
        Parameter::builder("diagnostics.config_endpoint.bind_address")
            .argument("--config-endpoint-bind-address")
            .environment("NETIXFS_DIAGNOSTICS_CONFIG_ENDPOINT_BIND_ADDRESS")
            .toml("diagnostics.config_endpoint.bind_address")
            .default(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 8081)))
            .arg_value_parser(value_parser!(SocketAddr))
            .build();
}

pub(super) fn arguments() -> impl IntoIterator<Item = Arg> {
    [
        CONFIG_FILE.to_arg(),
        // ── Server ───────────────────────────────────────────────────────────────
        SERVER_BIND_ADDRESS.to_arg(),
        SERVER_PORT.to_arg(),
        SERVER_PUBLIC_BASE_URL.to_arg(),
        // ── TLS ──────────────────────────────────────────────────────────────────
        TLS_ENABLED.to_arg(),
        TLS_CERT_PATH.to_arg(),
        TLS_KEY_PATH.to_arg(),
        // ── JWT ──────────────────────────────────────────────────────────────────
        AUTH_JWT_PUBLIC_KEY_PATH.to_arg(),
        AUTH_JWT_PUBLIC_KEY_URL.to_arg(),
        AUTH_JWT_JWKS_PATH.to_arg(),
        AUTH_JWT_JWKS_URL.to_arg(),
        AUTH_JWT_ISSUER.to_arg(),
        AUTH_JWT_AUDIENCE.to_arg(),
        AUTH_JWT_USERNAME_CLAIM.to_arg(),
        AUTH_JWT_REMOTE_KEY_REFRESH_INTERVAL.to_arg(),
        // ── Filesystem ───────────────────────────────────────────────────────────
        FILESYSTEM_ALLOWED_ROOTS.to_arg(),
        FILESYSTEM_READ_ONLY.to_arg(),
        FILESYSTEM_DEFAULT_FILE_MODE.to_arg(),
        FILESYSTEM_DEFAULT_DIR_MODE.to_arg(),
        FILESYSTEM_UMASK.to_arg(),
        FILESYSTEM_SYMLINK_POLICY.to_arg(),
        FILESYSTEM_ALLOW_MOUNT_CROSSING.to_arg(),
        // ── Operations ───────────────────────────────────────────────────────────
        OPERATIONS_ALLOW_RECURSIVE_DELETE.to_arg(),
        OPERATIONS_ALLOW_RECURSIVE_COPY.to_arg(),
        OPERATIONS_ALLOW_CHMOD.to_arg(),
        OPERATIONS_ALLOW_HARD_LINKS.to_arg(),
        OPERATIONS_ALLOW_SYMLINK_CREATE.to_arg(),
        // ── Limits ───────────────────────────────────────────────────────────────
        LIMITS_MAX_REQUEST_BODY_SIZE.to_arg(),
        LIMITS_MAX_READ_SIZE.to_arg(),
        LIMITS_MAX_DIRECTORY_ENTRIES.to_arg(),
        LIMITS_MAX_CONCURRENT_REQUESTS.to_arg(),
        LIMITS_MAX_CONCURRENT_STREAMS.to_arg(),
        // ── Streaming ────────────────────────────────────────────────────────────
        STREAMING_IDLE_TIMEOUT.to_arg(),
        STREAMING_MAX_DURATION.to_arg(),
        STREAMING_HEARTBEAT_INTERVAL.to_arg(),
        // ── Worker Pool ──────────────────────────────────────────────────────────
        POOL_MAX_WORKERS.to_arg(),
        POOL_IDLE_TIMEOUT.to_arg(),
        POOL_REQUEST_TIMEOUT.to_arg(),
        // ── Logging ──────────────────────────────────────────────────────────────
        LOGGING_LEVEL.to_arg(),
        LOGGING_FORMAT.to_arg(),
        LOGGING_REDACT_PATHS.to_arg(),
        // ── Metrics ──────────────────────────────────────────────────────────────
        METRICS_ENABLED.to_arg(),
        METRICS_BIND_ADDRESS.to_arg(),
        METRICS_PORT.to_arg(),
        // ── Diagnostics ──────────────────────────────────────────────────────────
        DIAGNOSTICS_CONFIG_ENDPOINT_ENABLED.to_arg(),
        DIAGNOSTICS_CONFIG_ENDPOINT_BIND_ADDRESS.to_arg(),
    ]
}

pub(super) fn argument_groups() -> impl IntoIterator<Item = ArgGroup> {
    [ArgGroup::new("jwt_key_sources").args([
        AUTH_JWT_JWKS_URL.id,
        AUTH_JWT_JWKS_PATH.id,
        AUTH_JWT_PUBLIC_KEY_URL.id,
        AUTH_JWT_PUBLIC_KEY_PATH.id,
    ])]
}
