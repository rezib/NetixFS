# NetixFS Technical Specifications

## 1. Purpose

NetixFS must be a Linux-only service that exposes selected POSIX filesystem
operations through an HTTP(S) API.

The service must act as an HTTP proxy for filesystem access. It must let remote
clients perform high-level file and directory operations that would normally be
performed from a shell, while preserving POSIX permission semantics as much as
possible.

NetixFS must offer a simple HTTP(S) API for end-user file management, with an
interface that is easy to integrate into applications.

## 2. Product Scope

### 2.1 Goals

- Provide a REST-style API for common high-level file management tasks involving
  files, directories, metadata, and streams.
- Support both read and write workflows, with streaming similar to `tail -f`.
- Preserve POSIX permission semantics as much as possible.
- Work on shared filesystems that use root squash, where root privileges are not
  sufficient for end-user filesystem access.
- Minimize the operational risk of exposing filesystem operations over the
  network.
- Provide deployment and configuration that remain simple enough for service
  operators to reason about.

### 2.2 Non-Goals

- NetixFS must not implement primary user authentication itself.
- NetixFS must not provide its own user database.
- NetixFS must not be a distributed filesystem and must not attempt to replicate,
  synchronize, or cache filesystem state across hosts.
- NetixFS must not expose a background job API for asynchronous operations.
- NetixFS must not be a general-purpose, low-level POSIX filesystem API for
  applications (eg. file locking).

## 3. Design Guidelines

### 3.1 Implementation

NetixFS must be implemented in Rust and should prefer memory-safe libraries.
Unsafe Rust should be minimal and justified for each unsafe block.

NetixFS must be Linux-only. It should work on currently supported Linux kernels
and Linux distributions, meaning kernels and distributions that still receive
upstream or vendor security updates. Runtime assumptions are detailed in
[section 4](#4-system-requirements).

### 3.2 Security And Identity

NetixFS must validate JWTs itself before performing filesystem operations. It
must map the configured JWT username claim to a local Linux UID, primary GID,
and supplementary groups through NSS. Authentication requirements are detailed
in [section 7](#7-authentication-boundary), and identity mapping is detailed in
[section 8](#8-identity-mapping).

### 3.3 Execution Model

NetixFS must use the supervisor/worker model defined in
[section 5](#5-software-architecture). The supervisor must handle the HTTP API,
authentication, identity resolution, request validation, routing, cancellation,
and worker lifecycle management. Workers must execute filesystem operations
under the resolved UID, primary GID, and supplementary groups.

The worker pool must be bounded, and each worker process must be bound to one
resolved local identity for its lifetime. Worker pool limits and backpressure
behavior are detailed in [section 5](#5-software-architecture).

### 3.4 Configuration

NetixFS must support configuration through a TOML configuration file,
environment variables, and command-line arguments. Configuration precedence
must be, from lowest to highest: TOML configuration file, environment variables,
then command-line arguments. Configuration settings are detailed in
[section 12](#12-configuration).

### 3.5 Deployment

NetixFS should be deployable as:

- a system service on a Linux host;
- a containerized service, provided the required filesystem mounts and Linux
  capabilities are available;
- an internal service behind a reverse proxy, API gateway, or identity-aware
  proxy.

## 4. System Requirements

The runtime environment must provide:

- a supported Linux kernel;
- access to the filesystem roots that NetixFS is allowed to expose;
- local user and group resolution through NSS;
- the minimum Linux capabilities required by the selected privilege model;
- network access for the HTTP(S) listener and, if used, JWT key discovery.

## 5. Software Architecture

NetixFS must use a supervisor/worker architecture. The supervisor must own the
network-facing API, request validation, routing, cancellation, and worker
lifecycle. Workers must own filesystem execution after switching to local Linux
identities.

### 5.1 Supervisor And Worker Model

The supervisor must handle all network-facing and control-plane work. Workers
must handle only filesystem operations received through their IPC socket after
switching to the resolved local Linux identity.

```mermaid
flowchart TD
  A[HTTP request] --> A1[Assign or validate request ID]
  A1 --> A2{CORS preflight OPTIONS on /api/v1?}
  A2 -- Yes, cors enabled --> A2a[Respond 204 or 403 without JWT or workers]
  A2 -- No --> B{limits.max_concurrent_requests available?}
  B -- No --> B1[429 Too Many Requests]
  B -- Yes --> C[Validate JWT and configured claims]
  C --> D{Token valid?}
  D -- No --> D1[401 Unauthorized]
  D -- Yes --> E[Extract username claim and resolve identity with NSS]
  E --> F{Identity resolved?}
  F -- No --> F1[403 Forbidden or 503 Service Unavailable]
  F -- Yes --> G[Resolve root_id and normalize path]
  G --> H{Path and root allowed?}
  H -- No --> H1[400, 403, 404, or 409 error]
  H -- Yes --> I[Apply request body, directory, and operation limits]
  I --> J{Operation allowed?}
  J -- No --> J1[403 Forbidden or 429 Too Many Requests]
  J -- Yes --> S{Streaming request?}
  S -- Yes --> S1{limits.max_concurrent_streams available?}
  S1 -- No --> S2[429 Too Many Requests]
  S1 -- Yes --> K{Idle worker for identity?}
  S -- No --> K
  K -- Yes --> M[Dispatch operation over socketpair]
  K -- No --> L{pool.max_workers capacity available?}
  L -- No --> L1[429 Too Many Requests]
  L -- Yes --> L2[Create worker bound to identity]
  L2 --> L3{Worker created?}
  L3 -- No --> L4[503 Service Unavailable]
  L3 -- Yes --> M
  M --> N[Worker performs descriptor-oriented filesystem operation]
  M --> BP[Propagate HTTP backpressure through bounded IPC frames]
  BP --> N
  N --> O{Operation completed?}
  O -- Success --> O1[Add CORS headers when enabled, then return response or stream]
  O -- Error --> O2[Add CORS headers when enabled, then return structured error]
  O -- Timeout or disconnect --> O3[Cancel operation and release resources]
```

Before dispatching to a worker, the supervisor must complete every check that
does not require filesystem access as the target user:

- request ID assignment or validation, as defined in
  [section 13.1](#131-request-ids);
- cross-origin resource sharing (CORS) on `/api/v1/**` when `cors.enabled=true`,
  as defined in [section 10.7](#107-cross-origin-resource-sharing-cors).
- authentication and JWT claim validation, as defined in
  [section 7](#7-authentication-boundary), except for CORS preflight requests;
- username extraction and NSS identity resolution, as defined in
  [section 8](#8-identity-mapping);
- root selection and path normalization, as defined in
  [section 10](#10-http-api-design);
- authorization and containment checks that are safe to perform before worker
  execution, as defined in [section 9](#9-authorization-model) and
  [section 10](#10-http-api-design);
- request body, directory, stream, concurrency, and worker pool limits, as
  defined in [section 5](#5-software-architecture),
  [section 10](#10-http-api-design), [section 11](#11-http-api-endpoints), and
  [section 12](#12-configuration);
- worker selection or creation for the resolved local identity;
- cancellation, timeout, backpressure, and logging context, as defined in
  [section 5](#5-software-architecture) and
  [section 13](#13-observability).

The supervisor must not perform client-requested filesystem operations directly.
It may only perform setup and control-plane work: worker creation, IPC
management, configured root resolution, service-limit enforcement, cancellation,
and worker termination. The supervisor capability model is defined in
[section 6](#6-security-model).

Workers must execute filesystem operations after UID, primary GID, and
supplementary group switching. Each worker must be bound to one resolved local
identity for its lifetime, must drop all capabilities before filesystem
execution, and must execute only operations received through its socket pair.

The worker pool bounds local identity contexts, worker processes, open file
descriptors, streams, and active filesystem operations. When limits are reached,
NetixFS must apply bounded backpressure or reject requests instead of creating
unbounded workers, queues, buffers, or streams.

The following limits must be enforced:

- `pool.max_workers` is the maximum number of live worker processes
  across all local identities.
- A worker should execute at most one active filesystem operation at a time.
- `limits.max_concurrent_requests` caps in-flight HTTP requests before worker
  dispatch.
- `limits.max_concurrent_streams` caps active streaming responses. Stream
  requests above this limit must be rejected before response headers are sent.

Requests that exceed concurrency or worker availability limits must fail with
`429 Too Many Requests`; worker creation failures caused by internal or
operating-system failures must fail with `503 Service Unavailable`.

### 5.2 Worker Communication

Supervisor and worker processes must communicate through per-worker Unix domain
socket pairs created with `socketpair`. Unlike filesystem-named Unix sockets,
socket pairs are anonymous, already-connected file descriptors. They require no
listener path, cannot be discovered by opening a pathname, and avoid lifecycle
and permission risks in `/tmp`, configured roots, or other operator-facing
directories.

Socket pairs should be used because they are local-only, bidirectional,
compatible with async runtimes, and suitable for both short request/response
operations and long-lived streaming responses. The supervisor must create each
pair, pass one endpoint to the worker, and keep the other endpoint.

The IPC protocol should use bounded frames:

- one frame type for operation requests;
- one frame type for structured responses;
- one frame type for structured errors;
- one frame type for bounded byte chunks used by streaming reads.

Shared task queues should not be used for worker IPC because they make
per-request cancellation, backpressure, identity isolation, and stream ownership
harder to reason about.

### 5.3 Worker Lifetime

A worker may be reused only for requests that resolve to the same local
identity. It remains eligible for reuse while it is idle and has not exceeded
`pool.idle_timeout`.

`pool.idle_timeout` defines how long an idle worker may remain alive
after its last completed operation. When the timeout expires, the supervisor
should terminate the worker and release its resources.

Active workers must not be expired by `pool.idle_timeout`. Non-streaming
operations must be bounded by `pool.request_timeout`; if the timeout is
exceeded, the supervisor must cancel the operation and may terminate the worker
if cancellation cannot complete cleanly.

Streaming operations are bounded by stream-specific idle timeout, maximum
duration, heartbeat, client disconnect, and cancellation rules. When a stream
ends, the worker becomes idle again and is subject to
`pool.idle_timeout`.

Workers must release file descriptors, inotify watches, IPC buffers, and other
resources when an operation completes, when the supervisor cancels the request,
or when the client disconnects. Detailed configuration keys are defined in
[section 12](#12-configuration).

## 6. Security Model

The supervisor must be able to start without root privileges. It must run under
a dedicated service account and receive only the capabilities required for
NetixFS operation:

- `CAP_SETUID`: required to create workers running as resolved local UIDs;
- `CAP_SETGID`: required to set the worker primary GID and supplementary groups;
- `CAP_KILL`: required only if the supervisor must forcibly terminate workers
  after they have switched to a different UID;
- `CAP_NET_BIND_SERVICE`: required only when NetixFS binds directly to a TCP
  port below 1024.

The supervisor must not require broad filesystem-bypass capabilities, including
`CAP_DAC_OVERRIDE`, `CAP_DAC_READ_SEARCH`, `CAP_FOWNER`, or `CAP_SYS_ADMIN`.
Deployments that cannot provide the required capability model must fail closed
rather than running the supervisor as root.

The external identity provider must be responsible for authenticating users and
issuing JWTs. NetixFS must validate token signatures and configured token
claims, but it must not maintain a user database and must not authenticate
passwords, sessions, or interactive login flows.

The local Linux identity must be derived from the configured JWT username claim
and resolved through NSS. Numeric UID, GID, or group claims from JWTs must not
be authoritative. Local UID, primary GID, and supplementary groups must come
from the host identity system.

Filesystem authorization must be delegated to the Linux kernel. Workers must
execute under the resolved local UID, primary GID, and supplementary groups, so
kernel permission checks remain the final authorization decision for filesystem
access.

Operators must decide which filesystem roots may be exposed. NetixFS must not
impose hardcoded restrictions on those roots, but every request must remain
inside one configured root and must pass path normalization, symlink,
mount-boundary, and resource-limit checks before filesystem access.

NetixFS must support both native TLS and deployment behind an upstream
TLS-terminating proxy. When native TLS is disabled, transport security becomes
an operator responsibility at the deployment boundary.

Cross-Origin Resource Sharing (CORS) is optional and disabled by default. It
controls what browser JavaScript on another origin may read from API responses.
Misconfigured CORS, such as broad `cors.allowed_origins` on a publicly reachable
listener, can let allowed frontends invoke the API with user-supplied Bearer
tokens. Details are in [section 10.7](#107-cross-origin-resource-sharing-cors).

## 7. Authentication Boundary

NetixFS expects requests to contain a JWT issued by an external authentication
system, for example Keycloak, Authentik, or another identity provider.

NetixFS must validate the JWT itself before performing any filesystem operation.
Validation requirements include:

- mandatory signature verification using a configured public key source or JWKS
  source;
- expiration and not-before validation;
- issuer validation when `auth.jwt.issuer` is configured;
- audience validation when `auth.jwt.audience` is configured;
- optional claim validation for tenant, group, role, or scope constraints.

NetixFS must support both local paths and remote URLs for JWT verification key
sources. Supported key sources are:

- static public key from a local file path: `auth.jwt.public_key_path`;
- static public key from a remote URL: `auth.jwt.public_key_url`;
- JWKS from a local file path: `auth.jwt.jwks_path`;
- JWKS from a remote URL: `auth.jwt.jwks_url`.

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

## 8. Identity Mapping

Each request must resolve to a local Linux filesystem identity from a username
claim contained in the JWT. The default username claim is `sub`. Operators may
override the claim name through `auth.jwt.username_claim`. NetixFS must not trust
numeric UID or GID claims as the authoritative filesystem identity.

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

## 9. Authorization Model

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

## 10. HTTP API Design

The API must be explicit, stable, and machine-friendly.

### 10.1 API Version Prefix

All versioned HTTP API routes must use the `/api/v1` prefix. The prefix is part
of the stable route contract and reserves space for future incompatible API
versions without changing the host, listener, or deployment topology. Endpoints
outside the versioned API, such as health, readiness, metrics, and diagnostics
endpoints, are not required to use this prefix.

### 10.2 Root Scopes

Filesystem endpoints are scoped to a configured root identifier:

```text
/api/v1/roots/{root_id}/...
```

`root_id` identifies one configured allowed root. Operators decide which
filesystem roots may be exposed through configuration. NetixFS must not impose
hardcoded path restrictions on exposed roots, but it must still enforce the
configured root boundary, path normalization rules, authentication, local POSIX
permissions, and service safety limits. Filesystem paths must not be embedded as
arbitrary URL path segments.

### 10.3 Path Normalization

Every operation that targets a filesystem path must provide that path as a query
parameter or JSON field using one of the following representations:

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

### 10.4 File Content

File contents should be transferred with the appropriate media type when NetixFS
can determine it. When the media type cannot be determined safely, NetixFS must
use `application/octet-stream`. Metadata, directory listings, operation
controls, and errors must use JSON.

NetixFS should use an implementation-documented MIME detection mechanism, such
as `libmagic` or an equivalent maintained MIME database. MIME detection may use
file signatures, magic rules, and file extensions. MIME detection must be used
only to select the HTTP `Content-Type`; it must not be used as an authorization
or security boundary.

For file reads, the HTTP `Accept` header is only a response negotiation hint
from the client. It can express that the client prefers a representation such as
`text/plain`, `application/json`, or `application/octet-stream`, but it must not
force NetixFS to reinterpret arbitrary file bytes as text or as another media
type. NetixFS should select the response `Content-Type` from MIME detection and
fall back to `application/octet-stream` when detection is unavailable or
uncertain.

If the client sends a restrictive `Accept` header that excludes the detected or
fallback media type, NetixFS may return `406 Not Acceptable`. Clients that can
handle any file content should omit `Accept` or send `Accept: */*`.

### 10.5 Preconditions

Conditional write requests should support standard HTTP precondition headers
where useful. These headers let clients avoid overwriting a file or directory
entry that changed after the client last observed it.

NetixFS should support:

- `If-Match`: perform the write only if the current target ETag matches one of
  the supplied ETags. This is the preferred guard for replacing or truncating a
  file that a client previously read.
- `If-None-Match`: perform the write only if the current target ETag does not
  match one of the supplied ETags. `If-None-Match: *` should be supported for
  create-if-absent workflows.
- `If-Unmodified-Since`: perform the write only if the target has not been
  modified since the supplied HTTP date. This is less precise than ETags and
  should be treated as a fallback compatibility mechanism.

When a precondition is supplied and does not match the current filesystem state,
NetixFS must fail the request with `412 Precondition Failed` and must not apply
the write.

An ETag is an HTTP validator for the filesystem object version observed by the
client. NetixFS may generate ETags from filesystem metadata that is cheap to
read and changes when normal filesystem writes occur. A typical file ETag should
include the device ID, inode number, size, modification time, and change time,
using the highest timestamp precision available from the filesystem, for
example:

```text
"{device_id}-{inode}-{size}-{mtime_ns}-{ctime_ns}"
```

ETags derived from metadata are validators, not cryptographic content hashes.
They are intended to detect ordinary concurrent modifications without reading
and hashing the full file contents. NetixFS must not promise stronger
consistency than Linux and the mounted filesystem provide, and should document
that metadata-derived ETags can have filesystem-specific edge cases.

### 10.6 API Errors

All filesystem API endpoints may return the following common errors unless an
endpoint documents a more specific rule:

- `400 Bad Request`: the request is syntactically invalid, including malformed
  query parameters, malformed JSON, invalid `path` or `path_b64`, both `path`
  and `path_b64` supplied for the same path, unsupported path components,
  invalid mode, invalid offset, invalid size, or invalid cursor.
- `401 Unauthorized`: authentication failed, including a missing bearer token,
  invalid JWT, expired JWT, rejected issuer, rejected audience, or failed
  signature validation.
- `403 Forbidden`: the request is authenticated but not allowed, including
  kernel permission denial for the resolved local identity, disabled operation,
  read-only mode violation, or service safety policy rejection.
- `404 Not Found`: the selected root or target path is not found, or NetixFS
  intentionally hides existence to avoid leaking information across
  authorization boundaries.
- `409 Conflict`: the requested operation conflicts with current filesystem
  state, including existing destination, non-empty directory, type mismatch,
  symlink policy conflict, or mount-boundary conflict.
- `413 Content Too Large`: the request body or non-streaming response would
  exceed a configured size limit.
- `415 Unsupported Media Type`: the endpoint does not accept the request body
  media type.
- `429 Too Many Requests`: worker, stream, or operation concurrency limits are
  exhausted.
- `500 Internal Server Error`: an unexpected internal failure occurred.
- `503 Service Unavailable`: a required worker process, identity lookup, key
  source, or filesystem dependency is temporarily unavailable.
- `504 Gateway Timeout`: the operation exceeded a configured request or stream
  setup timeout.

All non-`HEAD` error responses must use the structured JSON error model below.
For `HEAD` requests, NetixFS must return only the status and headers required by
HTTP.

Structured errors should preserve the following fields when doing so is safe:

- machine-readable error code;
- human-readable message;
- HTTP status code;
- operation name;
- normalized target path or opaque path identifier;
- POSIX errno category, when applicable;
- whether the request is retryable.

Important error mappings:

- Invalid JWT, expired JWT, rejected issuer, rejected audience, or failed
  signature validation maps to `401 Unauthorized`.
- Valid identity with a disallowed operation maps to `403 Forbidden`.
- `EACCES` and `EPERM` map to `403 Forbidden`.
- `ENOENT` maps to `404 Not Found`.
- `EEXIST` maps to `409 Conflict`.
- `ENOTDIR` and `EISDIR` map to `400 Bad Request` or `409 Conflict`, depending
  on operation context.
- Failed HTTP preconditions map to `412 Precondition Failed`.
- Unsatisfied byte ranges map to `416 Range Not Satisfiable`.
- Request or response size limit failures map to `413 Content Too Large`.
- Unsupported request media types map to `415 Unsupported Media Type`.
- Unsupported response representations map to `406 Not Acceptable`.
- Concurrency limit failures map to `429 Too Many Requests`.
- Unsupported filesystem features map to `501 Not Implemented`.
- Insufficient filesystem space maps to `507 Insufficient Storage`.

Example validation error:

```http
HTTP/1.1 400 Bad Request
Content-Type: application/json

{
  "error": {
    "code": "invalid_path",
    "message": "Path must be relative and must not contain '..' components.",
    "status": 400,
    "operation": "stat",
    "path": "projects/../secret.txt",
    "errno": null,
    "retryable": false,
    "request_id": "req_01HY9Z4Q8KVJ9Q2T8Q3J7XK9VM"
  }
}
```

Example POSIX error:

```http
HTTP/1.1 403 Forbidden
Content-Type: application/json

{
  "error": {
    "code": "permission_denied",
    "message": "The resolved local user cannot read the requested file.",
    "status": 403,
    "operation": "read_file",
    "path": "projects/report.txt",
    "errno": "EACCES",
    "retryable": false,
    "request_id": "req_01HY9Z55JWQHZJ4CK6V0M4JZ9D"
  }
}
```

Example retryable service error:

```http
HTTP/1.1 503 Service Unavailable
Content-Type: application/json
Retry-After: 2

{
  "error": {
    "code": "pool_unavailable",
    "message": "No worker process is currently available for this identity.",
    "status": 503,
    "operation": "replace_file",
    "path": "projects/report.txt",
    "errno": null,
    "retryable": true,
    "request_id": "req_01HY9Z5AZZSQPK9VC1M0C2B6SF"
  }
}
```

### 10.7 Cross-Origin Resource Sharing (CORS)

NetixFS must support Cross-Origin Resource Sharing (CORS) so browser-based
clients, such as single-page applications and internal file UIs, can call the
HTTP API from a different origin than the API host. CORS is optional and
disabled by default. Configuration is defined in
[section 12](#12-configuration).

#### 10.7.1 Purpose and threat model

CORS is a browser enforcement mechanism. It tells the browser whether
JavaScript on one origin may read HTTP responses from another origin.

When `cors.enabled=true`, an allowed frontend origin can cause a user's browser
to send API requests that include a Bearer token the frontend obtained. CORS
does not create new server-side privileges, but it can increase browser-visible
attack surface if tokens are stored or handled insecurely in the frontend.
Operators must not treat CORS as a substitute for network policy, firewall
rules, mTLS, or binding the API to trusted interfaces.

#### 10.7.2 Enablement and defaults

When `cors.enabled=false`, NetixFS must not emit `Access-Control-*` response
headers. Cross-origin browser reads will fail in the browser; same-origin
browser clients, command-line tools, and server-side SDKs are unaffected.

When `cors.enabled=true`, NetixFS must fail closed at startup if
`cors.allowed_origins` is empty. Wildcard `Access-Control-Allow-Origin: *` must
not be used when `cors.allow_credentials=true`.

Implementations may load and validate `cors.*` settings before CORS runtime
support is implemented. Until CORS is implemented, as described in
[section 16](#16-implementation-plan) step 13, NetixFS must not emit
`Access-Control-*` headers or handle CORS preflight even if `cors.enabled=true`.

#### 10.7.3 Scope (routes and listeners)

When `cors.enabled=true`, CORS applies only on the main HTTP API listener and
only to versioned filesystem routes:

```text
/api/v1/**
```

This includes all methods documented in [section 11](#11-http-api-endpoints),
including `OPTIONS` used for preflight.

CORS must not apply by default to:

- `GET /healthz` and `GET /readyz`;
- the metrics listener configured by `metrics.bind_address`;
- the runtime configuration listener configured by
  `diagnostics.config_endpoint.bind_address`.

`OPTIONS` requests outside `/api/v1/**` must return `404 Not Found` or
`405 Method Not Allowed` without `Access-Control-*` headers.

#### 10.7.4 Request classification (preflight vs actual)

A request is a CORS request when it includes an `Origin` header. If `Origin` is
absent, NetixFS must not add `Access-Control-*` headers.

A request is a CORS preflight when it is `OPTIONS`, includes `Origin`, and
includes `Access-Control-Request-Method`. NetixFS must handle preflight on
`/api/v1/**` without dispatching work to workers, without validating a JWT, and
without counting the request against `limits.max_concurrent_requests`.

#### 10.7.5 Origin validation

NetixFS must match the request `Origin` against `cors.allowed_origins`. Each
configured entry must be one of:

- an exact origin string, for example `https://app.example.com`, including
  scheme, host, and port when the port is not the default for the scheme;
- a single-subdomain wildcard origin, for example `https://*.example.com`, which
  matches `https://app.example.com` but not `https://evil.app.example.com` or
  bare `https://example.com`.

NetixFS must reject configuration entries that are only `*`, lack a scheme, or
are otherwise ambiguous.

When the origin is allowed, NetixFS must reflect the request `Origin` value in
`Access-Control-Allow-Origin`. NetixFS must never echo a disallowed origin in
that header. NetixFS must include `Vary: Origin` on responses when
`Access-Control-Allow-Origin` is reflected.

If `Origin` is present but not allowed:

- For preflight, NetixFS must return `403 Forbidden` without
  `Access-Control-Allow-Origin` and without echoing the disallowed origin.
- For actual requests, NetixFS must process the request normally, including JWT
  validation, but must omit `Access-Control-Allow-Origin` so the browser hides
  the response body from JavaScript.

#### 10.7.6 Response headers (success and error)

When `cors.enabled=true` and the request `Origin` is allowed, NetixFS must add
CORS headers to the response after the status code is known. This includes error
responses such as `401 Unauthorized`, `403 Forbidden`, `412 Precondition
Failed`, `429 Too Many Requests`, and `5xx` responses, so browser clients can
read structured JSON error bodies from JavaScript.

For allowed preflight requests, NetixFS must return `204 No Content` with:

- `Access-Control-Allow-Origin`: reflected allowed origin;
- `Access-Control-Allow-Methods`: configured or built-in method list;
- `Access-Control-Allow-Headers`: configured plus built-in request header list;
- `Access-Control-Max-Age`: from `cors.max_age`, unless `cors.max_age` is zero,
  in which case NetixFS must omit `Access-Control-Max-Age`;
- `Vary: Origin`.

For allowed actual requests, NetixFS must include at least:

- `Access-Control-Allow-Origin`: reflected allowed origin;
- `Access-Control-Expose-Headers`: configured plus built-in exposed header
  list;
- `Vary: Origin`.

When `cors.allow_credentials=true` and the origin is allowed, NetixFS must also
send `Access-Control-Allow-Credentials: true`. NetixFS must not use `*` for
`Access-Control-Allow-Origin` when credentials are allowed.

Built-in allowed request headers, merged with `cors.allowed_request_headers`
without duplicates:

- `Authorization`
- `Content-Type`
- `Accept`
- `Range`
- `If-Match`
- `If-None-Match`
- `If-Unmodified-Since`
- `X-Request-Id`

Built-in allowed methods unless overridden by `cors.allowed_methods`:

- `GET`, `HEAD`, `POST`, `PUT`, `PATCH`, `DELETE`, `OPTIONS`

Built-in exposed response headers, merged with `cors.exposed_response_headers`
without duplicates:

- `ETag`
- `Content-Range`
- `Content-Length`
- `Content-Type`
- `X-Request-Id`

#### 10.7.7 Interaction with JWT authentication

CORS preflight must not require `Authorization` and must not perform JWT
validation.

#### 10.7.8 Private Network Access (optional)

When `cors.allow_private_network=true` and CORS is enabled, NetixFS may support
Private Network Access for browser requests that include
`Access-Control-Request-Private-Network: true` on preflight. In that case,
NetixFS should respond with `Access-Control-Allow-Private-Network: true` when
the origin is allowed. This is optional and primarily useful for local
development, such as a public `https://` page calling `http://127.0.0.1:8080`.

#### 10.7.9 Examples

Example allowed preflight:

```http
OPTIONS /api/v1/roots/home/file?path=report.txt HTTP/1.1
Host: netixfs.example.com
Origin: https://app.example.com
Access-Control-Request-Method: PUT
Access-Control-Request-Headers: authorization, content-type
```

```http
HTTP/1.1 204 No Content
Access-Control-Allow-Origin: https://app.example.com
Access-Control-Allow-Methods: GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS
Access-Control-Allow-Headers: Authorization, Content-Type, Accept, Range, If-Match, If-None-Match, If-Unmodified-Since, X-Request-Id
Access-Control-Max-Age: 600
Vary: Origin
```

Example cross-origin read with Bearer token on the actual request:

```http
GET /api/v1/roots/home/file?path=report.txt HTTP/1.1
Host: netixfs.example.com
Origin: https://app.example.com
Authorization: Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9...
```

```http
HTTP/1.1 200 OK
Access-Control-Allow-Origin: https://app.example.com
Access-Control-Expose-Headers: ETag, Content-Range, Content-Length, Content-Type, X-Request-Id
Vary: Origin
Content-Type: application/octet-stream
ETag: "64769-734221-18432-1779097333"

<file bytes>
```

Example disallowed origin preflight:

```http
HTTP/1.1 403 Forbidden
Vary: Origin
```

Example `401 Unauthorized` with CORS so JavaScript can read the JSON body:

```http
HTTP/1.1 401 Unauthorized
Access-Control-Allow-Origin: https://app.example.com
Access-Control-Expose-Headers: ETag, Content-Range, Content-Length, Content-Type, X-Request-Id
Vary: Origin
Content-Type: application/json

{
  "error": {
    "code": "unauthorized",
    "message": "Missing or invalid bearer token.",
    "status": 401,
    "operation": "read_file",
    "path": "projects/report.txt",
    "errno": null,
    "retryable": false,
    "request_id": "req_01HY9Z55JWQHZJ4CK6V0M4JZ9D"
  }
}
```

When `cors.enabled=false`, browsers block cross-origin reads in JavaScript even
if the server would otherwise return `200 OK` to a non-browser client.

## 11. HTTP API Endpoints

### 11.1 Stat API

#### 11.1.1 Stat or lstat

`GET /api/v1/roots/{root_id}/stat`

Parameters:

- `path` or `path_b64` is required.
- `follow_symlinks=true|false` is optional and defaults according to the
  configured symlink policy.

Example request:

```sh
curl -sS \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  "https://netixfs.example.com/api/v1/roots/home/stat?path=projects/report.txt&follow_symlinks=false"
```

Example response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "path": "projects/report.txt",
  "type": "file",
  "size": 18432,
  "mode": "0640",
  "uid": 1000,
  "gid": 1000,
  "mtime": "2026-05-18T09:42:13Z",
  "ctime": "2026-05-18T09:42:13Z",
  "atime": "2026-05-18T10:05:01Z",
  "inode": 734221,
  "device_id": 64769,
  "etag": "\"64769-734221-18432-1779097333\""
}
```

Endpoint-specific errors:

- `400 Bad Request`: target path syntax is invalid or `follow_symlinks` is not a
  boolean.
- `404 Not Found`: target path does not exist or is hidden by authorization
  policy.
- `409 Conflict`: symlink handling or mount-boundary policy prevents resolving
  the target.

#### 11.1.2 Existence check

`HEAD /api/v1/roots/{root_id}/stat`

Parameters:

- `path` or `path_b64` is required.
- `follow_symlinks=true|false` is optional.

Example request:

```sh
curl -I \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  "https://netixfs.example.com/api/v1/roots/home/stat?path=projects/report.txt"
```

Example response:

```http
HTTP/1.1 200 OK
ETag: "64769-734221-18432-1779097333"
```

If the path is not visible or does not exist, the response is `404 Not Found`.
Other authorization or validation failures use the structured error model.

Endpoint-specific errors:

- `400 Bad Request`: target path syntax is invalid or `follow_symlinks` is not a
  boolean.
- `404 Not Found`: target path does not exist or is hidden by authorization
  policy.
- `409 Conflict`: symlink handling or mount-boundary policy prevents resolving
  the target.

#### 11.1.3 List extended attributes

`GET /api/v1/roots/{root_id}/xattrs`

Parameters:

- `path` or `path_b64` is required.
- `follow_symlinks=true|false` is optional.

Example request:

```sh
curl -sS \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  "https://netixfs.example.com/api/v1/roots/home/xattrs?path=projects/report.txt"
```

Example response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "attributes": ["user.comment", "user.checksum"]
}
```

Endpoint-specific errors:

- `400 Bad Request`: target path syntax is invalid or `follow_symlinks` is not a
  boolean.
- `404 Not Found`: target path does not exist.
- `409 Conflict`: target path is a type that cannot expose extended attributes,
  or symlink policy prevents resolving it.
- `501 Not Implemented`: the host filesystem or build does not support extended
  attributes.

#### 11.1.4 Read extended attribute

`GET /api/v1/roots/{root_id}/xattrs/{name}`

Parameters:

- `path` or `path_b64` is required.
- `follow_symlinks=true|false` is optional.
- `Accept: application/octet-stream` returns raw bytes.
- `Accept: application/json` returns a base64url-without-padding value.

Example request:

```sh
curl -sS \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  -H "Accept: application/json" \
  "https://netixfs.example.com/api/v1/roots/home/xattrs/user.comment?path=projects/report.txt"
```

Example response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "name": "user.comment",
  "value_b64": "UmV2aWV3ZWQ"
}
```

Endpoint-specific errors:

- `400 Bad Request`: attribute name is invalid, target path syntax is invalid,
  or response negotiation is invalid.
- `404 Not Found`: target path or extended attribute name does not exist.
- `406 Not Acceptable`: the requested response representation is unsupported.
- `409 Conflict`: target path is a type that cannot expose extended attributes,
  or symlink policy prevents resolving it.
- `501 Not Implemented`: the host filesystem or build does not support extended
  attributes.

POSIX ACLs must not be exposed through the extended attribute API.

### 11.2 Directory API

#### 11.2.1 List directory

`GET /api/v1/roots/{root_id}/dir`

Parameters:

- `path` or `path_b64` is required. The root directory may be requested with an
  empty `path`.
- `limit` is optional and must not exceed the configured maximum.
- `cursor` is optional and continues a previous listing.

Each returned entry must include the information visible in `ls -l`: entry
type, permission mode, hard-link count, owner, group, size, modification time,
and name. Entries must include the inode number. Entries should also include
numeric UID and GID when available.
Symbolic link entries must include the link target without following the link.
Regular file entries must include `mime_type`, using the same MIME detection
rules defined in [section 10](#10-http-api-design). When NetixFS cannot
determine a more specific media type for a regular file, `mime_type` must be
`application/octet-stream`.

Directory listings must not include `.` or `..` inside the `entries` array.
Instead, the response must include separate `current` and `parent` objects.
These objects use the same metadata shape as directory entries, represent the
requested directory and its parent directory, and are not affected by `limit` or
`cursor`. For a configured root directory, `parent` must be `null` because the
API must not expose a directory above the selected root.

Objects **`current`**, **`parent`** when present, and each element of **`entries`**
must include a string field **`url`**: a ready-to-follow HTTP URL for the primary
API resource for that row. Interpretation follows the existing **`type`** field:

- **`directory`**: **`url`** is [`GET …/dir`](#1121-list-directory) for that path.
- **`file`**: **`url`** is [`GET …/file`](#1131-read-file) for file contents.
- **`symlink`**: **`url`** is [`GET …/symlink`](#1141-read-symbolic-link) for symlink metadata JSON.

Each **`url`** must be either:

- **Absolute URL** when **`server.public_base_url`** is configured: scheme and
  host must match **`public_base_url`**; the path and query encode the `/api/v1`
  filesystem route (`/api/v1/roots/{root_id}/dir`, `/file`, or `/symlink`) and,
  unless the conventions in [section 10.3](#103-path-normalization) require **`path_b64`**, the **`path`**
  query parameter with correct percent-encoding; or
- **Path-absolute URL** when **`public_base_url`** is omitted: same path and query
  as above but starting at **`/api/v1/`** (clients resolve against their request origin).

Implementations derive the canonical relative POSIX path encoded in **`path`** /
**`path_b64`** inside **`url`** as follows:

- **`current`** and **`parent`** use their own **`path`** field from the listing
  response.
- **`entries[]`** entries use **`{listing_path}/{name}`**, where **`listing_path`**
  is the top-level **`path`** property in this directory response (**`entry.name`**
  only, with no slash when **`listing_path`** is empty). If **`path_b64`**
  is required for that joined path under [section 10.3](#103-path-normalization),
  **`url`** must use **`path_b64`** on the query string rather than **`path`**,
  omitting **`path`** in the URL entirely for that segment.

For **`type: "symlink"`**, **`url`** must target the symlink inode itself
(**`GET /symlink`** semantics), consistent with **`link_target`** in the listing
and without implying any follow-policy override.

Example request:

```sh
curl -sS \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  "https://netixfs.example.com/api/v1/roots/home/dir?path=projects&limit=2"
```

Example response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "path": "projects",
  "current": {
    "name": ".",
    "path": "projects",
    "inode": 734299,
    "type": "directory",
    "mode": "0750",
    "permissions": "rwxr-x---",
    "hard_links": 4,
    "uid": 1000,
    "owner": "alice",
    "gid": 1000,
    "group": "alice",
    "size": 4096,
    "mtime": "2026-05-18T08:00:00Z",
    "url": "https://netixfs.example.com/api/v1/roots/home/dir?path=projects"
  },
  "parent": {
    "name": "..",
    "path": "",
    "inode": 734100,
    "type": "directory",
    "mode": "0755",
    "permissions": "rwxr-xr-x",
    "hard_links": 6,
    "uid": 1000,
    "owner": "alice",
    "gid": 1000,
    "group": "alice",
    "size": 4096,
    "mtime": "2026-05-18T07:30:00Z",
    "url": "https://netixfs.example.com/api/v1/roots/home/dir?path="
  },
  "entries": [
    {
      "name": "archive",
      "inode": 734300,
      "type": "directory",
      "mode": "0750",
      "permissions": "rwxr-x---",
      "hard_links": 3,
      "uid": 1000,
      "owner": "alice",
      "gid": 1000,
      "group": "alice",
      "size": 4096,
      "mtime": "2026-05-18T08:30:00Z",
      "url": "https://netixfs.example.com/api/v1/roots/home/dir?path=projects%2Farchive"
    },
    {
      "name": "current",
      "inode": 734301,
      "type": "symlink",
      "mode": "0777",
      "permissions": "rwxrwxrwx",
      "hard_links": 1,
      "uid": 1000,
      "owner": "alice",
      "gid": 1000,
      "group": "alice",
      "size": 12,
      "mtime": "2026-05-18T09:00:00Z",
      "link_target": "projects/2026",
      "url": "https://netixfs.example.com/api/v1/roots/home/symlink?path=projects%2Fcurrent"
    }
  ],
  "next_cursor": "eyJsYXN0IjoiYXJjaGl2ZSJ9"
}
```

Example cursor request:

```sh
curl -sS \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  "https://netixfs.example.com/api/v1/roots/home/dir?path=projects&limit=2&cursor=eyJsYXN0IjoiYXJjaGl2ZSJ9"
```

Example cursor response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "path": "projects",
  "current": {
    "name": ".",
    "path": "projects",
    "inode": 734299,
    "type": "directory",
    "mode": "0750",
    "permissions": "rwxr-x---",
    "hard_links": 4,
    "uid": 1000,
    "owner": "alice",
    "gid": 1000,
    "group": "alice",
    "size": 4096,
    "mtime": "2026-05-18T08:00:00Z",
    "url": "https://netixfs.example.com/api/v1/roots/home/dir?path=projects"
  },
  "parent": {
    "name": "..",
    "path": "",
    "inode": 734100,
    "type": "directory",
    "mode": "0755",
    "permissions": "rwxr-xr-x",
    "hard_links": 6,
    "uid": 1000,
    "owner": "alice",
    "gid": 1000,
    "group": "alice",
    "size": 4096,
    "mtime": "2026-05-18T07:30:00Z",
    "url": "https://netixfs.example.com/api/v1/roots/home/dir?path="
  },
  "entries": [
    {
      "name": "report.txt",
      "inode": 734221,
      "type": "file",
      "mode": "0640",
      "permissions": "rw-r-----",
      "hard_links": 1,
      "uid": 1000,
      "owner": "alice",
      "gid": 1000,
      "group": "alice",
      "size": 18432,
      "mime_type": "text/plain; charset=utf-8",
      "mtime": "2026-05-18T09:42:13Z",
      "url": "https://netixfs.example.com/api/v1/roots/home/file?path=projects%2Freport.txt"
    }
  ],
  "next_cursor": null
}
```

Endpoint-specific errors:

- `400 Bad Request`: target path syntax, `limit`, or `cursor` is invalid.
- `404 Not Found`: directory path does not exist.
- `409 Conflict`: target path is not a directory, symlink policy prevents
  resolving it, or mount-boundary policy rejects traversal.

#### 11.2.2 Create directory

`POST /api/v1/roots/{root_id}/dir`

Request body fields:

- `path` or `path_b64` is required.
- `mode` is optional.
- `parents=false` is optional.

Example request:

```sh
curl -sS \
  -X POST \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"path":"projects/2026","mode":"0750","parents":true}' \
  "https://netixfs.example.com/api/v1/roots/home/dir"
```

Example response:

```http
HTTP/1.1 201 Created
Content-Type: application/json

{
  "path": "projects/2026",
  "type": "directory",
  "mode": "0750",
  "uid": 1000,
  "gid": 1000
}
```

Endpoint-specific errors:

- `400 Bad Request`: request body is invalid, target path syntax is invalid,
  `mode` is invalid, or `parents` is not a boolean.
- `403 Forbidden`: directory creation is blocked by read-only mode.
- `404 Not Found`: parent directory does not exist when `parents=false`.
- `409 Conflict`: target already exists, a parent component is not a directory,
  or symlink or mount-boundary policy rejects traversal.

#### 11.2.3 Remove directory

`DELETE /api/v1/roots/{root_id}/dir`

Parameters:

- `path` or `path_b64` is required.
- `recursive=false` is optional and must be rejected unless recursive deletion
  is enabled in configuration.

Example request:

```sh
curl -i \
  -X DELETE \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  "https://netixfs.example.com/api/v1/roots/home/dir?path=projects/old&recursive=false"
```

Example response:

```http
HTTP/1.1 204 No Content
```

Endpoint-specific errors:

- `400 Bad Request`: target path syntax is invalid or `recursive` is not a
  boolean.
- `403 Forbidden`: deletion is blocked by read-only mode or recursive deletion
  is requested while disabled.
- `404 Not Found`: directory path does not exist.
- `409 Conflict`: target is not a directory, directory is not empty for
  non-recursive removal, or symlink or mount-boundary policy rejects traversal.

#### 11.2.4 Rename or move path

`POST /api/v1/roots/{root_id}/rename`

Request body fields:

- `source` and `destination` path objects are required.
- `replace=false` is optional.

Example request:

```sh
curl -sS \
  -X POST \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"source":{"path":"projects/draft.txt"},"destination":{"path":"projects/report.txt"},"replace":true}' \
  "https://netixfs.example.com/api/v1/roots/home/rename"
```

Example response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "path": "projects/report.txt",
  "type": "file",
  "size": 18432,
  "mode": "0640"
}
```

Endpoint-specific errors:

- `400 Bad Request`: request body is invalid, source or destination path syntax
  is invalid, or `replace` is not a boolean.
- `403 Forbidden`: rename is blocked by read-only mode.
- `404 Not Found`: source path or destination parent does not exist.
- `409 Conflict`: destination exists when `replace=false`, source and
  destination types conflict, a parent component is not a directory, or symlink
  or mount-boundary policy rejects traversal.
- `412 Precondition Failed`: a supplied HTTP precondition fails.

#### 11.2.5 Copy path

`POST /api/v1/roots/{root_id}/copy`

Request body fields:

- `source` and `destination` path objects are required.
- `recursive=false` is optional and must be rejected unless recursive copy is
  enabled in configuration.
- `replace=false` is optional.

Example request:

```sh
curl -sS \
  -X POST \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"source":{"path":"projects/report.txt"},"destination":{"path":"projects/report-copy.txt"},"replace":false}' \
  "https://netixfs.example.com/api/v1/roots/home/copy"
```

Example response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "path": "projects/report-copy.txt",
  "type": "file",
  "size": 18432,
  "mode": "0640"
}
```

Endpoint-specific errors:

- `400 Bad Request`: request body is invalid, source or destination path syntax
  is invalid, or `recursive` or `replace` is not a boolean.
- `403 Forbidden`: copy is blocked by read-only mode or recursive copy is
  requested while disabled.
- `404 Not Found`: source path or destination parent does not exist.
- `409 Conflict`: destination exists when `replace=false`, source is a directory
  and `recursive=false`, a parent component is not a directory, or symlink or
  mount-boundary policy rejects traversal.
- `412 Precondition Failed`: a supplied HTTP precondition fails.

### 11.3 File API

File API endpoints must transfer file contents as text when the content is
represented as text, and as `application/octet-stream` otherwise. Text responses
must declare an appropriate text media type and character set when known.

#### 11.3.1 Read file

`GET /api/v1/roots/{root_id}/file`

Parameters:

- `path` or `path_b64` is required.
- The standard `Range` header is optional.

Example text request:

```sh
curl -sS \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  -H "Accept: text/plain; charset=utf-8" \
  "https://netixfs.example.com/api/v1/roots/home/file?path=projects/notes.txt"
```

Example text response:

```http
HTTP/1.1 200 OK
Content-Type: text/plain; charset=utf-8
ETag: "64769-734222-37-1779097333"

Release notes for the current sprint.
```

Example octet-stream request:

```sh
curl -sS \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  -H "Range: bytes=0-1023" \
  "https://netixfs.example.com/api/v1/roots/home/file?path=projects/archive.tar"
```

Example octet-stream response:

```http
HTTP/1.1 206 Partial Content
Content-Type: application/octet-stream
Content-Range: bytes 0-1023/18432
ETag: "64769-734221-18432-1779097333"

<1024 bytes>
```

Endpoint-specific errors:

- `400 Bad Request`: target path syntax is invalid or the `Range` header is
  malformed.
- `404 Not Found`: file path does not exist.
- `409 Conflict`: target is not a regular file or symlink or mount-boundary
  policy rejects traversal.
- `416 Range Not Satisfiable`: requested byte range is outside the file.

#### 11.3.2 Stream appended data

`GET /api/v1/roots/{root_id}/stream`

Parameters:

- `path` or `path_b64` is required.
- `offset` is optional and is zero-based.
- `from_end=true` is optional and starts at the current end of file.

Example request:

```sh
curl -N \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  "https://netixfs.example.com/api/v1/roots/home/stream?path=logs/app.log&from_end=true"
```

Example response:

```http
HTTP/1.1 200 OK
Content-Type: text/plain; charset=utf-8
Transfer-Encoding: chunked

<newly appended UTF-8 text>
```

The stream must follow the opened file descriptor, equivalent to `tail -f`, even
if the path is later renamed or replaced.

Streaming must satisfy the following requirements:

- The streaming endpoint is restricted to text files.
- Path safeguards are identical to non-streaming file reads and must be applied
  before the stream starts.
- Before the stream starts, NetixFS must reject targets that are not eligible
  text files for streaming. The text-file detection policy must be deterministic
  and documented by the implementation.
- Once a stream starts, bytes are sent exactly as read from the file. NetixFS
  must not transform line endings, recode character data, or parse log formats.
- If NetixFS detects invalid text according to its text-file policy before
  response headers are sent, it must return `415 Unsupported Media Type`.
- If invalid text is detected after streaming has started, NetixFS must close
  the stream and log the reason with request and stream identifiers.
- Client disconnect must cancel the stream and release the worker, file
  descriptor, and any watcher resources.
- NetixFS should use Linux `inotify` to wait for file changes instead of
  polling. Polling may only be used as a documented fallback when `inotify`
  cannot be used for a specific filesystem or deployment environment.
- Backpressure from the HTTP connection must propagate to file reading, and
  NetixFS must not buffer unbounded data per client.
- Configured maximum concurrent streams, per-stream idle timeout, maximum stream
  duration, and optional heartbeat behavior must be enforced.
- If an error occurs before response headers are sent, NetixFS must return a
  structured JSON error. If an error occurs after streaming has started, NetixFS
  must close the stream and log the reason with request and stream identifiers.
- If the followed file is truncated, NetixFS must continue from the new end of
  the same opened file descriptor.
- If the followed file is deleted or replaced, NetixFS must continue reading the
  opened descriptor until EOF and then wait for new data on that descriptor. It
  must not silently switch to the new path.

Endpoint-specific errors before response headers are sent:

- `400 Bad Request`: target path syntax, `offset`, or `from_end` is invalid.
- `404 Not Found`: file path does not exist.
- `409 Conflict`: target is not a regular file or symlink or mount-boundary
  policy rejects traversal.
- `415 Unsupported Media Type`: target is not eligible text for streaming.
- `429 Too Many Requests`: maximum concurrent streams is reached.

#### 11.3.3 Create or replace file

`PUT /api/v1/roots/{root_id}/file`

Parameters:

- `path` or `path_b64` is required.
- `mode` is optional.
- `create=always|if_missing|replace` is optional and controls whether the
  target may already exist. The default is `replace`.
- The request body must use a text media type when the client sends text
  content, and `application/octet-stream` otherwise.

When the request replaces an existing file, NetixFS must always perform the
replacement atomically by writing to a temporary file in the target directory and
then using atomic rename. Clients must not be able to opt into a non-atomic
replacement mode.

Example text request:

```sh
curl -sS \
  -X PUT \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  -H "Content-Type: text/plain; charset=utf-8" \
  --data-binary @notes.txt \
  "https://netixfs.example.com/api/v1/roots/home/file?path=projects/notes.txt&mode=0640"
```

Example text response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "path": "projects/notes.txt",
  "type": "file",
  "size": 37,
  "mode": "0640"
}
```

Example octet-stream request:

```sh
curl -sS \
  -X PUT \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  -H "Content-Type: application/octet-stream" \
  --data-binary @archive.tar \
  "https://netixfs.example.com/api/v1/roots/home/file?path=projects/archive.tar&mode=0640"
```

Example octet-stream response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "path": "projects/archive.tar",
  "type": "file",
  "size": 18432,
  "mode": "0640"
}
```

Endpoint-specific errors:

- `400 Bad Request`: target path syntax, `mode`, or `create` mode is invalid.
- `403 Forbidden`: write is blocked by read-only mode.
- `404 Not Found`: parent directory does not exist.
- `409 Conflict`: target exists when `create=if_missing`, target does not exist
  when `create=replace`, a parent component is not a directory, target is not a
  regular file, or symlink or mount-boundary policy rejects traversal.
- `412 Precondition Failed`: `If-Match`, `If-None-Match`, or
  `If-Unmodified-Since` fails.
- `413 Content Too Large`: request body exceeds the configured write limit.
- `415 Unsupported Media Type`: request body media type is not accepted for the
  supplied file content.
- `507 Insufficient Storage`: the filesystem cannot allocate space for the
  temporary file or atomic rename.

#### 11.3.4 Delete file

`DELETE /api/v1/roots/{root_id}/file`

Parameters:

- `path` or `path_b64` is required.

Example request:

```sh
curl -i \
  -X DELETE \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  "https://netixfs.example.com/api/v1/roots/home/file?path=projects/old-report.txt"
```

Example response:

```http
HTTP/1.1 204 No Content
```

Endpoint-specific errors:

- `400 Bad Request`: target path syntax is invalid.
- `403 Forbidden`: deletion is blocked by read-only mode.
- `404 Not Found`: file path does not exist.
- `409 Conflict`: target is a directory, parent component is not a directory, or
  symlink or mount-boundary policy rejects traversal.
- `412 Precondition Failed`: a supplied HTTP precondition fails.

`PUT /file` replaces or creates the target according to the `create` mode.
Replacement through `PUT /file` must write to a temporary file in the target
directory and then use atomic rename so readers never observe a partially
written replacement.
Request body size and non-streaming read size limits must be enforced.

### 11.4 Link API

#### 11.4.1 Read symbolic link

`GET /api/v1/roots/{root_id}/symlink`

Parameters:

- `path` or `path_b64` is required.

Example request:

```sh
curl -sS \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  "https://netixfs.example.com/api/v1/roots/home/symlink?path=projects/current"
```

Example response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "path": "projects/current",
  "target": "projects/2026"
}
```

Endpoint-specific errors:

- `400 Bad Request`: target path syntax is invalid.
- `404 Not Found`: symlink path does not exist.
- `409 Conflict`: target path is not a symbolic link or mount-boundary policy
  rejects traversal to the link parent.

#### 11.4.2 Create symbolic link

`POST /api/v1/roots/{root_id}/symlink`

Request body fields:

- `path` or `path_b64` is required for the link location.
- `target` is required and is the symlink target bytes interpreted by Linux.

Example request:

```sh
curl -sS \
  -X POST \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"path":"projects/current","target":"projects/2026"}' \
  "https://netixfs.example.com/api/v1/roots/home/symlink"
```

Example response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "path": "projects/current",
  "type": "symlink",
  "target": "projects/2026"
}
```

Endpoint-specific errors:

- `400 Bad Request`: request body is invalid, link path syntax is invalid, or
  link target is invalid.
- `403 Forbidden`: symbolic link creation is disabled or blocked by read-only
  mode.
- `404 Not Found`: link parent directory does not exist.
- `409 Conflict`: link path already exists, a parent component is not a
  directory, or mount-boundary policy rejects traversal.

#### 11.4.3 Create hard link

`POST /api/v1/roots/{root_id}/hardlink`

Request body fields:

- `source` and `destination` path objects are required.

Example request:

```sh
curl -sS \
  -X POST \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"source":{"path":"projects/report.txt"},"destination":{"path":"projects/report-link.txt"}}' \
  "https://netixfs.example.com/api/v1/roots/home/hardlink"
```

Example response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "path": "projects/report-link.txt",
  "type": "file",
  "size": 18432,
  "inode": 734221
}
```

Endpoint-specific errors:

- `400 Bad Request`: request body is invalid or source or destination path
  syntax is invalid.
- `403 Forbidden`: hard-link creation is disabled or blocked by read-only mode.
- `404 Not Found`: source path or destination parent does not exist.
- `409 Conflict`: destination already exists, source is a directory, source and
  destination are on different devices, a parent component is not a directory,
  or symlink or mount-boundary policy rejects traversal.

Symbolic link creation and hard-link creation must be controlled by
configuration. Symlink traversal behavior for all APIs must follow the
configured symlink policy.

### 11.5 Permissions API

#### 11.5.1 Change mode

`PATCH /api/v1/roots/{root_id}/mode`

Request body fields:

- `path` or `path_b64` is required.
- `mode` is required and must be an octal mode string.

Example request:

```sh
curl -sS \
  -X PATCH \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"path":"projects/report.txt","mode":"0640"}' \
  "https://netixfs.example.com/api/v1/roots/home/mode"
```

Example response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "path": "projects/report.txt",
  "type": "file",
  "mode": "0640"
}
```

Endpoint-specific errors:

- `400 Bad Request`: request body is invalid, target path syntax is invalid, or
  `mode` is invalid.
- `403 Forbidden`: chmod is disabled, blocked by read-only mode, or denied by
  filesystem permissions.
- `404 Not Found`: target path does not exist.
- `409 Conflict`: symlink or mount-boundary policy rejects traversal.
- `412 Precondition Failed`: a supplied HTTP precondition fails.

#### 11.5.2 Change group

`PATCH /api/v1/roots/{root_id}/group`

Request body fields:

- `path` or `path_b64` is required.
- Either `gid` or `group` is required.

Example request:

```sh
curl -sS \
  -X PATCH \
  -H "Authorization: Bearer $NETIXFS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"path":"projects/report.txt","group":"engineers"}' \
  "https://netixfs.example.com/api/v1/roots/home/group"
```

Example response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "path": "projects/report.txt",
  "type": "file",
  "gid": 1001,
  "group": "engineers"
}
```

Endpoint-specific errors:

- `400 Bad Request`: request body is invalid, target path syntax is invalid, or
  neither `gid` nor `group` is supplied.
- `403 Forbidden`: chgrp is disabled, blocked by read-only mode, or denied by
  filesystem permissions.
- `404 Not Found`: target path or named local group does not exist.
- `409 Conflict`: supplied `gid` and `group` disagree, or symlink or
  mount-boundary policy rejects traversal.
- `412 Precondition Failed`: a supplied HTTP precondition fails.

`chmod` must be controlled by configuration. `chgrp` availability must be
controlled by configuration when the implementation supports disabling it.

NetixFS must not support ownership changes through `chown`.
Ownership changes are especially sensitive and would require a separate threat
model before being considered for a later release.

NetixFS must not support POSIX ACL read or write operations. POSIX ACLs are out
of scope even on filesystems where they are represented through extended
attributes.

## 12. Configuration

NetixFS must support configuration through:

- a TOML configuration file;
- environment variables;
- command-line arguments.

### 12.1 Source Ordering

Configuration precedence must be, from lowest to highest:

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
NetixFS should either use documented defaults or exit with a configuration error
when a required setting has no value.

### 12.2 Configuration validation and startup

NetixFS must load, merge, and validate configuration from the TOML file (when
used), environment variables, and command-line arguments **atomically**. Either
every recognized setting has an accepted effective value after merge, or startup
fails. Rejected inputs must not be applied and must not be reflected as
effective runtime state.

A **configuration error** is any failure during configuration load, merge, or
validation. Configuration errors include, but are not limited to:

- missing unconditional mandatory settings;
- unsatisfied documented setting groups (for example, one JWT key source
  required, TLS certificate and key when native TLS is enabled, or CORS allowed
  origins when CORS is enabled);
- invalid type, range, or format for any setting, including optional settings
  with explicit values;
- semantic validation failures (for example, duplicate filesystem root IDs or
  invalid origin strings);
- conflicting settings when the specification defines mutual exclusion;
- unrecoverable TOML load or parse errors, or an unreadable configuration file
  when `--config-file` or `NETIXFS_CONFIG_FILE` is set.

On any configuration error, NetixFS must exit with a non-zero status before
binding **any** HTTP listener, including the main API listener, the metrics
listener, and the runtime configuration (`/configz`) listener. Startup failure
messages must identify the setting and the error, but must not leak sensitive
rejected values such as tokens, inline secrets, or credentials.

The following are **not** configuration errors: the supervisor may start, but
`/readyz` may report not ready, when configuration is valid but a runtime
dependency is temporarily unavailable (for example, a configured JWT public key
path whose file is not yet readable, or a worker pool that has not finished
initializing).

Examples of configuration errors (all prevent startup):

| Scenario | Outcome |
|----------|---------|
| No `filesystem.allowed_roots` after merge | Exit; message cites missing mandatory setting. |
| No JWT key source configured | Exit; message cites unsatisfied group. |
| `tls.enabled=true` without `tls.cert_path` or `tls.key_path` | Exit. |
| `cors.enabled=true` with empty `cors.allowed_origins` | Exit. |
| `NETIXFS_SERVER_PORT=99999` | Exit; invalid value (do not start with the default port while discarding the supplied value). |
| `server.bind_address = "not-an-ip"` | Exit. |
| Invalid TOML or unreadable config file when a config file path is set | Exit. |


**Start** means the supervisor process is running with validated configuration
and has bound the listeners permitted by that configuration. **Ready** means
`GET /readyz` returns `200 OK` as defined in [section 13.5](#135-health-and-readiness).

### 12.3 Configuration Settings

For each setting, `Mandatory: Yes` means startup fails with a configuration
error if the setting, or the documented setting group, has no valid value after
merge. Any other configuration validation failure is treated the same way:
NetixFS must exit before binding HTTP listeners.

#### General

##### Configuration file path

Selects the TOML configuration file to load before environment variables and
command-line overrides are applied. If omitted, NetixFS must use built-in
defaults and fail fast for mandatory settings without values.

- TOML setting: not applicable.
- Command-line argument: `--config-file`.
- Environment variable: `NETIXFS_CONFIG_FILE`.
- Default value: none.
- Mandatory: no.
- Example supported values: `/etc/netixfs/config.toml`.

#### Server

##### HTTP bind address

Controls which local network address the main HTTP listener binds to. Loopback
limits access to the local host, while wildcard addresses expose the service on
network interfaces.

- TOML setting: `server.bind_address`.
- Command-line argument: `--bind-address`.
- Environment variable: `NETIXFS_SERVER_BIND_ADDRESS`.
- Default value: `127.0.0.1`.
- Mandatory: no.
- Example supported values: `127.0.0.1`, `0.0.0.0`, `::1`.

##### HTTP port

Controls the TCP port for the main HTTP listener. Ports below 1024 require
`CAP_NET_BIND_SERVICE` or equivalent service-manager support.

- TOML setting: `server.port`.
- Command-line argument: `--port`.
- Environment variable: `NETIXFS_SERVER_PORT`.
- Default value: `8080`.
- Mandatory: no.
- Example supported values: `8080`, `8443`.

##### Public base URL

Defines the externally visible base URL used in generated URLs (including **`url`**
on directory listing responses), documentation, redirects if any, and
operator-facing diagnostics. It must not control the listener address.

- TOML setting: `server.public_base_url`.
- Command-line argument: `--public-base-url`.
- Environment variable: `NETIXFS_SERVER_PUBLIC_BASE_URL`.
- Default value: none.
- Mandatory: no.
- Example supported values: `https://netixfs.example.com`.

#### TLS

##### TLS enabled

Enables native TLS in NetixFS. When disabled, operators must provide transport
security through an upstream TLS-terminating proxy or trusted deployment
boundary.

- TOML setting: `tls.enabled`.
- Command-line argument: `--tls-enabled`.
- Environment variable: `NETIXFS_TLS_ENABLED`.
- Default value: `false`.
- Mandatory: no.
- Example supported values: `true`, `false`.

##### TLS certificate path

Points to the certificate chain used by the native TLS listener. It is required
only when native TLS is enabled.

- TOML setting: `tls.cert_path`.
- Command-line argument: `--tls-cert-path`.
- Environment variable: `NETIXFS_TLS_CERT_PATH`.
- Default value: none.
- Mandatory: yes, if TLS enabled.
- Example supported values: `/etc/netixfs/tls.crt`.

##### TLS private key path

Points to the private key used by the native TLS listener. The key must be
readable by the supervisor service account and protected from other users.

- TOML setting: `tls.key_path`.
- Command-line argument: `--tls-key-path`.
- Environment variable: `NETIXFS_TLS_KEY_PATH`.
- Default value: none.
- Mandatory: yes, if TLS enabled.
- Example supported values: `/etc/netixfs/tls.key`.

#### CORS

##### CORS enabled

Enables Cross-Origin Resource Sharing on `/api/v1/**` of the main HTTP listener.
When disabled, NetixFS must not emit `Access-Control-*` headers.

- TOML setting: `cors.enabled`.
- Command-line argument: `--cors-enabled`.
- Environment variable: `NETIXFS_CORS_ENABLED`.
- Default value: `false`.
- Mandatory: no.
- Example supported values: `true`, `false`.

##### CORS allowed origins

Defines the browser origins allowed to read API responses when CORS is enabled.
Each value must be an exact origin or a single-subdomain wildcard origin as
defined in [section 10.7](#107-cross-origin-resource-sharing-cors). When
`cors.enabled=true`, this list must not be empty or NetixFS must fail closed at
startup.

- TOML setting: `cors.allowed_origins`.
- Command-line argument: `--cors-allowed-origin` (repeatable).
- Environment variable: `NETIXFS_CORS_ALLOWED_ORIGINS`.
- Default value: none (empty list).
- Mandatory: yes, when CORS enabled.
- Example supported values: `https://app.example.com`,
  `https://*.example.com`, `http://localhost:5173`.

##### CORS allowed methods

Overrides the built-in `Access-Control-Allow-Methods` list used for preflight.
When omitted, NetixFS must use the built-in list from
[section 10.7](#107-cross-origin-resource-sharing-cors).

- TOML setting: `cors.allowed_methods`.
- Command-line argument: `--cors-allowed-method` (repeatable).
- Environment variable: `NETIXFS_CORS_ALLOWED_METHODS`.
- Default value: built-in list (`GET`, `HEAD`, `POST`, `PUT`, `PATCH`,
  `DELETE`, `OPTIONS`).
- Mandatory: no.
- Example supported values: `GET`, `PUT`, `DELETE`.

##### CORS allowed request headers

Adds request header names to the built-in preflight allowlist. NetixFS must merge
these values with the built-in list from
[section 10.7](#107-cross-origin-resource-sharing-cors) without duplicates.

- TOML setting: `cors.allowed_request_headers`.
- Command-line argument: `--cors-allowed-request-header` (repeatable).
- Environment variable: `NETIXFS_CORS_ALLOWED_REQUEST_HEADERS`.
- Default value: none (built-in list only).
- Mandatory: no.
- Example supported values: `X-Custom-Client-Version`.

##### CORS exposed response headers

Adds response header names to the built-in `Access-Control-Expose-Headers` list.
NetixFS must merge these values with the built-in list from
[section 10.7](#107-cross-origin-resource-sharing-cors) without duplicates.

- TOML setting: `cors.exposed_response_headers`.
- Command-line argument: `--cors-exposed-response-header` (repeatable).
- Environment variable: `NETIXFS_CORS_EXPOSED_RESPONSE_HEADERS`.
- Default value: none (built-in list only).
- Mandatory: no.
- Example supported values: `Location`.

##### CORS preflight max age

Controls `Access-Control-Max-Age` for successful preflight responses. A value
of `0` means NetixFS must omit `Access-Control-Max-Age`.

- TOML setting: `cors.max_age`.
- Command-line argument: `--cors-max-age`.
- Environment variable: `NETIXFS_CORS_MAX_AGE`.
- Default value: `600s`.
- Mandatory: no.
- Example supported values: `0s`, `600s`, `10m`.

##### CORS allow credentials

When `true`, NetixFS must send `Access-Control-Allow-Credentials: true` for
allowed origins and must not use `*` for `Access-Control-Allow-Origin`.

- TOML setting: `cors.allow_credentials`.
- Command-line argument: `--cors-allow-credentials`.
- Environment variable: `NETIXFS_CORS_ALLOW_CREDENTIALS`.
- Default value: `false`.
- Mandatory: no.
- Example supported values: `true`, `false`.

##### CORS allow private network

When `true` and CORS is enabled, NetixFS may respond to Private Network Access
preflight requests with `Access-Control-Allow-Private-Network: true` for allowed
origins, as described in [section 10.7](#107-cross-origin-resource-sharing-cors).

- TOML setting: `cors.allow_private_network`.
- Command-line argument: `--cors-allow-private-network`.
- Environment variable: `NETIXFS_CORS_ALLOW_PRIVATE_NETWORK`.
- Default value: `false`.
- Mandatory: no.
- Example supported values: `true`, `false`.

#### JWT

##### JWT public key path

Loads a static JWT verification public key from the local filesystem. Use this
for deployments with manual key rotation and no JWKS endpoint.

- TOML setting: `auth.jwt.public_key_path`.
- Command-line argument: `--jwt-public-key-path`.
- Environment variable: `NETIXFS_AUTH_JWT_PUBLIC_KEY_PATH`.
- Default value: none.
- Mandatory: yes, one JWT key source required.
- Example supported values: `/etc/netixfs/jwt.pub`.

##### JWT public key URL

Loads a static JWT verification public key from a remote URL. This keeps key
material outside the local config but still represents a single-key validation
model.

- TOML setting: `auth.jwt.public_key_url`.
- Command-line argument: `--jwt-public-key-url`.
- Environment variable: `NETIXFS_AUTH_JWT_PUBLIC_KEY_URL`.
- Default value: none.
- Mandatory: yes, one JWT key source required.
- Example supported values: `https://idp.example.com/jwt.pub`.

##### JWT JWKS path

Loads a JWKS document from the local filesystem. JWKS supports multiple keys and
is preferred when key rotation must be represented locally.

- TOML setting: `auth.jwt.jwks_path`.
- Command-line argument: `--jwt-jwks-path`.
- Environment variable: `NETIXFS_AUTH_JWT_JWKS_PATH`.
- Default value: none.
- Mandatory: yes, one JWT key source required.
- Example supported values: `/etc/netixfs/jwks.json`.

##### JWT JWKS URL

Loads and refreshes a JWKS document from a remote identity provider. This is the
preferred mode for automatic key rotation.

- TOML setting: `auth.jwt.jwks_url`.
- Command-line argument: `--jwt-jwks-url`.
- Environment variable: `NETIXFS_AUTH_JWT_JWKS_URL`.
- Default value: none.
- Mandatory: yes, one JWT key source required.
- Example supported values: `https://idp.example.com/.well-known/jwks.json`.

##### JWT issuer

Defines the expected token issuer. When this setting is present, issuer
validation is enabled and every accepted token must contain a matching `iss`
claim. When this setting is absent, NetixFS must not validate the token issuer.
A mismatch must reject the token before identity mapping or filesystem access.

- TOML setting: `auth.jwt.issuer`.
- Command-line argument: `--jwt-issuer`.
- Environment variable: `NETIXFS_AUTH_JWT_ISSUER`.
- Default value: none.
- Mandatory: no.
- Example supported values: `https://idp.example.com/realms/main`.

##### JWT audience

Defines the expected token audience. When this setting is present, audience
validation is enabled and every accepted token must contain a matching `aud`
claim. When this setting is absent, NetixFS must not validate the token
audience. A mismatch must reject tokens minted for another service.

- TOML setting: `auth.jwt.audience`.
- Command-line argument: `--jwt-audience`.
- Environment variable: `NETIXFS_AUTH_JWT_AUDIENCE`.
- Default value: none.
- Mandatory: no.
- Example supported values: `netixfs`, `files-api`.

##### JWT username claim

Selects the JWT claim used as the local username before NSS resolution. Changing
it affects which local Linux account a valid token maps to.

- TOML setting: `auth.jwt.username_claim`.
- Command-line argument: `--jwt-username-claim`.
- Environment variable: `NETIXFS_AUTH_JWT_USERNAME_CLAIM`.
- Default value: `sub`.
- Mandatory: no.
- Example supported values: `sub`, `preferred_username`, `email`.

##### Remote key refresh interval

Controls how often remote JWT key sources are refreshed. Shorter intervals pick
up rotations faster but increase dependency traffic and failure surface. Values
should use a documented duration syntax such as `30s`, `5m`, or `1h`.

- TOML setting: `auth.jwt.remote_key_refresh_interval`.
- Command-line argument: `--jwt-remote-key-refresh-interval`.
- Environment variable: `NETIXFS_AUTH_JWT_REMOTE_KEY_REFRESH_INTERVAL`.
- Default value: `5m`.
- Mandatory: no.
- Example supported values: `30s`, `5m`, `1h`.

#### Filesystem

##### Allowed filesystem roots

Defines the named root directories exposed through
`/api/v1/roots/{root_id}`. Operators choose these roots, and all filesystem
paths are resolved relative to one of them. When supplied through
`NETIXFS_FILESYSTEM_ALLOWED_ROOTS`, entries should be comma-separated unless the
implementation defines a stricter format.

- TOML setting: `filesystem.allowed_roots`.
- Command-line argument: `--allowed-root`.
- Environment variable: `NETIXFS_FILESYSTEM_ALLOWED_ROOTS`.
- Default value: none.
- Mandatory: yes.
- Example supported values: `home=/srv/netixfs/home`, `logs=/var/log/app`.

##### Read-only mode

Disables filesystem write operations while keeping read-oriented APIs available.
This is useful for safe inspection deployments or emergency lock-down.

- TOML setting: `filesystem.read_only`.
- Command-line argument: `--read-only`.
- Environment variable: `NETIXFS_FILESYSTEM_READ_ONLY`.
- Default value: `false`.
- Mandatory: no.
- Example supported values: `true`, `false`.

##### Default file creation mode

Sets the base mode for newly created files when the request does not supply a
mode. The final mode is still affected by the configured or process umask.

- TOML setting: `filesystem.default_file_mode`.
- Command-line argument: `--default-file-mode`.
- Environment variable: `NETIXFS_FILESYSTEM_DEFAULT_FILE_MODE`.
- Default value: `0644`.
- Mandatory: no.
- Example supported values: `0600`, `0644`.

##### Default directory creation mode

Sets the base mode for newly created directories when the request does not
supply a mode. The final mode is still affected by the configured or process
umask.

- TOML setting: `filesystem.default_dir_mode`.
- Command-line argument: `--default-dir-mode`.
- Environment variable: `NETIXFS_FILESYSTEM_DEFAULT_DIR_MODE`.
- Default value: `0755`.
- Mandatory: no.
- Example supported values: `0700`, `0755`.

##### Default umask

Controls permission bits removed during file and directory creation. A stricter
umask reduces accidental exposure of newly created filesystem objects.

- TOML setting: `filesystem.umask`.
- Command-line argument: `--umask`.
- Environment variable: `NETIXFS_FILESYSTEM_UMASK`.
- Default value: process umask.
- Mandatory: no.
- Example supported values: `0022`, `0077`.

##### Symlink policy

Controls whether APIs reject symlink traversal or allow only
containment-checked traversal. This affects path resolution for most filesystem
operations.

- TOML setting: `filesystem.symlink_policy`.
- Command-line argument: `--symlink-policy`.
- Environment variable: `NETIXFS_FILESYSTEM_SYMLINK_POLICY`.
- Default value: `reject`.
- Mandatory: no.
- Example supported values: `reject`, `follow_safe`.

##### Allow mount crossing

Controls whether path resolution may cross filesystem mount boundaries under an
allowed root. Disabling it reduces bind-mount escape and accidental exposure
risks.

- TOML setting: `filesystem.allow_mount_crossing`.
- Command-line argument: `--allow-mount-crossing`.
- Environment variable: `NETIXFS_FILESYSTEM_ALLOW_MOUNT_CROSSING`.
- Default value: `false`.
- Mandatory: no.
- Example supported values: `true`, `false`.

#### Operations

##### Allow recursive delete

Enables recursive directory deletion. This is dangerous because one request can
remove large directory trees.

- TOML setting: `operations.allow_recursive_delete`.
- Command-line argument: `--allow-recursive-delete`.
- Environment variable: `NETIXFS_OPERATIONS_ALLOW_RECURSIVE_DELETE`.
- Default value: `true`.
- Mandatory: no.
- Example supported values: `true`, `false`.

##### Allow recursive copy

Enables recursive directory copy. This can consume large amounts of time, space,
and worker capacity.

- TOML setting: `operations.allow_recursive_copy`.
- Command-line argument: `--allow-recursive-copy`.
- Environment variable: `NETIXFS_OPERATIONS_ALLOW_RECURSIVE_COPY`.
- Default value: `true`.
- Mandatory: no.
- Example supported values: `true`, `false`.

##### Allow chmod

Enables mode changes through the Permissions API. Permission changes can broaden
or break access for other users.

- TOML setting: `operations.allow_chmod`.
- Command-line argument: `--allow-chmod`.
- Environment variable: `NETIXFS_OPERATIONS_ALLOW_CHMOD`.
- Default value: `true`.
- Mandatory: no.
- Example supported values: `true`, `false`.

##### Allow hard links

Enables hard-link creation. Hard links can make ownership, retention, and
deletion semantics harder to reason about.

- TOML setting: `operations.allow_hard_links`.
- Command-line argument: `--allow-hard-links`.
- Environment variable: `NETIXFS_OPERATIONS_ALLOW_HARD_LINKS`.
- Default value: `true`.
- Mandatory: no.
- Example supported values: `true`, `false`.

##### Allow symbolic link creation

Enables symbolic-link creation. Symlinks can complicate containment, auditing,
and later path resolution.

- TOML setting: `operations.allow_symlink_create`.
- Command-line argument: `--allow-symlink-create`.
- Environment variable: `NETIXFS_OPERATIONS_ALLOW_SYMLINK_CREATE`.
- Default value: `true`.
- Mandatory: no.
- Example supported values: `true`, `false`.

#### Limits

##### Maximum request body size

Caps accepted request bodies for write operations and JSON controls. Larger
requests are rejected before consuming unbounded memory or disk resources.

- TOML setting: `limits.max_request_body_size`.
- Command-line argument: `--max-request-body-size`.
- Environment variable: `NETIXFS_LIMITS_MAX_REQUEST_BODY_SIZE`.
- Default value: `100MiB`.
- Mandatory: no.
- Example supported values: `10MiB`, `100MiB`, `1GiB`.

##### Maximum non-streaming read size

Caps non-streaming file read responses. Larger content should be accessed with
range requests or streaming workflows when supported.

- TOML setting: `limits.max_read_size`.
- Command-line argument: `--max-read-size`.
- Environment variable: `NETIXFS_LIMITS_MAX_READ_SIZE`.
- Default value: `100MiB`.
- Mandatory: no.
- Example supported values: `10MiB`, `100MiB`, `1GiB`.

##### Maximum directory entries

Caps the number of directory entries returned per listing page. Lower values
reduce memory and latency but require more cursor pagination.

- TOML setting: `limits.max_directory_entries`.
- Command-line argument: `--max-directory-entries`.
- Environment variable: `NETIXFS_LIMITS_MAX_DIRECTORY_ENTRIES`.
- Default value: `10000`.
- Mandatory: no.
- Example supported values: `1000`, `10000`.

##### Maximum concurrent requests

Caps in-flight HTTP requests before worker dispatch. Requests above the limit
are rejected with `429 Too Many Requests`.

- TOML setting: `limits.max_concurrent_requests`.
- Command-line argument: `--max-concurrent-requests`.
- Environment variable: `NETIXFS_LIMITS_MAX_CONCURRENT_REQUESTS`.
- Default value: `1024`.
- Mandatory: no.
- Example supported values: `128`, `1024`.

##### Maximum concurrent streams

Caps active streaming responses. Stream requests above the limit are rejected
before response headers are sent.

- TOML setting: `limits.max_concurrent_streams`.
- Command-line argument: `--max-concurrent-streams`.
- Environment variable: `NETIXFS_LIMITS_MAX_CONCURRENT_STREAMS`.
- Default value: `128`.
- Mandatory: no.
- Example supported values: `16`, `128`.

#### Streaming

##### Stream idle timeout

Defines how long a stream may remain idle without data or heartbeat activity
before it is closed. Lower values release workers sooner. Values should use a
documented duration syntax such as `30s`, `5m`, or `1h`.

- TOML setting: `streaming.idle_timeout`.
- Command-line argument: `--stream-idle-timeout`.
- Environment variable: `NETIXFS_STREAMING_IDLE_TIMEOUT`.
- Default value: `5m`.
- Mandatory: no.
- Example supported values: `30s`, `5m`.

##### Stream maximum duration

Defines the maximum lifetime of a streaming response. This prevents long-lived
clients from holding workers forever. Values should use a documented duration
syntax such as `30s`, `5m`, or `1h`.

- TOML setting: `streaming.max_duration`.
- Command-line argument: `--stream-max-duration`.
- Environment variable: `NETIXFS_STREAMING_MAX_DURATION`.
- Default value: `1h`.
- Mandatory: no.
- Example supported values: `10m`, `1h`, `24h`.

##### Stream heartbeat interval

Controls optional heartbeat writes for idle streams. A value of `0` disables
heartbeats when supported by the implementation. Non-zero values should use a
documented duration syntax such as `30s`, `5m`, or `1h`.

- TOML setting: `streaming.heartbeat_interval`.
- Command-line argument: `--stream-heartbeat-interval`.
- Environment variable: `NETIXFS_STREAMING_HEARTBEAT_INTERVAL`.
- Default value: `30s`.
- Mandatory: no.
- Example supported values: `0`, `10s`, `30s`.

#### Worker Pool

##### Worker pool maximum workers

Caps the total number of live worker processes across all identities. Lower
values reduce resource use, while higher values allow more parallel filesystem
work.

- TOML setting: `pool.max_workers`.
- Command-line argument: `--pool-max-workers`.
- Environment variable: `NETIXFS_POOL_MAX_WORKERS`.
- Default value: `64`.
- Mandatory: no.
- Example supported values: `8`, `64`, `256`.

##### Worker idle timeout

Defines how long an idle worker may stay alive after completing its last
operation. Shorter values reduce lingering identity contexts but increase
process churn. Values should use a documented duration syntax such as `30s`,
`5m`, or `1h`.

- TOML setting: `pool.idle_timeout`.
- Command-line argument: `--pool-idle-timeout`.
- Environment variable: `NETIXFS_POOL_IDLE_TIMEOUT`.
- Default value: `5m`.
- Mandatory: no.
- Example supported values: `30s`, `5m`.

##### Worker request timeout

Defines the maximum duration for a non-streaming worker operation. Exceeding it
triggers cancellation and may terminate the worker if needed. Values should use
a documented duration syntax such as `30s`, `5m`, or `1h`.

- TOML setting: `pool.request_timeout`.
- Command-line argument: `--pool-request-timeout`.
- Environment variable: `NETIXFS_POOL_REQUEST_TIMEOUT`.
- Default value: `30s`.
- Mandatory: no.
- Example supported values: `10s`, `30s`, `5m`.

#### Logging

##### Log level

Controls the minimum severity emitted by operational logs. More verbose levels
help debugging but may increase volume and expose more operational detail.

- TOML setting: `logging.level`.
- Command-line argument: `--log-level`.
- Environment variable: `NETIXFS_LOGGING_LEVEL`.
- Default value: `info`.
- Mandatory: no.
- Example supported values: `error`, `warn`, `info`, `debug`, `trace`.

##### Log format

Controls the log output format. `json` is preferred for production ingestion,
while human-readable formats may help local debugging.

- TOML setting: `logging.format`.
- Command-line argument: `--log-format`.
- Environment variable: `NETIXFS_LOGGING_FORMAT`.
- Default value: `json`.
- Mandatory: no.
- Example supported values: `json`, `pretty`, `compact`.

##### Log path redaction

Controls whether paths are redacted in logs and audit events by default.
Disabling it improves detail but can expose sensitive filenames and directory
structures.

- TOML setting: `logging.redact_paths`.
- Command-line argument: `--log-redact-paths`.
- Environment variable: `NETIXFS_LOGGING_REDACT_PATHS`.
- Default value: `false`.
- Mandatory: no.
- Example supported values: `true`, `false`.

#### Metrics

##### Metrics enabled

Enables the metrics listener and metrics export. Disabling it reduces exposed
surfaces but removes operational visibility.

- TOML setting: `metrics.enabled`.
- Command-line argument: `--metrics-enabled`.
- Environment variable: `NETIXFS_METRICS_ENABLED`.
- Default value: `false`.
- Mandatory: no.
- Example supported values: `true`, `false`.

##### Metrics bind address

Controls which local address the metrics listener binds to. Loopback is safest
when metrics are scraped by a local agent.

- TOML setting: `metrics.bind_address`.
- Command-line argument: `--metrics-bind-address`.
- Environment variable: `NETIXFS_METRICS_BIND_ADDRESS`.
- Default value: `127.0.0.1`.
- Mandatory: no.
- Example supported values: `127.0.0.1`, `0.0.0.0`.

##### Metrics port

Controls the TCP port for the metrics listener. It should be separated from the
main API listener unless the deployment explicitly combines them.

- TOML setting: `metrics.port`.
- Command-line argument: `--metrics-port`.
- Environment variable: `NETIXFS_METRICS_PORT`.
- Default value: `9090`.
- Mandatory: no.
- Example supported values: `9090`, `9100`.

#### Runtime Configuration Endpoint

##### Runtime configuration endpoint enabled

Enables the runtime configuration endpoint. Disabling it removes the endpoint
from the exposed HTTP surface.

- TOML setting: `diagnostics.config_endpoint.enabled`.
- Command-line argument: `--config-endpoint-enabled`.
- Environment variable: `NETIXFS_DIAGNOSTICS_CONFIG_ENDPOINT_ENABLED`.
- Default value: `false`.
- Mandatory: no.
- Example supported values: `true`, `false`.

##### Runtime configuration endpoint bind address

Controls which local address and port the runtime configuration endpoint binds
to when `diagnostics.config_endpoint.enabled=true`. Loopback is safest unless an
operator-facing network explicitly protects the endpoint.

- TOML setting: `diagnostics.config_endpoint.bind_address`.
- Command-line argument: `--config-endpoint-bind-address`.
- Environment variable: `NETIXFS_DIAGNOSTICS_CONFIG_ENDPOINT_BIND_ADDRESS`.
- Default value: `127.0.0.1:8081`.
- Mandatory: no.
- Example supported values: `127.0.0.1:8081`, `0.0.0.0:8081`.

## 13. Observability

NetixFS should provide production-grade observability through structured logs,
request IDs, audit logs, metrics, health/readiness endpoints, and runtime
configuration introspection.

### 13.1 Request IDs

Every request must have a request ID. If the caller supplies `X-Request-Id`,
NetixFS should preserve it after validating that it is safe to log. Otherwise,
NetixFS must generate one. Error responses must include the request ID in the
structured error body.

### 13.2 Logs

Operational logs must support running and debugging NetixFS. They should answer
what happened inside the service, such as request completion, worker creation,
JWT key refresh failures, stream disconnects, timeouts, internal errors, and
rejected CORS preflight requests when `cors.enabled=true`.

Logs must be structured when `logging.format=json`. Each log event should
include at least:

- `timestamp`;
- `level`;
- `message`;
- `request_id`, when the event belongs to a request;
- `operation`, when known;
- `root_id`, when known;
- `subject`, when authentication has succeeded;
- `uid`, `gid`, and supplementary groups, when identity resolution has
  succeeded;
- `status` or error code, when the event records a completed operation.

Example request log:

```json
{
  "timestamp": "2026-05-18T12:34:56.789Z",
  "level": "info",
  "message": "request completed",
  "request_id": "req_01HY9Z55JWQHZJ4CK6V0M4JZ9D",
  "method": "GET",
  "route": "/api/v1/roots/{root_id}/file",
  "operation": "read_file",
  "root_id": "home",
  "path": "[redacted]",
  "subject": "alice",
  "uid": 1000,
  "gid": 1000,
  "status": 200,
  "duration_ms": 12,
  "bytes_read": 18432
}
```

### 13.3 Audit Logs

Audit logs must support accountability and security review. They should answer
who attempted or performed a security-relevant action, what target was involved,
and whether the action succeeded or was rejected. Audit logs may use the same
logging backend and structured JSON format as operational logs, but their schema
should be more stable, they should be harder to disable accidentally, and they
should usually have longer retention.

Audit logs must be emitted for write operations, permission-changing operations,
link creation, recursive operations when enabled, authentication failures,
identity resolution failures, and rejected service-safety checks.

Audit events should include the authenticated subject, resolved UID, primary
GID, supplementary groups, operation, root ID, target path or redacted path,
result, HTTP status, errno when applicable, request ID, and timestamp.

Example audit log:

```json
{
  "timestamp": "2026-05-18T12:35:10.012Z",
  "level": "info",
  "message": "audit event",
  "event": "filesystem_write",
  "request_id": "req_01HY9Z5AZZSQPK9VC1M0C2B6SF",
  "operation": "replace_file",
  "root_id": "home",
  "path": "[redacted]",
  "subject": "alice",
  "uid": 1000,
  "gid": 1000,
  "groups": [1000, 1001],
  "result": "success",
  "status": 200,
  "errno": null
}
```

### 13.4 Metrics

Metrics should include request counts, latencies, error counts, active requests,
worker pool size, worker creation failures, active streams, stream disconnects,
bytes read, and bytes written.

Metrics should be exposed on the configured metrics listener when
`metrics.enabled=true`. The metrics format should be Prometheus-compatible unless
another exporter is explicitly configured.

Recommended metrics:

- `netixfs_http_requests_total{method,route,status}`: total HTTP requests;
- `netixfs_http_request_duration_seconds{method,route,status}`: request
  latency histogram;
- `netixfs_http_requests_active`: currently active HTTP requests;
- `netixfs_worker_processes{state}`: worker count by state, such as `idle`,
  `busy`, or `starting`;
- `netixfs_worker_create_failures_total{reason}`: worker creation failures;
- `netixfs_streams_active`: currently active streams;
- `netixfs_stream_disconnects_total{reason}`: stream disconnects;
- `netixfs_filesystem_bytes_read_total{operation}`: bytes read from files;
- `netixfs_filesystem_bytes_written_total{operation}`: bytes written to files;
- `netixfs_errors_total{operation,status,code}`: structured errors.

Example Prometheus metrics:

```text
netixfs_http_requests_total{method="GET",route="/api/v1/roots/{root_id}/file",status="200"} 42
netixfs_worker_processes{state="idle"} 7
netixfs_worker_processes{state="busy"} 3
netixfs_streams_active 2
netixfs_filesystem_bytes_read_total{operation="read_file"} 7340032
```

Metrics labels must avoid high-cardinality or sensitive values such as raw
paths, subjects, request IDs, or file names. Route labels should use route
templates such as `/api/v1/roots/{root_id}/file`, not concrete request paths.

### 13.5 Health And Readiness

Health and readiness endpoints must not require authentication unless the
deployment explicitly enables authenticated probes.

NetixFS should expose:

- `GET /healthz`: liveness check;
- `GET /readyz`: readiness check.

`/healthz` should return `200 OK` when the supervisor process is alive and able
to run its event loop. It should not perform expensive filesystem, identity, or
network checks.

Example liveness response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status": "ok"
}
```

`/readyz` should return `200 OK` only when NetixFS is able to accept useful
traffic. It should check JWT key availability, metrics and worker pool
initialization, and whether required service limits are valid. It may also
include degraded warnings for non-fatal conditions.

Once the supervisor is running, configuration has already validated successfully
at startup (see [section 12.2](#122-configuration-validation-and-startup)). The
`configuration` readiness check must therefore report `"ok"`. Readiness
failures must be reported through the other checks, such as `jwt_keys` or
`pool`. NetixFS must not re-validate full configuration on every `/readyz`
request unless a future specification adds configuration hot reload.

Example readiness response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status": "ready",
  "checks": {
    "configuration": "ok",
    "jwt_keys": "ok",
    "pool": "ok"
  }
}
```

If readiness fails, NetixFS should return `503 Service Unavailable` with a JSON
body that identifies failed checks without leaking secrets.

Example readiness failure:

```http
HTTP/1.1 503 Service Unavailable
Content-Type: application/json

{
  "status": "not_ready",
  "checks": {
    "configuration": "ok",
    "jwt_keys": "unavailable",
    "pool": "ok"
  }
}
```

### 13.6 Runtime Configuration

NetixFS should expose an operator-facing endpoint for inspecting the effective
runtime configuration:

- `GET /configz`: effective runtime configuration and value provenance.

`/configz` is available only after configuration has validated successfully and
the supervisor has started. If configuration validation fails, NetixFS must not
start and `/configz` must not be exposed.

`/configz` should be exposed only when
`diagnostics.config_endpoint.enabled=true`, and its listener should bind to
`diagnostics.config_endpoint.bind_address`. It should return `200 OK` with every
recognized runtime setting, including settings that are using defaults. Each
setting must include the current effective value and the source that supplied
that value. Supported sources are:

- `default`;
- `config_file`;
- `environment`;
- `argument`.

Values loaded from higher-precedence sources must report the source that won the
configuration merge. For example, a command-line argument must report
`"source": "argument"` even when the same setting also appeared in the TOML file
or environment.

The response should be structured by configuration section. Each entry should
include:

- `value`: the current effective value, redacted when the value is sensitive;
- `source`: one of the supported source identifiers;
- `toml`: the TOML key when one exists;
- `environment`: the environment variable name when one exists;
- `argument`: the command-line argument name when one exists.

Example runtime configuration response:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "settings": {
    "server": {
      "bind_address": {
        "value": "127.0.0.1",
        "source": "default",
        "toml": "server.bind_address",
        "environment": "NETIXFS_SERVER_BIND_ADDRESS",
        "argument": "--bind-address"
      },
      "port": {
        "value": 8080,
        "source": "environment",
        "toml": "server.port",
        "environment": "NETIXFS_SERVER_PORT",
        "argument": "--port"
      }
    },
    "auth": {
      "jwt": {
        "public_key_path": {
          "value": "/run/secrets/netixfs-jwt.pub",
          "source": "config_file",
          "toml": "auth.jwt.public_key_path",
          "environment": "NETIXFS_AUTH_JWT_PUBLIC_KEY_PATH",
          "argument": "--jwt-public-key-path"
        }
      }
    },
    "diagnostics": {
      "config_endpoint": {
        "enabled": {
          "value": true,
          "source": "argument",
          "toml": "diagnostics.config_endpoint.enabled",
          "environment": "NETIXFS_DIAGNOSTICS_CONFIG_ENDPOINT_ENABLED",
          "argument": "--config-endpoint-enabled"
        },
        "bind_address": {
          "value": "127.0.0.1:8081",
          "source": "default",
          "toml": "diagnostics.config_endpoint.bind_address",
          "environment": "NETIXFS_DIAGNOSTICS_CONFIG_ENDPOINT_BIND_ADDRESS",
          "argument": "--config-endpoint-bind-address"
        }
      }
    }
  }
}
```

Sensitive values such as private key paths, tokens, inline secrets, credentials,
or secret-bearing URLs must be redacted while still reporting their source. Path
values must follow the configured path redaction policy. The endpoint must not
expose unknown environment variables, raw process arguments, or unrecognized
configuration keys from the host environment.

The endpoint should require the same operator-level protection as other
diagnostic endpoints unless the deployment explicitly disables authentication
for local-only diagnostics.

## 14. Security Requirements

Security is central to NetixFS because it exposes filesystem operations over a
network.

The design must address:

- JWT signature validation and configured issuer and audience validation;
- local username-to-UID/GID/supplementary-groups resolution;
- privilege dropping and per-worker UID, primary GID, and supplementary group
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
- explicit controls for dangerous operations;
- protection against accidental exposure of host-sensitive paths;
- optional CORS configuration with disabled-by-default behavior and explicit
  allowed origins, as defined in [section 10.7](#107-cross-origin-resource-sharing-cors).

## 15. Testing Requirements

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
- file truncation and replacement behavior during streaming;
- concurrent writes and reads;
- error mapping;
- configuration parsing;
- Linux capability assumptions;
- worker process reuse, expiration, and identity isolation;
- CORS behavior.

Security-sensitive behavior should be covered by integration tests on real Linux
filesystems, not only unit tests.

## 16. Implementation Plan

The following sequence is a proposed implementation roadmap, ordered from a
minimal working prototype to full specification coverage. It is guidance for
planning work and is not itself a conformance requirement.

1. Minimal HTTP skeleton:
   Start with configuration loading, command-line and environment overrides,
   `/healthz`, `/readyz`, structured JSON errors, request IDs, and basic
   structured logs. At this stage, filesystem APIs can remain stubbed.

2. Authentication and identity resolution:
   Add JWT validation, issuer and audience validation when configured, username
   claim extraction with `sub` as the default, and local identity resolution
   through NSS. Use this step to verify that unresolved identities fail before
   filesystem access.

3. Root and path safety layer:
   Add configured roots, `root_id` routing, `path` and `path_b64`
   handling, path normalization, rejection of unsafe path components, and root
   containment checks. Include Linux integration tests for traversal and
   containment behavior.

4. Supervisor/worker execution model:
   Add the non-root supervisor, capability requirements, worker creation,
   per-worker Unix domain socket pairs, framed IPC, worker identity switching,
   and the rule that each worker is bound to one resolved local identity for its
   lifetime.

5. Read-only filesystem APIs:
   Add Stat API, Directory API listing with limits and cursors, file reads
   with range support, symbolic link reads, and xattr reads where supported.
   Cover configured symlink and mount-boundary policy in this phase.

6. Core write APIs:
   Add create or replace file with atomic replacement, delete file,
   create directory, remove empty directory, rename, and copy. Cover read-only
   mode, request body limits, and structured error mapping.

7. Worker pool and backpressure:
   Add worker pool limits, idle expiration, request timeout handling,
   cancellation, bounded pending work, stream limits, and `429` / `503` behavior
   when capacity is exhausted or workers cannot be created.

8. Streaming:
   Add text-only stream support with `tail -f` descriptor-following
   semantics, inotify-based change detection where available, truncation and
   replacement behavior, stream cancellation, backpressure, idle timeout,
   maximum duration, and optional heartbeat behavior.

9. Optional and dangerous operations:
   Add configurable recursive delete, recursive copy, chmod, hard-link creation,
   and symbolic-link creation.

10. ETags and HTTP preconditions:
    Add ETag generation for filesystem objects, conditional write handling
    with `If-Match`, `If-None-Match`, and `If-Unmodified-Since`, and
    `412 Precondition Failed` behavior. Include tests for stale writes,
    create-if-absent workflows, truncation guards, and filesystem metadata edge
    cases.

11. Observability and operations hardening:
    Round out operational logs, audit logs, metrics, health and readiness
    behavior, path redaction, configuration validation, and operator-facing
    failure messages.

12. Cross-origin resource sharing (CORS):
    Add supervisor CORS handling on `/api/v1/**` when `cors.enabled=true`.

13. Full specification verification:
    Add end-to-end Linux integration tests for permission behavior, symlink
    races, mount-boundary behavior, worker identity isolation, streaming edge
    cases, concurrent reads and writes, ETag and precondition behavior, error
    mapping, CORS and configuration precedence.
