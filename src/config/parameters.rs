use crate::config::{FileMode, LogFormat, LogLevel, Root, SymlinkPolicy, Value, ValueSource};
use bon::Builder;
use bytesize::ByteSize;
use clap::{
    Arg, ArgAction, ArgGroup, ArgMatches,
    builder::{BoolishValueParser, ValueParser},
    value_parser,
};
use eyre::{Context, Result, eyre};
use lazy_static::lazy_static;
use serde::de::DeserializeOwned;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    path::PathBuf,
    time::Duration,
};
use url::Url;

/// Descriptor for a single configuration parameter knob.
#[derive(Debug, Builder)]
pub(crate) struct Parameter<T> {
    #[builder(start_fn)]
    pub(super) id: &'static str,

    pub(super) argument: &'static str,

    pub(super) environment: &'static str,

    pub(super) toml: Option<&'static str>,

    #[builder(into)]
    default: T,

    #[builder(default, with=|| true)]
    pub(super) sensitive: bool,

    #[builder(into)]
    arg_value_parser: Option<ValueParser>,

    #[builder(default = ArgAction::Set)]
    arg_action: ArgAction,
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

impl<T> Parameter<T>
where
    T: ValueSeed,
{
    pub(super) fn resolve(
        &self,
        arguments: &ArgMatches,
        file_config: Option<&toml::Table>,
    ) -> Result<Value<T::Output>> {
        let (value, source) = self.resolve_value(arguments, file_config)?;
        Ok(Value {
            value,
            source,
            id: self.id,
            argument: self.argument,
            environment: self.environment,
            toml: self.toml,
            sensitive: self.sensitive,
        })
    }

    fn resolve_value(
        &self,
        arguments: &ArgMatches,
        file_config: Option<&toml::Table>,
    ) -> Result<(T::Output, ValueSource)> {
        if let Some((value, source)) = T::read_from_args(arguments, self.id)? {
            return Ok((value, source));
        }

        if let Some((value, source)) = file_config
            .zip(self.toml)
            .and_then(|(file_config, key)| T::read_from_file(file_config, key).transpose())
            .transpose()?
        {
            return Ok((value, source));
        }

        Ok(T::make_from_default_seed(&self.default))
    }
}

pub(super) trait ValueSeed {
    type Output;

    fn read_from_args(args: &ArgMatches, id: &str) -> Result<Option<(Self::Output, ValueSource)>>;
    fn read_from_file(
        file_config: &toml::Table,
        key: &str,
    ) -> Result<Option<(Self::Output, ValueSource)>>;
    fn make_from_default_seed(&self) -> (Self::Output, ValueSource);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct Simple<T>(T);

impl<T> From<T> for Simple<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> ValueSeed for Simple<T>
where
    T: DeserializeOwned + Clone + Send + Sync + 'static,
{
    type Output = T;

    fn read_from_args(args: &ArgMatches, id: &str) -> Result<Option<(Self::Output, ValueSource)>> {
        get_one::<T>(args, id)
    }

    fn read_from_file(
        file_config: &toml::Table,
        key: &str,
    ) -> Result<Option<(Self::Output, ValueSource)>> {
        get_from_file_config(file_config, key)
    }

    fn make_from_default_seed(&self) -> (Self::Output, ValueSource) {
        (self.0.clone(), ValueSource::Default)
    }
}

impl<T> ValueSeed for Option<T>
where
    T: DeserializeOwned + Clone + Send + Sync + 'static,
{
    type Output = Option<T>;

    fn read_from_args(args: &ArgMatches, id: &str) -> Result<Option<(Self::Output, ValueSource)>> {
        Ok(get_one::<T>(args, id)?.map(|(value, source)| (Some(value), source)))
    }

    fn read_from_file(
        file_config: &toml::Table,
        key: &str,
    ) -> Result<Option<(Self::Output, ValueSource)>> {
        Ok(get_from_file_config::<T>(file_config, key)?
            .map(|(value, source)| (Some(value), source)))
    }

    fn make_from_default_seed(&self) -> (Self::Output, ValueSource) {
        (self.clone(), ValueSource::Default)
    }
}

impl<T> ValueSeed for Vec<T>
where
    T: DeserializeOwned + Clone + Send + Sync + 'static,
{
    type Output = Vec<T>;

    fn read_from_args(args: &ArgMatches, id: &str) -> Result<Option<(Self::Output, ValueSource)>> {
        get_many::<T, _>(args, id)
    }

    fn read_from_file(
        file_config: &toml::Table,
        key: &str,
    ) -> Result<Option<(Self::Output, ValueSource)>> {
        get_from_file_config::<Vec<T>>(file_config, key)
    }

    fn make_from_default_seed(&self) -> (Self::Output, ValueSource) {
        (self.clone(), ValueSource::Default)
    }
}

fn get_one<T>(args: &ArgMatches, id: &str) -> Result<Option<(T, ValueSource)>>
where
    T: Clone + Send + Sync + 'static,
{
    args.try_get_one::<T>(id)
        .wrap_err_with(|| {
            format!(
                "failed to convert configuration argument configuration value for id {:?}",
                id
            )
        })?
        .map(|value| {
            Ok((
                value.clone(),
                args.value_source(id)
                    .ok_or_else(|| {
                        eyre!("logic error: argument {:?} has a value but no source", id)
                    })?
                    .into(),
            ))
        })
        .transpose()
}

fn get_from_file_config<T>(file_config: &toml::Table, key: &str) -> Result<Option<(T, ValueSource)>>
where
    T: DeserializeOwned,
{
    key.split('.')
        .try_fold((Some(file_config), None), |(section, _), key| {
            let value = section?.get(key)?;
            Some((value.as_table(), Some(value)))
        })
        .and_then(|(_, value)| value.cloned())
        .map(|value| {
            Ok((
                value.try_into().wrap_err_with(|| {
                    format!(
                        "failed to convert file configuration value for key {:?}",
                        key
                    )
                })?,
                ValueSource::ConfigFile,
            ))
        })
        .transpose()
}

fn get_many<T, C>(args: &ArgMatches, id: &str) -> Result<Option<(C, ValueSource)>>
where
    T: Clone + Send + Sync + 'static,
    C: FromIterator<T>,
{
    args.try_get_many::<T>(id)?
        .map(|iter| {
            Ok((
                iter.cloned().collect(),
                args.value_source(id)
                    .ok_or_else(|| {
                        eyre!("logic error: argument {:?} has a value but no source", id)
                    })?
                    .into(),
            ))
        })
        .transpose()
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub(super) struct HumanDuration(#[serde(with = "humantime_serde")] Duration);

impl From<Duration> for HumanDuration {
    fn from(duration: Duration) -> Self {
        Self(duration)
    }
}

impl From<HumanDuration> for Duration {
    fn from(duration: HumanDuration) -> Self {
        duration.0
    }
}

impl std::str::FromStr for HumanDuration {
    type Err = humantime::DurationError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        humantime::parse_duration(s).map(Self)
    }
}

lazy_static! {
    // ── General ──────────────────────────────────────────────────────────────────────────
    pub(super) static ref CONFIG_FILE: Parameter<Option<PathBuf>> =
        Parameter::builder("config_file")
            .argument("--config-file")
            .environment("NETIXFS_CONFIG_FILE")
            .default(None)
            .arg_value_parser(value_parser!(PathBuf))
            .build();

    // ── Server ───────────────────────────────────────────────────────────────────────────
    pub(super) static ref SERVER_BIND_ADDRESS: Parameter<Simple<IpAddr>> =
        Parameter::builder("server.bind_address")
            .argument("--bind-address")
            .environment("NETIXFS_SERVER_BIND_ADDRESS")
            .toml("server.bind_address")
            .default(IpAddr::V4(Ipv4Addr::LOCALHOST))
            .arg_value_parser(value_parser!(IpAddr))
            .build();
    pub(super) static ref SERVER_PORT: Parameter<Simple<u16>> =
        Parameter::builder("server.port")
            .argument("--port")
            .environment("NETIXFS_SERVER_PORT")
            .toml("server.port")
            .default(8080u16)
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

    // ── TLS ─────────────────────────────────────────────────────────────────────────────
    pub(super) static ref TLS_ENABLED: Parameter<Simple<bool>> =
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
            .arg_value_parser(value_parser!(Url))
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

    pub(super) static ref AUTH_JWT_USERNAME_CLAIM: Parameter<Simple<String>> =
        Parameter::builder("auth.jwt.username_claim")
            .argument("--jwt-username-claim")
            .environment("NETIXFS_AUTH_JWT_USERNAME_CLAIM")
            .toml("auth.jwt.username_claim")
            .default("sub".to_owned())
            .arg_value_parser(value_parser!(String))
            .build();

    pub(super) static ref AUTH_JWT_REMOTE_KEY_REFRESH_INTERVAL: Parameter<Simple<HumanDuration>> =
        Parameter::builder("auth.jwt.remote_key_refresh_interval")
            .argument("--jwt-remote-key-refresh-interval")
            .environment("NETIXFS_AUTH_JWT_REMOTE_KEY_REFRESH_INTERVAL")
            .toml("auth.jwt.remote_key_refresh_interval")
            .default(HumanDuration(Duration::from_mins(5)))
            .arg_value_parser(value_parser!(HumanDuration))
            .build();

    // ── Filesystem ───────────────────────────────────────────────────────────────────────
    pub(super) static ref FILESYSTEM_ALLOWED_ROOTS: Parameter<Vec<Root>> =
        Parameter::builder("filesystem.allowed_roots")
            .argument("--allowed-root")
            .environment("NETIXFS_FILESYSTEM_ALLOWED_ROOTS")
            .toml("filesystem.allowed_roots")
            .default(Vec::new())
            .arg_value_parser(ValueParser::from(value_parser!(Root)))
            .arg_action(ArgAction::Append)
            .build();

    pub(super) static ref FILESYSTEM_READ_ONLY: Parameter<Simple<bool>> =
        Parameter::builder("filesystem.read_only")
            .argument("--read-only")
            .environment("NETIXFS_FILESYSTEM_READ_ONLY")
            .toml("filesystem.read_only")
            .default(false)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    pub(super) static ref FILESYSTEM_DEFAULT_FILE_MODE: Parameter<Simple<FileMode>> =
        Parameter::builder("filesystem.default_file_mode")
            .argument("--default-file-mode")
            .environment("NETIXFS_FILESYSTEM_DEFAULT_FILE_MODE")
            .toml("filesystem.default_file_mode")
            .default(FileMode(0o0644))
            .arg_value_parser(value_parser!(FileMode))
            .build();

    pub(super) static ref FILESYSTEM_DEFAULT_DIR_MODE: Parameter<Simple<FileMode>> =
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

    pub(super) static ref FILESYSTEM_SYMLINK_POLICY: Parameter<Simple<SymlinkPolicy>> =
        Parameter::builder("filesystem.symlink_policy")
            .argument("--symlink-policy")
            .environment("NETIXFS_FILESYSTEM_SYMLINK_POLICY")
            .toml("filesystem.symlink_policy")
            .default(SymlinkPolicy::Reject)
            .arg_value_parser(value_parser!(SymlinkPolicy))
            .build();

    pub(super) static ref FILESYSTEM_ALLOW_MOUNT_CROSSING: Parameter<Simple<bool>> =
        Parameter::builder("filesystem.allow_mount_crossing")
            .argument("--allow-mount-crossing")
            .environment("NETIXFS_FILESYSTEM_ALLOW_MOUNT_CROSSING")
            .toml("filesystem.allow_mount_crossing")
            .default(false)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    // ── Operations ───────────────────────────────────────────────────────────────────────
    pub(super) static ref OPERATIONS_ALLOW_RECURSIVE_DELETE: Parameter<Simple<bool>> =
        Parameter::builder("operations.allow_recursive_delete")
            .argument("--allow-recursive-delete")
            .environment("NETIXFS_OPERATIONS_ALLOW_RECURSIVE_DELETE")
            .toml("operations.allow_recursive_delete")
            .default(true) .arg_value_parser(BoolishValueParser::new())
            .build();

    pub(super) static ref OPERATIONS_ALLOW_RECURSIVE_COPY: Parameter<Simple<bool>> =
        Parameter::builder("operations.allow_recursive_copy")
            .argument("--allow-recursive-copy")
            .environment("NETIXFS_OPERATIONS_ALLOW_RECURSIVE_COPY")
            .toml("operations.allow_recursive_copy")
            .default(true)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    pub(super) static ref OPERATIONS_ALLOW_CHMOD: Parameter<Simple<bool>> =
        Parameter::builder("operations.allow_chmod")
            .argument("--allow-chmod")
            .environment("NETIXFS_OPERATIONS_ALLOW_CHMOD")
            .toml("operations.allow_chmod")
            .default(true)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    pub(super) static ref OPERATIONS_ALLOW_HARD_LINKS: Parameter<Simple<bool>> =
        Parameter::builder("operations.allow_hard_links")
            .argument("--allow-hard-links")
            .environment("NETIXFS_OPERATIONS_ALLOW_HARD_LINKS")
            .toml("operations.allow_hard_links")
            .default(true)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    pub(super) static ref OPERATIONS_ALLOW_SYMLINK_CREATE: Parameter<Simple<bool>> =
        Parameter::builder("operations.allow_symlink_create")
            .argument("--allow-symlink-create")
            .environment("NETIXFS_OPERATIONS_ALLOW_SYMLINK_CREATE")
            .toml("operations.allow_symlink_create")
            .default(true)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    // ── Limits ───────────────────────────────────────────────────────────────────────────
    pub(super) static ref LIMITS_MAX_REQUEST_BODY_SIZE: Parameter<Simple<ByteSize>> =
        Parameter::builder("limits.max_request_body_size")
            .argument("--max-request-body-size")
            .environment("NETIXFS_LIMITS_MAX_REQUEST_BODY_SIZE")
            .toml("limits.max_request_body_size")
            .default(ByteSize::mib(100))
            .arg_value_parser(value_parser!(ByteSize))
            .build();
    pub(super) static ref LIMITS_MAX_READ_SIZE: Parameter<Simple<ByteSize>> =
        Parameter::builder("limits.max_read_size")
            .argument("--max-read-size")
            .environment("NETIXFS_LIMITS_MAX_READ_SIZE")
            .toml("limits.max_read_size")
            .default(ByteSize::mib(100))
            .arg_value_parser(value_parser!(ByteSize))
            .build();
    pub(super) static ref LIMITS_MAX_DIRECTORY_ENTRIES: Parameter<Simple<u32>> =
        Parameter::builder("limits.max_directory_entries")
            .argument("--max-directory-entries")
            .environment("NETIXFS_LIMITS_MAX_DIRECTORY_ENTRIES")
            .toml("limits.max_directory_entries")
            .default(10000u32)
            .arg_value_parser(value_parser!(u32))
            .build();
    pub(super) static ref LIMITS_MAX_CONCURRENT_REQUESTS: Parameter<Simple<u32>> =
        Parameter::builder("limits.max_concurrent_requests")
            .argument("--max-concurrent-requests")
            .environment("NETIXFS_LIMITS_MAX_CONCURRENT_REQUESTS")
            .toml("limits.max_concurrent_requests")
            .default(1024u32)
            .arg_value_parser(value_parser!(u32))
            .build();
    pub(super) static ref LIMITS_MAX_CONCURRENT_STREAMS: Parameter<Simple<u32>> =
        Parameter::builder("limits.max_concurrent_streams")
            .argument("--max-concurrent-streams")
            .environment("NETIXFS_LIMITS_MAX_CONCURRENT_STREAMS")
            .toml("limits.max_concurrent_streams")
            .default(128u32)
            .arg_value_parser(value_parser!(u32))
            .build();

    // ── Streaming ────────────────────────────────────────────────────────────────────────
    pub(super) static ref STREAMING_IDLE_TIMEOUT: Parameter<Simple<HumanDuration>> =
        Parameter::builder("streaming.idle_timeout")
            .argument("--stream-idle-timeout")
            .environment("NETIXFS_STREAMING_IDLE_TIMEOUT")
            .toml("streaming.idle_timeout")
            .default(HumanDuration(Duration::from_mins(5)))
            .arg_value_parser(value_parser!(HumanDuration))
            .build();
    pub(super) static ref STREAMING_MAX_DURATION: Parameter<Simple<HumanDuration>> =
        Parameter::builder("streaming.max_duration")
            .argument("--stream-max-duration")
            .environment("NETIXFS_STREAMING_MAX_DURATION")
            .toml("streaming.max_duration")
            .default(HumanDuration(Duration::from_hours(1)))
            .arg_value_parser(value_parser!(HumanDuration))
            .build();
    pub(super) static ref STREAMING_HEARTBEAT_INTERVAL: Parameter<Simple<HumanDuration>> =
        Parameter::builder("streaming.heartbeat_interval")
            .argument("--stream-heartbeat-interval")
            .environment("NETIXFS_STREAMING_HEARTBEAT_INTERVAL")
            .toml("streaming.heartbeat_interval")
            .default(HumanDuration(Duration::from_secs(30)))
            .arg_value_parser(value_parser!(HumanDuration))
            .build();

    // ── Worker Pool ──────────────────────────────────────────────────────────────────────
    pub(super) static ref POOL_MAX_WORKERS: Parameter<Simple<u32>> =
        Parameter::builder("pool.max_workers")
            .argument("--pool-max-workers")
            .environment("NETIXFS_POOL_MAX_WORKERS")
            .toml("pool.max_workers")
            .default(64u32)
            .arg_value_parser(value_parser!(u32))
            .build();
    pub(super) static ref POOL_IDLE_TIMEOUT: Parameter<Simple<HumanDuration>> =
        Parameter::builder("pool.idle_timeout")
            .argument("--pool-idle-timeout")
            .environment("NETIXFS_POOL_IDLE_TIMEOUT")
            .toml("pool.idle_timeout")
            .default(HumanDuration(Duration::from_mins(5)))
            .arg_value_parser(value_parser!(HumanDuration))
            .build();
    pub(super) static ref POOL_REQUEST_TIMEOUT: Parameter<Simple<HumanDuration>> =
        Parameter::builder("pool.request_timeout")
            .argument("--pool-request-timeout")
            .environment("NETIXFS_POOL_REQUEST_TIMEOUT")
            .toml("pool.request_timeout")
            .default(HumanDuration(Duration::from_secs(30)))
            .arg_value_parser(value_parser!(HumanDuration))
            .build();

    // ── Logging ──────────────────────────────────────────────────────────────────────────
    pub(super) static ref LOGGING_LEVEL: Parameter<Simple<LogLevel>> =
        Parameter::builder("logging.level")
            .argument("--log-level")
            .environment("NETIXFS_LOGGING_LEVEL")
            .toml("logging.level")
            .default(LogLevel::Info)
            .arg_value_parser(value_parser!(LogLevel))
            .build();

    pub(super) static ref LOGGING_FORMAT: Parameter<Simple<LogFormat>> =
        Parameter::builder("logging.format")
            .argument("--log-format")
            .environment("NETIXFS_LOGGING_FORMAT")
            .toml("logging.format")
            .default(LogFormat::Json)
            .arg_value_parser(value_parser!(LogFormat))
            .build();

    pub(super) static ref LOGGING_REDACT_PATHS: Parameter<Simple<bool>> =
        Parameter::builder("logging.redact_paths")
            .argument("--log-redact-paths")
            .environment("NETIXFS_LOGGING_REDACT_PATHS")
            .toml("logging.redact_paths")
            .default(false)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    // ── Metrics ──────────────────────────────────────────────────────────────────────────
    pub(super) static ref METRICS_ENABLED: Parameter<Simple<bool>> =
        Parameter::builder("metrics.enabled")
            .argument("--metrics-enabled")
            .environment("NETIXFS_METRICS_ENABLED")
            .toml("metrics.enabled")
            .default(false)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    pub(super) static ref METRICS_BIND_ADDRESS: Parameter<Simple<IpAddr>> =
        Parameter::builder("metrics.bind_address")
            .argument("--metrics-bind-address")
            .environment("NETIXFS_METRICS_BIND_ADDRESS")
            .toml("metrics.bind_address")
            .default(IpAddr::V4(Ipv4Addr::LOCALHOST))
            .arg_value_parser(value_parser!(IpAddr))
            .build();

    pub(super) static ref METRICS_PORT: Parameter<Simple<u16>> =
        Parameter::builder("metrics.port")
            .argument("--metrics-port")
            .environment("NETIXFS_METRICS_PORT")
            .toml("metrics.port")
            .default(9090u16)
            .arg_value_parser(value_parser!(u16))
            .build();

    // ── Diagnostics ───────────────────────────────────────────────────────────────────────
    pub(super) static ref DIAGNOSTICS_CONFIG_ENDPOINT_ENABLED: Parameter<Simple<bool>> =
        Parameter::builder("diagnostics.config_endpoint.enabled")
            .argument("--config-endpoint-enabled")
            .environment("NETIXFS_DIAGNOSTICS_CONFIG_ENDPOINT_ENABLED")
            .toml("diagnostics.config_endpoint.enabled")
            .default(false)
            .arg_value_parser(BoolishValueParser::new())
            .build();

    pub(super) static ref DIAGNOSTICS_CONFIG_ENDPOINT_BIND_ADDRESS: Parameter<Simple<SocketAddr>> =
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
