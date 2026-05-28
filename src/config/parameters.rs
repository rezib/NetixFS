use crate::config::{FileMode, LogFormat, LogLevel, Root, SymlinkPolicy, ValueSource};
use bytesize::ByteSize;
use clap::{Arg, ArgAction, ArgMatches, Id, builder::ValueParser, value_parser};
use eyre::{Context, OptionExt, Result};
use lazy_static::lazy_static;
use serde::de::DeserializeOwned;
use std::{
    convert::identity,
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    path::PathBuf,
    time::Duration,
};
use url::Url;

//#[derive(Debug)]
//pub(super) struct Default<T>(T);
//
//impl<T> Default<T> {
//    pub(super) fn value(&self) -> &T {
//        &self.0
//    }
//}

/// Descriptor for a single configuration parameter knob.
#[derive(Debug, Clone)]
pub(crate) struct Parameter<T> {
    pub(super) id: &'static str,
    pub(super) argument: &'static str,
    pub(super) environment: &'static str,
    pub(super) default: T,
    pub(super) toml: Option<&'static str>,
    pub(super) sensitive: bool,
    setup_arg: fn(Arg) -> Arg,
    get_arg: Option<fn(&ArgMatches) -> Result<T>>,
}

impl<T> Parameter<T> {
    fn to_arg(&self) -> Arg {
        let arg = Arg::new(self.id)
            .long(self.argument.trim_start_matches('-'))
            .env(self.environment)
            .required(false);
        self.setup_arg.unwrap_or(identity)(arg)
    }
}

fn get_one<T>(args: &ArgMatches, id: &str) -> Result<Option<T>> where T: Clone + Send + Sync + 'static {
    Ok(args.try_get_one::<T>(id)?.cloned())
}

impl<T> Parameter<T> {
    pub(super) fn try_resolve_from_args(
        &self,
        arguments: &ArgMatches,
    ) -> Result<Option<(T, ValueSource)>>
    where
        T: Clone + Send + Sync + 'static,
    {
        self.get_arg.unwrap_or() arguments
            .try_get_one::<T>(self.id)?
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

lazy_static! {
    // ── General ──────────────────────────────────────────────────────────────────────────
    pub(super) static ref CONFIG_FILE: Parameter<Option<PathBuf>> = Parameter {
        id: "config_file",
        argument: "--config-file",
        environment: "NETIXFS_CONFIG_FILE",
        default: None,
        toml: None,
        sensitive: false,
        arg_value_parser: value_parser!(PathBuf),
        arg_action: ArgAction::Set,
    };

    // ── Server ───────────────────────────────────────────────────────────────────────────
    pub(super) static ref SERVER_BIND_ADDRESS: Parameter<IpAddr> = Parameter {
        id: "server.bind_address",
        argument: "--bind-address",
        environment: "NETIXFS_SERVER_BIND_ADDRESS",
        default: IpAddr::V4(Ipv4Addr::LOCALHOST),
        toml: Some("server.bind_address"),
        sensitive: false,
        arg_value_parser: value_parser!(IpAddr).into(),
        arg_action: ArgAction::Set,
    };

    pub(super) static ref SERVER_PORT: Parameter<u16> = Parameter {
        id: "server.port",
        argument: "--port",
        environment: "NETIXFS_SERVER_PORT",
        default: 8080,
        toml: Some("server.port"),
        sensitive: false,
        arg_value_parser: value_parser!(u16).into(),
        arg_action: ArgAction::Set,
    };

    pub(super) static ref SERVER_PUBLIC_BASE_URL: Parameter<Option<Url>> = Parameter {
        id: "server.public_base_url",
        argument: "--public-base-url",
        environment: "NETIXFS_SERVER_PUBLIC_BASE_URL",
        default: None,
        toml: Some("server.public_base_url"),
        sensitive: false,
        arg_value_parser: value_parser!(Url).into(),
        arg_action: ArgAction::Set,
    };

    // ── TLS ──────────────────────────────────────────────────────────────────────────────
    pub(super) static ref TLS_ENABLED: Parameter<bool> = Parameter {
        id: "tls.enabled",
        argument: "--tls-enabled",
        environment: "NETIXFS_TLS_ENABLED",
        default: false,
        toml: Some("tls.enabled"),
        sensitive: false,
        arg_value_parser: value_parser!(bool),
        arg_action: ArgAction::Set,
    };

    pub(super) static ref TLS_CERT_PATH: Parameter<Option<PathBuf>> = Parameter {
        id: "tls.cert_path",
        argument: "--tls-cert-path",
        environment: "NETIXFS_TLS_CERT_PATH",
        default: None,
        toml: Some("tls.cert_path"),
        sensitive: false,
        arg_value_parser: value_parser!(PathBuf),
        arg_action: ArgAction::Set,
    };

    pub(super) static ref TLS_KEY_PATH: Parameter<Option<PathBuf>> = Parameter {
        id: "tls.key_path",
        argument: "--tls-key-path",
        environment: "NETIXFS_TLS_KEY_PATH",
        default: None,
        toml: Some("tls.key_path"),
        sensitive: true,
        arg_value_parser: value_parser!(PathBuf),
        arg_action: ArgAction::Set,
    };

    // ── Authentication ───────────────────────────────────────────────────────────────────
    pub(super) static ref AUTH_JWT_PUBLIC_KEY_PATH: Parameter<Option<PathBuf>> = Parameter {
        id: "auth.jwt.public_key_path",
        argument: "--jwt-public-key-path",
        environment: "NETIXFS_AUTH_JWT_PUBLIC_KEY_PATH",
        default: None,
        toml: Some("auth.jwt.public_key_path"),
        sensitive: false,
        arg_value_parser: value_parser!(PathBuf),
        arg_action: ArgAction::Set,
    };

    pub(super) static ref AUTH_JWT_PUBLIC_KEY_URL: Parameter<Option<Url>> = Parameter {
        id: "auth.jwt.public_key_url",
        argument: "--jwt-public-key-url",
        environment: "NETIXFS_AUTH_JWT_PUBLIC_KEY_URL",
        default: None,
        toml: Some("auth.jwt.public_key_url"),
        sensitive: true, // secret-bearing URL: may contain embedded credentials
        arg_value_parser: value_parser!(Url).into(),
        arg_action: ArgAction::Set,
    };

    pub(super) static ref AUTH_JWT_JWKS_PATH: Parameter<Option<PathBuf>> = Parameter {
        id: "auth.jwt.jwks_path",
        argument: "--jwt-jwks-path",
        environment: "NETIXFS_AUTH_JWT_JWKS_PATH",
        default: None,
        toml: Some("auth.jwt.jwks_path"),
        sensitive: false,
        arg_value_parser: value_parser!(PathBuf),
        arg_action: ArgAction::Set,
    };

    pub(super) static ref AUTH_JWT_JWKS_URL: Parameter<Option<Url>> = Parameter {
        id: "auth.jwt.jwks_url",
        argument: "--jwt-jwks-url",
        environment: "NETIXFS_AUTH_JWT_JWKS_URL",
        default: None,
        toml: Some("auth.jwt.jwks_url"),
        sensitive: true, // secret-bearing URL: may contain embedded credentials
        arg_value_parser: value_parser!(PathBuf),
        arg_action: ArgAction::Set,
    };

    pub(super) static ref AUTH_JWT_ISSUER: Parameter<Option<String>> = Parameter {
        id: "auth.jwt.issuer",
        argument: "--jwt-issuer",
        environment: "NETIXFS_AUTH_JWT_ISSUER",
        default: None,
        toml: Some("auth.jwt.issuer"),
        sensitive: false,
        arg_value_parser: value_parser!(String),
        arg_action: ArgAction::Set,
    };

    pub(super) static ref AUTH_JWT_AUDIENCE: Parameter<Option<String>> = Parameter {
        id: "auth.jwt.audience",
        argument: "--jwt-audience",
        environment: "NETIXFS_AUTH_JWT_AUDIENCE",
        default: None,
        toml: Some("auth.jwt.audience"),
        sensitive: false,
        arg_value_parser: value_parser!(String),
        arg_action: ArgAction::Set,
    };

    pub(super) static ref AUTH_JWT_USERNAME_CLAIM: Parameter<String> = Parameter {
        id: "auth.jwt.username_claim",
        argument: "--jwt-username-claim",
        environment: "NETIXFS_AUTH_JWT_USERNAME_CLAIM",
        default: "sub".to_owned(),
        toml: Some("auth.jwt.username_claim"),
        sensitive: false,
        arg_value_parser: value_parser!(String),
        arg_action: ArgAction::Set,
    };

    pub(super) static ref AUTH_JWT_REMOTE_KEY_REFRESH_INTERVAL: Parameter<Duration> = Parameter {
        id: "auth.jwt.remote_key_refresh_interval",
        argument: "--jwt-remote-key-refresh-interval",
        environment: "NETIXFS_AUTH_JWT_REMOTE_KEY_REFRESH_INTERVAL",
        default: Duration::from_mins(5),
        toml: Some("auth.jwt.remote_key_refresh_interval"),
        sensitive: false,
        arg_value_parser: ValueParser::new(humantime::parse_duration),
        arg_action: ArgAction::Set,
    };

    // ── Filesystem ───────────────────────────────────────────────────────────────────────
    pub(super) static ref FILESYSTEM_ALLOWED_ROOTS: Parameter<Vec<Root>> = Parameter {
        id: "filesystem.allowed_roots",
        argument: "--allowed-root",
        environment: "NETIXFS_FILESYSTEM_ALLOWED_ROOTS",
        default: Vec::new(),
        toml: Some("filesystem.allowed_roots"),
        sensitive: false,
        arg_value_parser: value_parser!(Root).into(),
        arg_action: ArgAction::Append,
    };

    pub(super) static ref FILESYSTEM_READ_ONLY: Parameter<bool> = Parameter {
        id: "filesystem.read_only",
        argument: "--read-only",
        environment: "NETIXFS_FILESYSTEM_READ_ONLY",
        default: false,
        toml: Some("filesystem.read_only"),
        sensitive: false,
        arg_value_parser: value_parser!(bool),
        arg_action: ArgAction::Set,
    };

    pub(super) static ref FILESYSTEM_DEFAULT_FILE_MODE: Parameter<FileMode> = Parameter {
        id: "filesystem.default_file_mode",
        argument: "--default-file-mode",
        environment: "NETIXFS_FILESYSTEM_DEFAULT_FILE_MODE",
        default: FileMode(0o0644),
        toml: Some("filesystem.default_file_mode"),
        sensitive: false,
        arg_value_parser: value_parser!(FileMode).into(),
        arg_action: ArgAction::Set,
    };

    pub(super) static ref FILESYSTEM_DEFAULT_DIR_MODE: Parameter<FileMode> = Parameter {
        id: "filesystem.default_dir_mode",
        argument: "--default-dir-mode",
        environment: "NETIXFS_FILESYSTEM_DEFAULT_DIR_MODE",
        default: FileMode(0o0755),
        toml: Some("filesystem.default_dir_mode"),
        sensitive: false,
        arg_value_parser: value_parser!(FileMode).into(),
        arg_action: ArgAction::Set,
    };

    pub(super) static ref FILESYSTEM_UMASK: Parameter<Option<FileMode>> = Parameter {
        id: "filesystem.umask",
        argument: "--umask",
        environment: "NETIXFS_FILESYSTEM_UMASK",
        default: None, // inherits process umask at runtime
        toml: Some("filesystem.umask"),
        sensitive: false,
        arg_value_parser: value_parser!(FileMode).into(),
        arg_action: ArgAction::Set,
    };

    pub(super) static ref FILESYSTEM_SYMLINK_POLICY: Parameter<SymlinkPolicy> = Parameter {
        id: "filesystem.symlink_policy",
        argument: "--symlink-policy",
        environment: "NETIXFS_FILESYSTEM_SYMLINK_POLICY",
        default: SymlinkPolicy::Reject,
        toml: Some("filesystem.symlink_policy"),
        sensitive: false,
        arg_value_parser: value_parser!(SymlinkPolicy).into(),
        arg_action: ArgAction::Set,
    };

    pub(super) static ref FILESYSTEM_ALLOW_MOUNT_CROSSING: Parameter<bool> = Parameter {
        id: "filesystem.allow_mount_crossing",
        argument: "--allow-mount-crossing",
        environment: "NETIXFS_FILESYSTEM_ALLOW_MOUNT_CROSSING",
        default: false,
        toml: Some("filesystem.allow_mount_crossing"),
        sensitive: false,
    };

    // ── Operations ───────────────────────────────────────────────────────────────────────
    pub(super) static ref OPERATIONS_ALLOW_RECURSIVE_DELETE: Parameter<bool> = Parameter {
        id: "operations.allow_recursive_delete",
        argument: "--allow-recursive-delete",
        environment: "NETIXFS_OPERATIONS_ALLOW_RECURSIVE_DELETE",
        default: true,
        toml: Some("operations.allow_recursive_delete"),
        sensitive: false,
    };

    pub(super) static ref OPERATIONS_ALLOW_RECURSIVE_COPY: Parameter<bool> = Parameter {
        id: "operations.allow_recursive_copy",
        argument: "--allow-recursive-copy",
        environment: "NETIXFS_OPERATIONS_ALLOW_RECURSIVE_COPY",
        default: true,
        toml: Some("operations.allow_recursive_copy"),
        sensitive: false,
    };

    pub(super) static ref OPERATIONS_ALLOW_CHMOD: Parameter<bool> = Parameter {
        id: "operations.allow_chmod",
        argument: "--allow-chmod",
        environment: "NETIXFS_OPERATIONS_ALLOW_CHMOD",
        default: true,
        toml: Some("operations.allow_chmod"),
        sensitive: false,
    };

    pub(super) static ref OPERATIONS_ALLOW_HARD_LINKS: Parameter<bool> = Parameter {
        id: "operations.allow_hard_links",
        argument: "--allow-hard-links",
        environment: "NETIXFS_OPERATIONS_ALLOW_HARD_LINKS",
        default: true,
        toml: Some("operations.allow_hard_links"),
        sensitive: false,
        arg_value_parser: value_parser!(bool),
    };

    pub(super) static ref OPERATIONS_ALLOW_SYMLINK_CREATE: Parameter<bool> = Parameter {
        id: "operations.allow_symlink_create",
        argument: "--allow-symlink-create",
        environment: "NETIXFS_OPERATIONS_ALLOW_SYMLINK_CREATE",
        default: true,
        toml: Some("operations.allow_symlink_create"),
        sensitive: false,
        arg_value_parser: value_parser!(bool),
    };

    // ── Limits ───────────────────────────────────────────────────────────────────────────
    pub(super) static ref LIMITS_MAX_REQUEST_BODY_SIZE: Parameter<ByteSize> = Parameter {
        id: "limits.max_request_body_size",
        argument: "--max-request-body-size",
        environment: "NETIXFS_LIMITS_MAX_REQUEST_BODY_SIZE",
        default: ByteSize::mib(100),
        toml: Some("limits.max_request_body_size"),
        sensitive: false,
        arg_value_parser: value_parser!(ByteSize).into(),
    };

    pub(super) static ref LIMITS_MAX_READ_SIZE: Parameter<ByteSize> = Parameter {
        id: "limits.max_read_size",
        argument: "--max-read-size",
        environment: "NETIXFS_LIMITS_MAX_READ_SIZE",
        default: ByteSize::mib(100),
        toml: Some("limits.max_read_size"),
        sensitive: false,
        arg_value_parser: value_parser!(ByteSize).into(),
    };

    pub(super) static ref LIMITS_MAX_DIRECTORY_ENTRIES: Parameter<u32> = Parameter {
        id: "limits.max_directory_entries",
        argument: "--max-directory-entries",
        environment: "NETIXFS_LIMITS_MAX_DIRECTORY_ENTRIES",
        default: 10000,
        toml: Some("limits.max_directory_entries"),
        sensitive: false,
        arg_value_parser: value_parser!(u32).into(),
    };

    pub(super) static ref LIMITS_MAX_CONCURRENT_REQUESTS: Parameter<u32> = Parameter {
        id: "limits.max_concurrent_requests",
        argument: "--max-concurrent-requests",
        environment: "NETIXFS_LIMITS_MAX_CONCURRENT_REQUESTS",
        default: 1024,
        toml: Some("limits.max_concurrent_requests"),
        sensitive: false,
        arg_value_parser: value_parser!(u32).into(),
    };

    pub(super) static ref LIMITS_MAX_CONCURRENT_STREAMS: Parameter<u32> = Parameter {
        id: "limits.max_concurrent_streams",
        argument: "--max-concurrent-streams",
        environment: "NETIXFS_LIMITS_MAX_CONCURRENT_STREAMS",
        default: 128,
        toml: Some("limits.max_concurrent_streams"),
        sensitive: false,
        arg_value_parser: value_parser!(u32).into(),
    };

    // ── Streaming ────────────────────────────────────────────────────────────────────────
    pub(super) static ref STREAMING_IDLE_TIMEOUT: Parameter<Duration> = Parameter {
        id: "streaming.idle_timeout",
        argument: "--stream-idle-timeout",
        environment: "NETIXFS_STREAMING_IDLE_TIMEOUT",
        default: Duration::from_mins(5),
        toml: Some("streaming.idle_timeout"),
        sensitive: false,
        arg_value_parser: ValueParser::new(humantime::parse_duration),
    };

    pub(super) static ref STREAMING_MAX_DURATION: Parameter<Duration> = Parameter {
        id: "streaming.max_duration",
        argument: "--stream-max-duration",
        environment: "NETIXFS_STREAMING_MAX_DURATION",
        default: Duration::from_hours(1),
        toml: Some("streaming.max_duration"),
        sensitive: false,
        arg_value_parser: ValueParser::new(humantime::parse_duration),
    };

    pub(super) static ref STREAMING_HEARTBEAT_INTERVAL: Parameter<Duration> = Parameter {
        id: "streaming.heartbeat_interval",
        argument: "--stream-heartbeat-interval",
        environment: "NETIXFS_STREAMING_HEARTBEAT_INTERVAL",
        default: Duration::from_secs(30),
        toml: Some("streaming.heartbeat_interval"),
        sensitive: false,
        arg_value_parser: ValueParser::new(humantime::parse_duration),
    };

    // ── Worker Pool ──────────────────────────────────────────────────────────────────────
    pub(super) static ref POOL_MAX_WORKERS: Parameter<u32> = Parameter {
        id: "pool.max_workers",
        argument: "--pool-max-workers",
        environment: "NETIXFS_POOL_MAX_WORKERS",
        default: 64,
        toml: Some("pool.max_workers"),
        sensitive: false,
        arg_value_parser: value_parser!(u32).into(),
    };

    pub(super) static ref POOL_IDLE_TIMEOUT: Parameter<Duration> = Parameter {
        id: "pool.idle_timeout",
        argument: "--pool-idle-timeout",
        environment: "NETIXFS_POOL_IDLE_TIMEOUT",
        default: Duration::from_mins(5),
        toml: Some("pool.idle_timeout"),
        sensitive: false,
        arg_value_parser: ValueParser::new(humantime::parse_duration),
    };

    pub(super) static ref POOL_REQUEST_TIMEOUT: Parameter<Duration> = Parameter {
        id: "pool.request_timeout",
        argument: "--pool-request-timeout",
        environment: "NETIXFS_POOL_REQUEST_TIMEOUT",
        default: Duration::from_secs(30),
        toml: Some("pool.request_timeout"),
        sensitive: false,
        arg_value_parser: ValueParser::new(humantime::parse_duration),
    };

    // ── Logging ──────────────────────────────────────────────────────────────────────────
    pub(super) static ref LOGGING_LEVEL: Parameter<LogLevel> = Parameter {
        id: "logging.level",
        argument: "--log-level",
        environment: "NETIXFS_LOGGING_LEVEL",
        default: LogLevel::Info,
        toml: Some("logging.level"),
        sensitive: false,
        arg_value_parser: value_parser!(LogLevel).into(),
    };

    pub(super) static ref LOGGING_FORMAT: Parameter<LogFormat> = Parameter {
        id: "logging.format",
        argument: "--log-format",
        environment: "NETIXFS_LOGGING_FORMAT",
        default: LogFormat::Json,
        toml: Some("logging.format"),
        sensitive: false,
        arg_value_parser: value_parser!(LogFormat).into(),
    };

    pub(super) static ref LOGGING_REDACT_PATHS: Parameter<bool> = Parameter {
        id: "logging.redact_paths",
        argument: "--log-redact-paths",
        environment: "NETIXFS_LOGGING_REDACT_PATHS",
        default: false,
        toml: Some("logging.redact_paths"),
        sensitive: false,
        arg_value_parser: value_parser!(bool),
    };

    // ── Metrics ──────────────────────────────────────────────────────────────────────────
    pub(super) static ref METRICS_ENABLED: Parameter<bool> = Parameter {
        id: "metrics.enabled",
        argument: "--metrics-enabled",
        environment: "NETIXFS_METRICS_ENABLED",
        default: false,
        toml: Some("metrics.enabled"),
        sensitive: false,
        arg_value_parser: value_parser!(bool),
    };

    pub(super) static ref METRICS_BIND_ADDRESS: Parameter<IpAddr> = Parameter {
        id: "metrics.bind_address",
        argument: "--metrics-bind-address",
        environment: "NETIXFS_METRICS_BIND_ADDRESS",
        default: IpAddr::V4(Ipv4Addr::LOCALHOST),
        toml: Some("metrics.bind_address"),
        sensitive: false,
        arg_value_parser: value_parser!(IpAddr).into(),
    };

    pub(super) static ref METRICS_PORT: Parameter<u16> = Parameter {
        id: "metrics.port",
        argument: "--metrics-port",
        environment: "NETIXFS_METRICS_PORT",
        default: 9090,
        toml: Some("metrics.port"),
        sensitive: false,
        arg_value_parser: value_parser!(u16).into(),
    };

    // ── Diagnostics ───────────────────────────────────────────────────────────────────────
    pub(super) static ref DIAGNOSTICS_CONFIG_ENDPOINT_ENABLED: Parameter<bool> = Parameter {
        id: "diagnostics.config_endpoint.enabled",
        argument: "--config-endpoint-enabled",
        environment: "NETIXFS_DIAGNOSTICS_CONFIG_ENDPOINT_ENABLED",
        default: false,
        toml: Some("diagnostics.config_endpoint.enabled"),
        sensitive: false,
        arg_value_parser: value_parser!(bool),
    };

    pub(super) static ref DIAGNOSTICS_CONFIG_ENDPOINT_BIND_ADDRESS: Parameter<SocketAddr> = Parameter {
        id: "diagnostics.config_endpoint.bind_address",
        argument: "--config-endpoint-bind-address",
        environment: "NETIXFS_DIAGNOSTICS_CONFIG_ENDPOINT_BIND_ADDRESS",
        default: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 8081)),
        toml: Some("diagnostics.config_endpoint.bind_address"),
        sensitive: false,
        arg_value_parser: value_parser!(SocketAddr).into(),
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
