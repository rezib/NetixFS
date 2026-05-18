# NetixFS Technical Specification

## 1. Purpose

NetixFS is a Linux-only service that exposes selected POSIX filesystem
operations through an HTTP(S) API.

The service acts as an HTTP proxy for filesystem access. It is intended to let
remote clients perform high-level file and directory operations that would
normally be performed from a shell, while preserving POSIX permission semantics
as much as possible.

## 2. Product Scope

### Goals

- Provide a REST-style API for common filesystem operations on files,
  directories, metadata, and streams.
- Support both read and write workflows.
- Support streaming workflows such as `tail -f`.
- Preserve POSIX permission semantics as much as possible.
- Minimize the operational risk of exposing filesystem operations over the
  network.
- Provide deployment and configuration that remain simple enough for service
  operators to reason about.

### Non-Goals

- NetixFS must not implement primary user authentication itself.
- NetixFS must not provide its own user database.
- NetixFS is not a distributed filesystem and must not attempt to replicate,
  synchronize, or cache filesystem state across hosts.

## 3. Design Guidelines

- NetixFS must be implemented in Rust.
- NetixFS aims to support Linux only in the initial version.
- NetixFS validates JWTs itself before performing filesystem operations.
- JWT signature validation is mandatory.
- JWT issuer validation must be supported and may be optional depending on
  deployment policy.
- NetixFS resolves the local Linux UID, primary GID, and supplementary groups
  from a username claim present in the JWT.
- NetixFS executes filesystem operations from a process running under the
  resolved UID, primary GID, and supplementary groups.
- NetixFS supports a bounded worker process pool so recently active users can
  reuse existing worker processes instead of spawning a fresh process for every
  request.
- NetixFS supports configuration through a TOML configuration file, environment
  variables, and command-line arguments.
- Configuration precedence is, from lowest to highest: TOML configuration file,
  environment variables, then command-line arguments.

## 4. System Requirements

The service should be deployable as:

- a system service on a Linux host;
- a containerized service, provided the required filesystem mounts and Linux
  capabilities are available;
- an internal service behind a reverse proxy, API gateway, or identity-aware
  proxy.

The runtime environment must provide:

- access to the filesystem roots that NetixFS is allowed to expose;
- local user and group resolution through NSS;
- Linux process identity controls for UID, primary GID, and supplementary group
  switching;
- the minimum Linux capabilities required by the selected privilege model;
- network access for the HTTP(S) listener and, if used, JWT key discovery.

The deployment model must make the required privileges explicit, especially for
containerized deployments where capabilities, mounts, user namespaces, and
account resolution may differ from a normal host service.

## 5. Authentication Boundary

NetixFS expects requests to contain a JWT issued by an external authentication
system, for example Keycloak, Authentik, or another identity provider.

NetixFS must validate the JWT itself before performing any filesystem operation.
Validation requirements include:

- mandatory signature verification using a configured public key source or JWKS
  source;
- expiration and not-before validation;
- optional issuer validation;
- optional audience validation;
- optional claim validation for tenant, group, role, or scope constraints.

NetixFS must support both local paths and remote URLs for JWT verification key
sources. Supported key sources are:

- static public key from a local file path;
- static public key from a remote URL;
- JWKS from a local file path;
- JWKS from a remote URL.

When both a static public key source and a JWKS source are configured, NetixFS
must use JWKS. JWKS is preferred because it supports multiple keys and smoother
key rotation.

When a local static public key file is used, NetixFS must load the configured
public key at startup and use it for JWT signature validation. Key rotation in
this mode is an operator responsibility and should require updating the file and
restarting or reloading the service.

When a remote static public key URL is used, NetixFS must retrieve the public
key from the configured URL and refresh it on a configured interval.

When a local JWKS file is used, NetixFS must load signing keys from the
configured JWKS path. Key rotation in this mode is an operator responsibility
and should require updating the file and restarting or reloading the service.

When a remote JWKS URL is used, NetixFS must retrieve signing keys from the
configured JWKS URL and refresh them on a configured interval.

Refresh failures for remote key sources should be reported through logs and
metrics. NetixFS should continue using the last valid key material until it
expires or until a stricter deployment policy requires failing closed.

## 6. Identity Mapping

Each request must resolve to a local Linux filesystem identity from a username
claim contained in the JWT. NetixFS must not trust numeric UID or GID claims as
the authoritative filesystem identity.

The resolved identity consists of:

- local username;
- local UID;
- local primary GID;
- supplementary groups resolved locally.

The username, UID, primary GID, and supplementary groups must be resolved
exclusively through NSS. NetixFS must not read `/etc/passwd`, `/etc/group`, LDAP,
SSSD, or any other identity data source directly. If NSS cannot resolve the
username locally, the request must fail before any filesystem operation is
attempted.

If any required identity component cannot be resolved, including username, UID,
primary GID, or supplementary groups, NetixFS must refuse the request before any
filesystem operation is attempted. The refusal must be logged with a clear
operator-facing reason.

## 7. Authorization Model

Filesystem authorization must be delegated to the Linux kernel by performing
operations in a worker process running under the resolved UID, primary GID, and
supplementary groups.

NetixFS must not implement an application-level filesystem allow/deny
authorization policy. It may still enforce service-level safety boundaries that
define what the service exposes and how much work a request may perform,
including:

- one or more allowed root directories;
- optional read-only mode;
- maximum request body size;
- maximum file size for non-streaming reads;
- maximum number of directory entries returned in one response;
- symlink traversal policy.

## 8. HTTP API Design Requirements

The API must be explicit, stable, machine-friendly, and versioned. The initial
API version is `v1`, exposed under `/api/v1`. Backward-incompatible API changes
must use a new version prefix.

All endpoints require a valid JWT bearer token unless explicitly documented as a
health or readiness endpoint. Filesystem endpoints are scoped to a configured
root identifier:

```text
/api/v1/roots/{root_id}/...
```

`root_id` identifies one configured allowed root. Filesystem paths must not be
embedded as arbitrary URL path segments. Every operation that targets a
filesystem path must provide that path as a query parameter or JSON field using
one of the following representations:

- `path`: a UTF-8 relative POSIX path string, for example
  `projects/report.txt`;
- `path_b64`: a base64url-without-padding encoding of the raw relative POSIX
  path bytes.

Exactly one of `path` or `path_b64` must be supplied for a given path value. The
path must be relative to the selected `root_id`. Absolute paths, NUL bytes, empty
path components, and `..` components must be rejected before filesystem access.
The root directory itself should be represented by an empty `path` value or the
equivalent empty `path_b64` value. For operations with two paths, such as rename
or copy, the request body must use `source` and `destination` path objects with
the same representation rules.

Path handling must include the following containment guards:

- `..` segments: reject paths containing traversal components before filesystem
  access, and resolve accepted paths relative to a configured allowed root.
- Symlink escapes: apply the configured symlink policy consistently. If symlink
  traversal is allowed, resolve the final target and verify that it remains
  inside an allowed root before use.
- Bind mounts: define whether crossing mount points is allowed. If it is not
  allowed, compare device IDs during path resolution and reject paths that cross
  out of the allowed root's device.
- Race conditions and time-of-check/time-of-use issues: prefer descriptor-based
  filesystem operations rooted at an already opened directory, using Linux APIs
  such as `openat` / `openat2` where available, so validation and use are not
  separated by an attacker-controllable path lookup.

Example: if `/srv/netixfs` is an allowed root and a client requests
`projects/report.txt`, NetixFS must avoid validating the absolute path
`/srv/netixfs/projects/report.txt` and then opening it later as a separate
operation. Between validation and open, another process could replace
`projects` with a symlink or otherwise change the path lookup result. A safer
implementation opens `/srv/netixfs` as a directory file descriptor, resolves
path components relative to that descriptor, applies symlink and containment
rules during resolution, and opens the final file relative to the already opened
parent directory. On Linux, `openat2` with resolution constraints such as
`RESOLVE_BENEATH` and `RESOLVE_NO_SYMLINKS` should be used when available.

POSIX paths are arbitrary byte sequences except for `/` and NUL. The API must
support non-UTF-8 paths through `path_b64`. JSON responses that include path
values should include both a UTF-8 `path` field when lossless UTF-8 conversion is
possible and a `path_b64` field when raw byte preservation is required.

File contents must be transferred as `application/octet-stream`. Metadata,
directory listings, operation controls, and errors must use JSON. Error
responses must follow the structured error model defined in section 11.

Conditional write requests should support standard HTTP precondition headers
where useful, especially `If-Match`, `If-None-Match`, and `If-Unmodified-Since`.
ETags may be derived from stable metadata such as device ID, inode, size, and
mtime, but NetixFS must not promise stronger consistency than the underlying
filesystem provides.

### 8.1 Metadata API

| Operation | Method and path | Parameters | Response |
| --- | --- | --- | --- |
| Stat or lstat | `GET /api/v1/roots/{root_id}/metadata` | `path` or `path_b64`; `follow_symlinks=true|false` | JSON metadata including type, size, mode, UID, GID, timestamps, inode, device ID, and symlink target when relevant |
| Existence check | `HEAD /api/v1/roots/{root_id}/metadata` | `path` or `path_b64`; `follow_symlinks=true|false` | `200` if visible and accessible, `404` if not found, appropriate error otherwise |
| List extended attributes | `GET /api/v1/roots/{root_id}/xattrs` | `path` or `path_b64`; `follow_symlinks=true|false` | JSON list of extended attribute names |
| Read extended attribute | `GET /api/v1/roots/{root_id}/xattrs/{name}` | `path` or `path_b64`; `follow_symlinks=true|false` | Raw bytes or JSON with base64url value, as defined by response negotiation |

POSIX ACLs must not be exposed through the extended attribute API in the initial
version.

### 8.2 Directory API

| Operation | Method and path | Parameters or body | Response |
| --- | --- | --- | --- |
| List directory | `GET /api/v1/roots/{root_id}/directory` | `path` or `path_b64`; optional `limit`; optional `cursor` | JSON entries with pagination cursor when more entries are available |
| Create directory | `POST /api/v1/roots/{root_id}/directory` | JSON path object; optional `mode`; optional `parents=false` | JSON metadata for created directory |
| Remove directory | `DELETE /api/v1/roots/{root_id}/directory` | `path` or `path_b64`; optional `recursive=false` | `204 No Content` on success |
| Rename or move path | `POST /api/v1/roots/{root_id}/rename` | JSON `source`, `destination`; optional `replace=false` | JSON metadata for destination or `204 No Content` |
| Copy path | `POST /api/v1/roots/{root_id}/copy` | JSON `source`, `destination`; optional `recursive=false`; optional `replace=false` | JSON metadata for destination |

Recursive directory removal and recursive copy must be disabled unless explicitly
enabled in configuration. Non-recursive removal must fail when the directory is
not empty. Directory listing must enforce configured entry limits and return a
cursor instead of loading unbounded entries into memory.

All v1 filesystem operations are synchronous HTTP requests. Recursive operations,
when enabled, must obey configured request timeouts and resource limits. NetixFS
does not expose a background job API in v1.

### 8.3 File API

| Operation | Method and path | Parameters or body | Response |
| --- | --- | --- | --- |
| Read file | `GET /api/v1/roots/{root_id}/file` | `path` or `path_b64`; optional standard `Range` header | `application/octet-stream`; supports `206 Partial Content` for ranges |
| Create or overwrite file | `PUT /api/v1/roots/{root_id}/file` | `path` or `path_b64`; raw request body; optional `mode`; optional `create=always|if_missing|replace` | JSON metadata or `204 No Content` |
| Atomic replace file | `PUT /api/v1/roots/{root_id}/file:atomic` | `path` or `path_b64`; raw request body; optional `mode` | JSON metadata or `204 No Content` |
| Append to file | `POST /api/v1/roots/{root_id}/file:append` | `path` or `path_b64`; raw request body | JSON metadata or `204 No Content` |
| Write byte range | `PATCH /api/v1/roots/{root_id}/file` | `path` or `path_b64`; required `offset`; raw request body | JSON metadata or `204 No Content` |
| Truncate file | `POST /api/v1/roots/{root_id}/file:truncate` | JSON path object; required `size` | JSON metadata or `204 No Content` |
| Delete file | `DELETE /api/v1/roots/{root_id}/file` | `path` or `path_b64` | `204 No Content` |

`PUT /file` replaces or creates the target according to the `create` mode.
`PUT /file:atomic` must write to a temporary file in the target directory and
then use atomic rename so readers never observe a partially written replacement.
Request body size and non-streaming read size limits must be enforced.

### 8.4 Link API

| Operation | Method and path | Parameters or body | Response |
| --- | --- | --- | --- |
| Read symbolic link | `GET /api/v1/roots/{root_id}/symlink` | `path` or `path_b64` | JSON symlink target |
| Create symbolic link | `POST /api/v1/roots/{root_id}/symlink` | JSON `path` for link location and `target` for link target | JSON metadata or `204 No Content` |
| Create hard link | `POST /api/v1/roots/{root_id}/hardlink` | JSON `source`, `destination` | JSON metadata or `204 No Content` |

Symbolic link creation and hard-link creation must be disabled unless explicitly
enabled in configuration. Symlink traversal behavior for all APIs must follow
the configured symlink policy.

### 8.5 Permissions and Timestamp API

| Operation | Method and path | Parameters or body | Response |
| --- | --- | --- | --- |
| Change mode | `PATCH /api/v1/roots/{root_id}/mode` | JSON path object; required `mode` | JSON metadata or `204 No Content` |
| Change group | `PATCH /api/v1/roots/{root_id}/group` | JSON path object; required local GID or group name | JSON metadata or `204 No Content` |
| Update timestamps | `PATCH /api/v1/roots/{root_id}/timestamps` | JSON path object; optional `atime`; optional `mtime` | JSON metadata or `204 No Content` |

`chmod` and `chgrp` must be disabled unless explicitly enabled in configuration.

The initial version must not support ownership changes through `chown`.
Ownership changes are especially sensitive and would require a separate threat
model before being considered for a later version.

The initial version must not support POSIX ACL read or write operations. POSIX
ACLs are out of scope even on filesystems where they are represented through
extended attributes.

## 9. Streaming Requirements

NetixFS must support streaming file reads over HTTP using chunked
`application/octet-stream` responses. WebSocket and Server-Sent Events are out
of scope for the initial version unless a later API version introduces a framed
streaming protocol.

| Operation | Method and path | Parameters | Response |
| --- | --- | --- | --- |
| Stream file from offset | `GET /api/v1/roots/{root_id}/file:stream` | `path` or `path_b64`; optional `offset`; optional `limit` | Chunked `application/octet-stream` |
| Follow appended data | `GET /api/v1/roots/{root_id}/file:tail` | `path` or `path_b64`; optional `offset`; optional `from_end=true`; `follow=descriptor` | Chunked `application/octet-stream` |
| Follow path across rotation | `GET /api/v1/roots/{root_id}/file:tail` | `path` or `path_b64`; `follow=path` | Chunked `application/octet-stream`; optional feature |

The initial version must support `follow=descriptor`, equivalent to `tail -f`: it
follows the opened file descriptor even if the path is later renamed or replaced.
`follow=path`, equivalent to `tail -F`, is optional and must only be available
when explicitly enabled in configuration.

Streaming must satisfy the following requirements:

- Path safeguards are identical to non-streaming file reads and must be applied
  before the stream starts.
- Once a stream starts, bytes are sent exactly as read from the file; NetixFS
  must not reinterpret file contents as text.
- Client disconnect must cancel the stream and release the worker, file
  descriptor, and any watcher resources.
- Backpressure from the HTTP connection must propagate to file reading so NetixFS
  does not buffer unbounded data per client.
- Configured maximum concurrent streams, per-stream idle timeout, maximum stream
  duration, and optional heartbeat behavior must be enforced.
- If an error occurs before response headers are sent, NetixFS must return a
  structured JSON error. If an error occurs after streaming has started, NetixFS
  must close the stream and log the reason with request and stream identifiers.
- If the followed file is truncated while using `follow=descriptor`, NetixFS must
  continue from the new end of the same opened file descriptor.
- If the followed file is deleted or replaced while using `follow=descriptor`,
  NetixFS must continue reading the opened descriptor until EOF and then wait for
  new data on that descriptor. It must not silently switch to the new path.
- If `follow=path` is enabled, NetixFS must document the polling or notification
  mechanism used to detect rotation and must reapply full path containment
  checks before opening the replacement path.

## 10. Concurrency and Consistency

NetixFS must define how it handles concurrent clients operating on the same
paths.

Concurrency rules for v1:

- the API does not expose file locking;
- the API does not expose background jobs;
- recursive operations are synchronous when enabled;
- atomic replacement is available through
  `PUT /api/v1/roots/{root_id}/file:atomic`;
- other write endpoints must document their atomicity in terms of Linux and
  filesystem behavior;
- partial failures must be reported through the structured error model.

Hard point: POSIX filesystems provide different guarantees depending on the
underlying filesystem. NetixFS should avoid promising stronger consistency than
Linux and the mounted filesystem can provide.

## 11. Error Model

Errors should be returned in a consistent structured format.

The error model should preserve useful POSIX information where safe, including:

- errno category;
- HTTP status code;
- operation name;
- normalized target path or opaque path identifier;
- whether the error is retryable;
- human-readable message;
- machine-readable error code.

Examples of important mappings:

- `ENOENT` to `404 Not Found`;
- `EACCES` / `EPERM` to `403 Forbidden`;
- `EEXIST` to `409 Conflict`;
- `ENOTDIR` / `EISDIR` to `400 Bad Request` or `409 Conflict`, depending on
  operation;
- invalid JWT to `401 Unauthorized`;
- valid identity but disallowed operation to `403 Forbidden`.

Hard point: Error responses must avoid leaking sensitive path existence or
metadata across authorization boundaries.

## 12. Configuration

NetixFS must support configuration through:

- a TOML configuration file;
- environment variables;
- command-line arguments.

Configuration precedence is, from lowest to highest:

1. TOML configuration file;
2. environment variables;
3. command-line arguments.

Command-line arguments therefore always override environment variables, and
environment variables always override values loaded from the TOML configuration
file.

All NetixFS environment variables must use the `NETIXFS_` prefix. Environment
variable names should be uppercase and use underscores to represent nested TOML
sections.

The path to the TOML configuration file should be configurable with
`--config-file` or `NETIXFS_CONFIG_FILE`. If no configuration file is provided,
NetixFS should either use documented defaults or fail fast when a required
setting has no value.

### 12.1 Configuration Settings

| Purpose | TOML setting | Command-line argument | Environment variable |
| --- | --- | --- | --- |
| Configuration file path | Not applicable | `--config-file` | `NETIXFS_CONFIG_FILE` |
| HTTP bind address | `server.bind_address` | `--bind-address` | `NETIXFS_SERVER_BIND_ADDRESS` |
| HTTP port | `server.port` | `--port` | `NETIXFS_SERVER_PORT` |
| Public base URL | `server.public_base_url` | `--public-base-url` | `NETIXFS_SERVER_PUBLIC_BASE_URL` |
| TLS enabled | `tls.enabled` | `--tls-enabled` | `NETIXFS_TLS_ENABLED` |
| TLS certificate path | `tls.cert_path` | `--tls-cert-path` | `NETIXFS_TLS_CERT_PATH` |
| TLS private key path | `tls.key_path` | `--tls-key-path` | `NETIXFS_TLS_KEY_PATH` |
| JWT public key path | `auth.jwt.public_key_path` | `--jwt-public-key-path` | `NETIXFS_AUTH_JWT_PUBLIC_KEY_PATH` |
| JWT public key URL | `auth.jwt.public_key_url` | `--jwt-public-key-url` | `NETIXFS_AUTH_JWT_PUBLIC_KEY_URL` |
| JWT JWKS path | `auth.jwt.jwks_path` | `--jwt-jwks-path` | `NETIXFS_AUTH_JWT_JWKS_PATH` |
| JWT JWKS URL | `auth.jwt.jwks_url` | `--jwt-jwks-url` | `NETIXFS_AUTH_JWT_JWKS_URL` |
| JWT issuer | `auth.jwt.issuer` | `--jwt-issuer` | `NETIXFS_AUTH_JWT_ISSUER` |
| Require issuer validation | `auth.jwt.require_issuer` | `--jwt-require-issuer` | `NETIXFS_AUTH_JWT_REQUIRE_ISSUER` |
| JWT audience | `auth.jwt.audience` | `--jwt-audience` | `NETIXFS_AUTH_JWT_AUDIENCE` |
| Require audience validation | `auth.jwt.require_audience` | `--jwt-require-audience` | `NETIXFS_AUTH_JWT_REQUIRE_AUDIENCE` |
| JWT username claim | `auth.jwt.username_claim` | `--jwt-username-claim` | `NETIXFS_AUTH_JWT_USERNAME_CLAIM` |
| Remote key refresh interval | `auth.jwt.remote_key_refresh_interval` | `--jwt-remote-key-refresh-interval` | `NETIXFS_AUTH_JWT_REMOTE_KEY_REFRESH_INTERVAL` |
| Allowed filesystem roots | `filesystem.allowed_roots` | `--allowed-root` | `NETIXFS_FILESYSTEM_ALLOWED_ROOTS` |
| Read-only mode | `filesystem.read_only` | `--read-only` | `NETIXFS_FILESYSTEM_READ_ONLY` |
| Default file creation mode | `filesystem.default_file_mode` | `--default-file-mode` | `NETIXFS_FILESYSTEM_DEFAULT_FILE_MODE` |
| Default directory creation mode | `filesystem.default_dir_mode` | `--default-dir-mode` | `NETIXFS_FILESYSTEM_DEFAULT_DIR_MODE` |
| Default umask | `filesystem.umask` | `--umask` | `NETIXFS_FILESYSTEM_UMASK` |
| Symlink policy | `filesystem.symlink_policy` | `--symlink-policy` | `NETIXFS_FILESYSTEM_SYMLINK_POLICY` |
| Allow mount crossing | `filesystem.allow_mount_crossing` | `--allow-mount-crossing` | `NETIXFS_FILESYSTEM_ALLOW_MOUNT_CROSSING` |
| Allow recursive delete | `operations.allow_recursive_delete` | `--allow-recursive-delete` | `NETIXFS_OPERATIONS_ALLOW_RECURSIVE_DELETE` |
| Allow recursive copy | `operations.allow_recursive_copy` | `--allow-recursive-copy` | `NETIXFS_OPERATIONS_ALLOW_RECURSIVE_COPY` |
| Allow chmod | `operations.allow_chmod` | `--allow-chmod` | `NETIXFS_OPERATIONS_ALLOW_CHMOD` |
| Allow hard links | `operations.allow_hard_links` | `--allow-hard-links` | `NETIXFS_OPERATIONS_ALLOW_HARD_LINKS` |
| Allow symbolic link creation | `operations.allow_symlink_create` | `--allow-symlink-create` | `NETIXFS_OPERATIONS_ALLOW_SYMLINK_CREATE` |
| Maximum request body size | `limits.max_request_body_size` | `--max-request-body-size` | `NETIXFS_LIMITS_MAX_REQUEST_BODY_SIZE` |
| Maximum non-streaming read size | `limits.max_read_size` | `--max-read-size` | `NETIXFS_LIMITS_MAX_READ_SIZE` |
| Maximum directory entries | `limits.max_directory_entries` | `--max-directory-entries` | `NETIXFS_LIMITS_MAX_DIRECTORY_ENTRIES` |
| Maximum concurrent requests | `limits.max_concurrent_requests` | `--max-concurrent-requests` | `NETIXFS_LIMITS_MAX_CONCURRENT_REQUESTS` |
| Maximum concurrent streams | `limits.max_concurrent_streams` | `--max-concurrent-streams` | `NETIXFS_LIMITS_MAX_CONCURRENT_STREAMS` |
| Stream idle timeout | `streaming.idle_timeout` | `--stream-idle-timeout` | `NETIXFS_STREAMING_IDLE_TIMEOUT` |
| Stream maximum duration | `streaming.max_duration` | `--stream-max-duration` | `NETIXFS_STREAMING_MAX_DURATION` |
| Stream heartbeat interval | `streaming.heartbeat_interval` | `--stream-heartbeat-interval` | `NETIXFS_STREAMING_HEARTBEAT_INTERVAL` |
| Allow path-follow tail mode | `streaming.allow_path_follow` | `--stream-allow-path-follow` | `NETIXFS_STREAMING_ALLOW_PATH_FOLLOW` |
| Worker pool maximum workers | `worker_pool.max_workers` | `--worker-pool-max-workers` | `NETIXFS_WORKER_POOL_MAX_WORKERS` |
| Worker idle timeout | `worker_pool.idle_timeout` | `--worker-pool-idle-timeout` | `NETIXFS_WORKER_POOL_IDLE_TIMEOUT` |
| Worker request timeout | `worker_pool.request_timeout` | `--worker-pool-request-timeout` | `NETIXFS_WORKER_POOL_REQUEST_TIMEOUT` |
| Log level | `logging.level` | `--log-level` | `NETIXFS_LOGGING_LEVEL` |
| Log format | `logging.format` | `--log-format` | `NETIXFS_LOGGING_FORMAT` |
| Log path redaction | `logging.redact_paths` | `--log-redact-paths` | `NETIXFS_LOGGING_REDACT_PATHS` |
| Metrics enabled | `metrics.enabled` | `--metrics-enabled` | `NETIXFS_METRICS_ENABLED` |
| Metrics bind address | `metrics.bind_address` | `--metrics-bind-address` | `NETIXFS_METRICS_BIND_ADDRESS` |
| Metrics port | `metrics.port` | `--metrics-port` | `NETIXFS_METRICS_PORT` |

List-valued environment variables, such as `NETIXFS_FILESYSTEM_ALLOWED_ROOTS`,
should use comma-separated values unless the implementation defines a stricter
format. Duration-valued settings should use a documented duration syntax, such
as `30s`, `5m`, or `1h`.

Hard point: Some settings, especially allowed filesystem roots and policy-like
operation controls, are structured enough that TOML should be the preferred
operator-facing format. Environment variables and command-line arguments remain
important for deployment overrides and secrets management.

## 13. Observability

NetixFS should provide production-grade observability:

- structured logs;
- request IDs and trace IDs;
- audit logs for write operations and permission-changing operations;
- metrics for request counts, latencies, error codes, open streams, bytes read,
  and bytes written;
- health and readiness endpoints.

Audit logs should include the authenticated subject, resolved UID, primary GID,
supplementary groups, operation, target path or redacted path, result, and
timestamp.

Hard point: Logs can leak sensitive filenames and directory structures. The
logging policy must define what is logged by default and how redaction works.

## 14. Security Requirements

Security is central to NetixFS because it exposes filesystem operations over a
network.

The design must address:

- JWT signature validation and optional issuer validation;
- local username-to-UID/GID/supplementary-groups resolution;
- privilege dropping and per-request UID, primary GID, and supplementary group
  switching;
- worker process pooling and identity isolation;
- supplementary group handling;
- path traversal prevention;
- symlink race prevention;
- mount and bind-mount escape behavior;
- resource exhaustion;
- request body limits;
- stream limits;
- rate limiting or upstream rate-limit expectations;
- TLS termination model;
- secure defaults for dangerous operations;
- protection against accidental exposure of host-sensitive paths.

Hard point: A service that can switch worker processes to arbitrary local UIDs
or retain excessive capabilities can become equivalent to root if compromised.
The worker lifecycle, capability set, and privilege-dropping sequence need
careful threat modeling before implementation.

## 15. Implementation Constraints

The implementation language is Rust.

The implementation should prefer:

- memory-safe Rust libraries;
- async HTTP handling where appropriate;
- bounded resource usage;
- explicit error types;
- integration tests that exercise real filesystem behavior on Linux;
- minimal unsafe code, with justification for any unsafe block.

Potential Rust ecosystem choices to evaluate:

- HTTP framework: Axum, Actix Web, or Hyper-based implementation;
- JWT validation: `jsonwebtoken`, `openidconnect`, or another maintained crate;
- async runtime: Tokio;
- tracing: `tracing` and `tracing-subscriber`;
- metrics: Prometheus-compatible exporter or OpenTelemetry.

Filesystem operations must run in separate worker processes after UID, primary
GID, and supplementary group switching. The HTTP layer should dispatch
authorized operations to an appropriate worker process and receive structured
results or streams back from that worker.

The HTTP supervisor and worker processes must communicate through per-worker
Unix domain socket pairs created by the supervisor before the worker drops
privileges. The socket pair should use a small framed protocol: one frame type
for operation requests, one for structured responses, one for structured errors,
and one for bounded byte chunks used by streaming reads.

Unix domain socket pairs are preferred because they are local-only,
bidirectional, compatible with async runtimes, and naturally fit both short
request/response operations and long-lived streaming responses. They also avoid
the lifecycle and access-control risks of filesystem-named sockets and are more
flexible than unidirectional pipes. Shared task queues should not be used for
worker IPC in the initial version because they make per-request cancellation,
backpressure, and stream ownership harder to reason about.

The supervisor must own request routing, cancellation, timeout enforcement, and
worker lifecycle management. Workers must only execute filesystem operations
received through their socket pair and must release file descriptors and other
resources when the supervisor closes the connection or cancels the request.

## 16. Testing Requirements

Testing should cover:

- path normalization and containment;
- symlink traversal and symlink race scenarios;
- JWT validation failures;
- username claim extraction failures;
- local username resolution failures;
- POSIX permission behavior;
- read and write operations;
- large directory listings;
- streaming reads;
- file truncation and rotation during streaming;
- concurrent writes and reads;
- error mapping;
- configuration parsing;
- Linux capability assumptions;
- worker process reuse, expiration, and identity isolation.

Security-sensitive behavior should be covered by integration tests, not only
unit tests.

## 17. Open Questions

The following questions should be resolved before implementation starts:

1. Which JWT claim is the authoritative local username?
2. What is the exact Linux privilege model for the supervisor and worker
   processes?
3. Can a worker process safely transition between local identities, or should a
   worker be bound to one identity for its lifetime?
4. What are the worker pool limits, expiration rules, and backpressure behavior?
5. What filesystem roots may be exposed?
6. Are symbolic links followed, rejected, or configurable per operation?
7. Is TLS handled by NetixFS or by an upstream proxy?
8. What information may be logged without violating privacy or security
    requirements?
9. What are the minimum supported Linux kernel and distribution assumptions?

Resolved for the initial version: `chown` and POSIX ACL support are out of
scope.

The v1 API path representation supports non-UTF-8 POSIX paths through
`path_b64`. The v1 API includes the file, directory, link, metadata, permission,
timestamp, and streaming operations defined in section 8. Dangerous operations
are either disabled unless explicitly enabled in configuration or excluded from
v1. Recursive operations are synchronous and bounded by configured limits.
Streaming supports `tail -f` through `follow=descriptor`; `tail -F` behavior is
optional through `follow=path` when explicitly enabled.

## 18. Initial Version Proposal

A conservative first version should include:

- direct JWT signature validation with optional issuer validation;
- local username resolution from a JWT claim;
- worker process execution under the resolved UID, primary GID, and
  supplementary groups;
- bounded worker process pool for recently active users;
- one or more configured allowed roots;
- metadata read;
- directory listing with limits;
- file read with range support;
- raw streaming read with `tail -f` semantics;
- file create, overwrite, append, truncate, rename, and delete;
- directory create and remove-empty-directory;
- no recursive delete by default;
- no `chown` support;
- no POSIX ACL support;
- no hard-link creation by default;
- explicit symlink policy;
- structured JSON errors;
- structured logs and basic metrics;
- Linux integration tests for permission and path-safety behavior.
