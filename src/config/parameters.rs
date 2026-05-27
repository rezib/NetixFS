use crate::config::{FileMode, LogFormat, LogLevel, Root, SymlinkPolicy, ValueSource};
use bytesize::ByteSize;
use clap::{Arg, ArgMatches, builder::ValueParser, value_parser};
use eyre::{Context, OptionExt, Result};
use lazy_static::lazy_static;
use serde::de::DeserializeOwned;
use std::{
    marker::PhantomData,
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    path::PathBuf,
    time::Duration,
};
use url::Url;

#[derive(Debug)]
pub(super) struct Default<T>(T);

impl<T> Default<T> {
    pub(super) fn value(&self) -> &T {
        &self.0
    }
}

/// Descriptor for a single configuration knob in the Parameters form.
#[derive(Debug, Clone)]
pub(crate) struct Parameter<T> {
    pub(super) id: &'static str,
    pub(super) argument: &'static str,
    pub(super) environment: &'static str,
    pub(super) default: T,
    pub(super) toml: Option<&'static str>,
    pub(super) sensitive: bool,
    value_parser: ValueParser,
}

impl<T> Parameter<T> {
    fn to_arg(&self) -> Arg {
        Arg::new(self.id)
            .long(self.argument.trim_start_matches('-'))
            .env(self.environment)
            .value_parser(self.value_parser.clone())
            .required(false)
    }
}

impl<T> Parameter<T> {
    pub(super) fn try_resolve_from_args<U>(
        &self,
        arguments: &ArgMatches,
    ) -> Result<Option<(U, ValueSource)>>
    where
        U: Clone + Send + Sync + 'static,
    {
        arguments
            .try_get_one::<U>(self.id)?
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

    pub(super) fn try_resolve_from_file<U>(
        &self,
        file_config: &toml::Table,
    ) -> Result<Option<(U, ValueSource)>>
    where
        U: DeserializeOwned,
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
}

impl<T> From<&Parameter<T>> for Arg {
    fn from(param: &Parameter<T>) -> Self {
        param.to_arg()
    }
}

lazy_static! {
    // ── General ──────────────────────────────────────────────────────────────────────────
    pub(super) static ref CONFIG_FILE: Parameter<PhantomData<PathBuf>> = Parameter {
        id: "config_file",
        argument: "--config-file",
        environment: "NETIXFS_CONFIG_FILE",
        default: PhantomData,
        toml: None,
        sensitive: false,
        value_parser: value_parser!(PathBuf),
    };

    // ── Server ───────────────────────────────────────────────────────────────────────────
    pub(super) static ref SERVER_BIND_ADDRESS: Parameter<Default<IpAddr>> = Parameter {
        id: "server.bind_address",
        argument: "--bind-address",
        environment: "NETIXFS_SERVER_BIND_ADDRESS",
        default: Default(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        toml: Some("server.bind_address"),
        sensitive: false,
        value_parser: value_parser!(IpAddr).into(),
    };

    pub(super) static ref SERVER_PORT: Parameter<Default<u16>> = Parameter {
        id: "server.port",
        argument: "--port",
        environment: "NETIXFS_SERVER_PORT",
        default: Default(8080),
        toml: Some("server.port"),
        sensitive: false,
        value_parser: value_parser!(u16).into(),
    };

    pub(super) static ref SERVER_PUBLIC_BASE_URL: Parameter<PhantomData<Url>> = Parameter {
        id: "server.public_base_url",
        argument: "--public-base-url",
        environment: "NETIXFS_SERVER_PUBLIC_BASE_URL",
        default: PhantomData,
        toml: Some("server.public_base_url"),
        sensitive: false,
        value_parser: value_parser!(Url).into(),
    };

    // ── TLS ──────────────────────────────────────────────────────────────────────────────
    pub(super) static ref TLS_ENABLED: Parameter<Default<bool>> = Parameter {
        id: "tls.enabled",
        argument: "--tls-enabled",
        environment: "NETIXFS_TLS_ENABLED",
        default: Default(false),
        toml: Some("tls.enabled"),
        sensitive: false,
        value_parser: value_parser!(bool),
    };

    pub(super) static ref TLS_CERT_PATH: Parameter<PhantomData<PathBuf>> = Parameter {
        id: "tls.cert_path",
        argument: "--tls-cert-path",
        environment: "NETIXFS_TLS_CERT_PATH",
        default: PhantomData,
        toml: Some("tls.cert_path"),
        sensitive: false,
        value_parser: value_parser!(PathBuf),
    };

    pub(super) static ref TLS_KEY_PATH: Parameter<PhantomData<PathBuf>> = Parameter {
        id: "tls.key_path",
        argument: "--tls-key-path",
        environment: "NETIXFS_TLS_KEY_PATH",
        default: PhantomData,
        toml: Some("tls.key_path"),
        sensitive: true,
        value_parser: value_parser!(PathBuf),
    };

    // ── Authentication ───────────────────────────────────────────────────────────────────
    pub(super) static ref AUTH_JWT_PUBLIC_KEY_PATH: Parameter<PhantomData<PathBuf>> = Parameter {
        id: "auth.jwt.public_key_path",
        argument: "--jwt-public-key-path",
        environment: "NETIXFS_AUTH_JWT_PUBLIC_KEY_PATH",
        default: PhantomData,
        toml: Some("auth.jwt.public_key_path"),
        sensitive: false,
        value_parser: value_parser!(PathBuf),
    };

    pub(super) static ref AUTH_JWT_PUBLIC_KEY_URL: Parameter<PhantomData<Url>> = Parameter {
        id: "auth.jwt.public_key_url",
        argument: "--jwt-public-key-url",
        environment: "NETIXFS_AUTH_JWT_PUBLIC_KEY_URL",
        default: PhantomData,
        toml: Some("auth.jwt.public_key_url"),
        sensitive: true, // secret-bearing URL: may contain embedded credentials
        value_parser: value_parser!(Url).into(),
    };

    pub(super) static ref AUTH_JWT_JWKS_PATH: Parameter<PhantomData<PathBuf>> = Parameter {
        id: "auth.jwt.jwks_path",
        argument: "--jwt-jwks-path",
        environment: "NETIXFS_AUTH_JWT_JWKS_PATH",
        default: PhantomData,
        toml: Some("auth.jwt.jwks_path"),
        sensitive: false,
        value_parser: value_parser!(PathBuf),
    };

    pub(super) static ref AUTH_JWT_JWKS_URL: Parameter<PhantomData<Url>> = Parameter {
        id: "auth.jwt.jwks_url",
        argument: "--jwt-jwks-url",
        environment: "NETIXFS_AUTH_JWT_JWKS_URL",
        default: PhantomData,
        toml: Some("auth.jwt.jwks_url"),
        sensitive: true, // secret-bearing URL: may contain embedded credentials
        value_parser: value_parser!(PathBuf),
    };

    pub(super) static ref AUTH_JWT_ISSUER: Parameter<PhantomData<String>> = Parameter {
        id: "auth.jwt.issuer",
        argument: "--jwt-issuer",
        environment: "NETIXFS_AUTH_JWT_ISSUER",
        default: PhantomData,
        toml: Some("auth.jwt.issuer"),
        sensitive: false,
        value_parser: value_parser!(String),
    };

    pub(super) static ref AUTH_JWT_AUDIENCE: Parameter<PhantomData<String>> = Parameter {
        id: "auth.jwt.audience",
        argument: "--jwt-audience",
        environment: "NETIXFS_AUTH_JWT_AUDIENCE",
        default: PhantomData,
        toml: Some("auth.jwt.audience"),
        sensitive: false,
        value_parser: value_parser!(String),
    };

    pub(super) static ref AUTH_JWT_USERNAME_CLAIM: Parameter<Default<String>> = Parameter {
        id: "auth.jwt.username_claim",
        argument: "--jwt-username-claim",
        environment: "NETIXFS_AUTH_JWT_USERNAME_CLAIM",
        default: Default("sub".into()),
        toml: Some("auth.jwt.username_claim"),
        sensitive: false,
        value_parser: value_parser!(String),
    };

    pub(super) static ref AUTH_JWT_REMOTE_KEY_REFRESH_INTERVAL: Parameter<Default<Duration>> = Parameter {
        id: "auth.jwt.remote_key_refresh_interval",
        argument: "--jwt-remote-key-refresh-interval",
        environment: "NETIXFS_AUTH_JWT_REMOTE_KEY_REFRESH_INTERVAL",
        default: Default(Duration::from_mins(5)),
        toml: Some("auth.jwt.remote_key_refresh_interval"),
        sensitive: false,
        value_parser: ValueParser::new(humantime::parse_duration),
    };

    // ── Filesystem ───────────────────────────────────────────────────────────────────────
    pub(super) static ref FILESYSTEM_ALLOWED_ROOTS: Parameter<Default<Vec<Root>>> = Parameter {
        id: "filesystem.allowed_roots",
        argument: "--allowed-root",
        environment: "NETIXFS_FILESYSTEM_ALLOWED_ROOTS",
        default: Default(Vec::new()),
        toml: Some("filesystem.allowed_roots"),
        sensitive: false,
        value_parser: value_parser!(Vec<Root>).into(),
        // TODO: Add action to append values
    };

    pub(super) static ref FILESYSTEM_READ_ONLY: Parameter<Default<bool>> = Parameter {
        id: "filesystem.read_only",
        argument: "--read-only",
        environment: "NETIXFS_FILESYSTEM_READ_ONLY",
        default: Default(false),
        toml: Some("filesystem.read_only"),
        sensitive: false,
        value_parser: value_parser!(bool),
    };

    pub(super) static ref FILESYSTEM_DEFAULT_FILE_MODE: Parameter<Default<FileMode>> = Parameter {
        id: "filesystem.default_file_mode",
        argument: "--default-file-mode",
        environment: "NETIXFS_FILESYSTEM_DEFAULT_FILE_MODE",
        default: Default(FileMode(0o0644)),
        toml: Some("filesystem.default_file_mode"),
        sensitive: false,
        value_parser: value_parser!(FileMode).into(),
    };

    pub(super) static ref FILESYSTEM_DEFAULT_DIR_MODE: Parameter<Default<FileMode>> = Parameter {
        id: "filesystem.default_dir_mode",
        argument: "--default-dir-mode",
        environment: "NETIXFS_FILESYSTEM_DEFAULT_DIR_MODE",
        default: Default(FileMode(0o0755)),
        toml: Some("filesystem.default_dir_mode"),
        sensitive: false,
        value_parser: value_parser!(FileMode).into(),
    };

    pub(super) static ref FILESYSTEM_UMASK: Parameter<PhantomData<FileMode>> = Parameter {
        id: "filesystem.umask",
        argument: "--umask",
        environment: "NETIXFS_FILESYSTEM_UMASK",
        default: PhantomData, // inherits process umask at runtime
        toml: Some("filesystem.umask"),
        sensitive: false,
        value_parser: value_parser!(FileMode).into(),
    };

    pub(super) static ref FILESYSTEM_SYMLINK_POLICY: Parameter<Default<SymlinkPolicy>> = Parameter {
        id: "filesystem.symlink_policy",
        argument: "--symlink-policy",
        environment: "NETIXFS_FILESYSTEM_SYMLINK_POLICY",
        default: Default(SymlinkPolicy::Reject),
        toml: Some("filesystem.symlink_policy"),
        sensitive: false,
        value_parser: value_parser!(SymlinkPolicy).into(),
    };

    pub(super) static ref FILESYSTEM_ALLOW_MOUNT_CROSSING: Parameter<Default<bool>> = Parameter {
        id: "filesystem.allow_mount_crossing",
        argument: "--allow-mount-crossing",
        environment: "NETIXFS_FILESYSTEM_ALLOW_MOUNT_CROSSING",
        default: Default(false),
        toml: Some("filesystem.allow_mount_crossing"),
        sensitive: false,
        value_parser: value_parser!(bool),
    };

    // ── Operations ───────────────────────────────────────────────────────────────────────
    pub(super) static ref OPERATIONS_ALLOW_RECURSIVE_DELETE: Parameter<Default<bool>> = Parameter {
        id: "operations.allow_recursive_delete",
        argument: "--allow-recursive-delete",
        environment: "NETIXFS_OPERATIONS_ALLOW_RECURSIVE_DELETE",
        default: Default(true),
        toml: Some("operations.allow_recursive_delete"),
        sensitive: false,
        value_parser: value_parser!(bool),
    };

    pub(super) static ref OPERATIONS_ALLOW_RECURSIVE_COPY: Parameter<Default<bool>> = Parameter {
        id: "operations.allow_recursive_copy",
        argument: "--allow-recursive-copy",
        environment: "NETIXFS_OPERATIONS_ALLOW_RECURSIVE_COPY",
        default: Default(true),
        toml: Some("operations.allow_recursive_copy"),
        sensitive: false,
        value_parser: value_parser!(bool),
    };

    pub(super) static ref OPERATIONS_ALLOW_CHMOD: Parameter<Default<bool>> = Parameter {
        id: "operations.allow_chmod",
        argument: "--allow-chmod",
        environment: "NETIXFS_OPERATIONS_ALLOW_CHMOD",
        default: Default(true),
        toml: Some("operations.allow_chmod"),
        sensitive: false,
        value_parser: value_parser!(bool),
    };

    pub(super) static ref OPERATIONS_ALLOW_HARD_LINKS: Parameter<Default<bool>> = Parameter {
        id: "operations.allow_hard_links",
        argument: "--allow-hard-links",
        environment: "NETIXFS_OPERATIONS_ALLOW_HARD_LINKS",
        default: Default(true),
        toml: Some("operations.allow_hard_links"),
        sensitive: false,
        value_parser: value_parser!(bool),
    };

    pub(super) static ref OPERATIONS_ALLOW_SYMLINK_CREATE: Parameter<Default<bool>> = Parameter {
        id: "operations.allow_symlink_create",
        argument: "--allow-symlink-create",
        environment: "NETIXFS_OPERATIONS_ALLOW_SYMLINK_CREATE",
        default: Default(true),
        toml: Some("operations.allow_symlink_create"),
        sensitive: false,
        value_parser: value_parser!(bool),
    };

    // ── Limits ───────────────────────────────────────────────────────────────────────────
    pub(super) static ref LIMITS_MAX_REQUEST_BODY_SIZE: Parameter<Default<ByteSize>> = Parameter {
        id: "limits.max_request_body_size",
        argument: "--max-request-body-size",
        environment: "NETIXFS_LIMITS_MAX_REQUEST_BODY_SIZE",
        default: Default(ByteSize::mib(100)),
        toml: Some("limits.max_request_body_size"),
        sensitive: false,
        value_parser: value_parser!(ByteSize).into(),
    };

    pub(super) static ref LIMITS_MAX_READ_SIZE: Parameter<Default<ByteSize>> = Parameter {
        id: "limits.max_read_size",
        argument: "--max-read-size",
        environment: "NETIXFS_LIMITS_MAX_READ_SIZE",
        default: Default(ByteSize::mib(100)),
        toml: Some("limits.max_read_size"),
        sensitive: false,
        value_parser: value_parser!(ByteSize).into(),
    };

    pub(super) static ref LIMITS_MAX_DIRECTORY_ENTRIES: Parameter<Default<u32>> = Parameter {
        id: "limits.max_directory_entries",
        argument: "--max-directory-entries",
        environment: "NETIXFS_LIMITS_MAX_DIRECTORY_ENTRIES",
        default: Default(10000),
        toml: Some("limits.max_directory_entries"),
        sensitive: false,
        value_parser: value_parser!(u32).into(),
    };

    pub(super) static ref LIMITS_MAX_CONCURRENT_REQUESTS: Parameter<Default<u32>> = Parameter {
        id: "limits.max_concurrent_requests",
        argument: "--max-concurrent-requests",
        environment: "NETIXFS_LIMITS_MAX_CONCURRENT_REQUESTS",
        default: Default(1024),
        toml: Some("limits.max_concurrent_requests"),
        sensitive: false,
        value_parser: value_parser!(u32).into(),
    };

    pub(super) static ref LIMITS_MAX_CONCURRENT_STREAMS: Parameter<Default<u32>> = Parameter {
        id: "limits.max_concurrent_streams",
        argument: "--max-concurrent-streams",
        environment: "NETIXFS_LIMITS_MAX_CONCURRENT_STREAMS",
        default: Default(128),
        toml: Some("limits.max_concurrent_streams"),
        sensitive: false,
        value_parser: value_parser!(u32).into(),
    };

    // ── Streaming ────────────────────────────────────────────────────────────────────────
    pub(super) static ref STREAMING_IDLE_TIMEOUT: Parameter<Default<Duration>> = Parameter {
        id: "streaming.idle_timeout",
        argument: "--stream-idle-timeout",
        environment: "NETIXFS_STREAMING_IDLE_TIMEOUT",
        default: Default(Duration::from_mins(5)),
        toml: Some("streaming.idle_timeout"),
        sensitive: false,
        value_parser: ValueParser::new(humantime::parse_duration),
    };

    pub(super) static ref STREAMING_MAX_DURATION: Parameter<Default<Duration>> = Parameter {
        id: "streaming.max_duration",
        argument: "--stream-max-duration",
        environment: "NETIXFS_STREAMING_MAX_DURATION",
        default: Default(Duration::from_hours(1)),
        toml: Some("streaming.max_duration"),
        sensitive: false,
        value_parser: ValueParser::new(humantime::parse_duration),
    };

    pub(super) static ref STREAMING_HEARTBEAT_INTERVAL: Parameter<Default<Duration>> = Parameter {
        id: "streaming.heartbeat_interval",
        argument: "--stream-heartbeat-interval",
        environment: "NETIXFS_STREAMING_HEARTBEAT_INTERVAL",
        default: Default(Duration::from_secs(30)),
        toml: Some("streaming.heartbeat_interval"),
        sensitive: false,
        value_parser: ValueParser::new(humantime::parse_duration),
    };

    // ── Worker Pool ──────────────────────────────────────────────────────────────────────
    pub(super) static ref POOL_MAX_WORKERS: Parameter<Default<u32>> = Parameter {
        id: "pool.max_workers",
        argument: "--pool-max-workers",
        environment: "NETIXFS_POOL_MAX_WORKERS",
        default: Default(64),
        toml: Some("pool.max_workers"),
        sensitive: false,
        value_parser: value_parser!(u32).into(),
    };

    pub(super) static ref POOL_IDLE_TIMEOUT: Parameter<Default<Duration>> = Parameter {
        id: "pool.idle_timeout",
        argument: "--pool-idle-timeout",
        environment: "NETIXFS_POOL_IDLE_TIMEOUT",
        default: Default(Duration::from_mins(5)),
        toml: Some("pool.idle_timeout"),
        sensitive: false,
        value_parser: ValueParser::new(humantime::parse_duration),
    };

    pub(super) static ref POOL_REQUEST_TIMEOUT: Parameter<Default<Duration>> = Parameter {
        id: "pool.request_timeout",
        argument: "--pool-request-timeout",
        environment: "NETIXFS_POOL_REQUEST_TIMEOUT",
        default: Default(Duration::from_secs(30)),
        toml: Some("pool.request_timeout"),
        sensitive: false,
        value_parser: ValueParser::new(humantime::parse_duration),
    };

    // ── Logging ──────────────────────────────────────────────────────────────────────────
    pub(super) static ref LOGGING_LEVEL: Parameter<Default<LogLevel>> = Parameter {
        id: "logging.level",
        argument: "--log-level",
        environment: "NETIXFS_LOGGING_LEVEL",
        default: Default(LogLevel::Info),
        toml: Some("logging.level"),
        sensitive: false,
        value_parser: value_parser!(LogLevel).into(),
    };

    pub(super) static ref LOGGING_FORMAT: Parameter<Default<LogFormat>> = Parameter {
        id: "logging.format",
        argument: "--log-format",
        environment: "NETIXFS_LOGGING_FORMAT",
        default: Default(LogFormat::Json),
        toml: Some("logging.format"),
        sensitive: false,
        value_parser: value_parser!(LogFormat).into(),
    };

    pub(super) static ref LOGGING_REDACT_PATHS: Parameter<Default<bool>> = Parameter {
        id: "logging.redact_paths",
        argument: "--log-redact-paths",
        environment: "NETIXFS_LOGGING_REDACT_PATHS",
        default: Default(false),
        toml: Some("logging.redact_paths"),
        sensitive: false,
        value_parser: value_parser!(bool),
    };

    // ── Metrics ──────────────────────────────────────────────────────────────────────────
    pub(super) static ref METRICS_ENABLED: Parameter<Default<bool>> = Parameter {
        id: "metrics.enabled",
        argument: "--metrics-enabled",
        environment: "NETIXFS_METRICS_ENABLED",
        default: Default(false),
        toml: Some("metrics.enabled"),
        sensitive: false,
        value_parser: value_parser!(bool),
    };

    pub(super) static ref METRICS_BIND_ADDRESS: Parameter<Default<IpAddr>> = Parameter {
        id: "metrics.bind_address",
        argument: "--metrics-bind-address",
        environment: "NETIXFS_METRICS_BIND_ADDRESS",
        default: Default(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        toml: Some("metrics.bind_address"),
        sensitive: false,
        value_parser: value_parser!(IpAddr).into(),
    };

    pub(super) static ref METRICS_PORT: Parameter<Default<u16>> = Parameter {
        id: "metrics.port",
        argument: "--metrics-port",
        environment: "NETIXFS_METRICS_PORT",
        default: Default(9090),
        toml: Some("metrics.port"),
        sensitive: false,
        value_parser: value_parser!(u16).into(),
    };

    // ── Diagnostics ───────────────────────────────────────────────────────────────────────
    pub(super) static ref DIAGNOSTICS_CONFIG_ENDPOINT_ENABLED: Parameter<Default<bool>> = Parameter {
        id: "diagnostics.config_endpoint.enabled",
        argument: "--config-endpoint-enabled",
        environment: "NETIXFS_DIAGNOSTICS_CONFIG_ENDPOINT_ENABLED",
        default: Default(false),
        toml: Some("diagnostics.config_endpoint.enabled"),
        sensitive: false,
        value_parser: value_parser!(bool),
    };

    pub(super) static ref DIAGNOSTICS_CONFIG_ENDPOINT_BIND_ADDRESS: Parameter<Default<SocketAddr>> = Parameter {
        id: "diagnostics.config_endpoint.bind_address",
        argument: "--config-endpoint-bind-address",
        environment: "NETIXFS_DIAGNOSTICS_CONFIG_ENDPOINT_BIND_ADDRESS",
        default: Default(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 8081))),
        toml: Some("diagnostics.config_endpoint.bind_address"),
        sensitive: false,
        value_parser: value_parser!(SocketAddr).into(),
    };
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
