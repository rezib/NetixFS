# NetixFS Technical Specification

## 1. Purpose

NetixFS is a Linux-only service that exposes selected POSIX filesystem
operations through an HTTP(S) API.

The service acts as an HTTP proxy for filesystem access. It is intended to let
remote clients perform high-level file and directory operations that would
normally be performed from a shell, while preserving POSIX permission semantics
as much as possible.

NetixFS must be implemented in Rust.

## 2. Product Goals

- Provide a REST-style API for common filesystem operations on files,
  directories, metadata, and streams.
- Support both read and write workflows.
- Support streaming workflows such as `tail -f`.
- Delegate user authentication to an external identity provider or gateway.
- Execute filesystem operations using the UID and GID associated with the
  authenticated user identity.
- Run on Linux with the smallest practical privilege set.
- Be configurable through command-line arguments and environment variables only.

## 3. Non-Goals

- NetixFS must not implement primary user authentication itself.
- NetixFS must not provide its own user database.
- NetixFS must not require a configuration file.
- NetixFS does not target non-Linux operating systems in the initial version.
- NetixFS is not a distributed filesystem and must not attempt to replicate,
  synchronize, or cache filesystem state across hosts unless explicitly added in
  a later design.

## 4. Target Environment

The initial target platform is Linux.

The service should be deployable as:

- a system service on a Linux host;
- a containerized service, provided the required filesystem mounts and Linux
  capabilities are available;
- an internal service behind a reverse proxy, API gateway, or identity-aware
  proxy.

The service must make its privilege requirements explicit. In particular, the
design must clarify whether UID/GID switching requires:

- running the process as root;
- using Linux capabilities such as `CAP_SETUID`, `CAP_SETGID`, `CAP_DAC_READ_SEARCH`,
  or `CAP_DAC_OVERRIDE`;
- spawning per-request helper processes;
- using kernel mechanisms such as `setfsuid` / `setfsgid`;
- relying on a preconfigured pool of worker processes.

## 5. Authentication Boundary

NetixFS expects requests to contain a JWT issued by an external authentication
system, for example Keycloak, Authentik, or an upstream API gateway.

NetixFS is responsible for validating the JWT before performing any filesystem
operation. Validation requirements should include:

- signature verification using a configured issuer key or JWKS endpoint;
- issuer validation;
- audience validation;
- expiration and not-before validation;
- optional claim validation for tenant, group, role, or scope constraints.

Open question: Should NetixFS validate JWTs directly, or should it only trust
headers injected by a trusted reverse proxy after validation?

Open question: If NetixFS validates JWTs directly, how should key discovery and
key rotation be configured without a configuration file?

## 6. Identity Mapping

Each request must resolve to a filesystem identity made of:

- effective UID;
- effective primary GID;
- optional supplementary groups.

The source of this mapping must be defined precisely. Possible approaches:

- JWT contains numeric `uid`, `gid`, and `groups` claims.
- JWT contains a username, and NetixFS resolves it through local NSS.
- JWT contains an external subject, and NetixFS maps it through an explicit
  mapping table provided by environment variables or command-line arguments.
- NetixFS trusts identity headers provided by an upstream service.

Hard point: POSIX access checks often depend on supplementary groups, not only
UID and primary GID. The specification must decide whether supplementary groups
are supported, required, or intentionally out of scope for the first version.

Hard point: Numeric IDs in JWTs are host-local concepts. The same UID may
represent different users on different machines unless identity management is
centralized.

## 7. Authorization Model

Filesystem authorization should primarily be delegated to the Linux kernel by
performing operations under the resolved UID/GID.

NetixFS should also support service-level restrictions to reduce blast radius,
including:

- one or more allowed root directories;
- optional read-only mode;
- optional deny rules for paths, file types, or operations;
- maximum request body size;
- maximum file size for non-streaming reads;
- maximum number of directory entries returned in one response;
- symlink traversal policy.

Hard point: Path normalization and containment must be robust against `..`
segments, symlink escapes, bind mounts, hard links, race conditions, and
time-of-check/time-of-use issues.

Open question: Should authorization be purely kernel-enforced, or should NetixFS
also implement an application-level allow/deny policy?

## 8. Filesystem Operation Scope

The API should cover the following operation groups.

### 8.1 Metadata

- stat path;
- lstat path;
- read file type, size, mode, owner, group, timestamps, inode, and device;
- check existence;
- list extended attributes, if supported;
- read selected extended attributes, if supported.

### 8.2 Directory Operations

- list directory entries;
- create directory;
- create directory tree;
- remove empty directory;
- recursively remove directory, if explicitly enabled;
- rename or move path;
- copy path, if included in scope.

### 8.3 File Read Operations

- read whole file, subject to size limits;
- read byte range;
- stream file contents;
- follow appended data, equivalent to `tail -f`;
- optionally follow file rotation, equivalent to `tail -F`.

### 8.4 File Write Operations

- create file;
- overwrite file;
- append to file;
- write byte range, if supported;
- truncate file;
- create temporary file and atomically rename it into place;
- control file mode on creation, subject to umask and policy.

### 8.5 Links

- create symbolic link, if enabled;
- read symbolic link;
- create hard link, if enabled;
- define symlink traversal behavior for every operation.

### 8.6 Permissions and Ownership

- chmod, if enabled;
- chown, if enabled;
- chgrp, if enabled;
- update timestamps, if enabled.

Hard point: Ownership changes are especially sensitive. Supporting `chown`
likely requires elevated privileges and should be disabled by default unless a
clear operational need exists.

## 9. HTTP API Design Requirements

The API should be explicit, stable, and machine-friendly.

Required design decisions:

- URL structure for addressing filesystem paths;
- whether paths are passed in URL components, query parameters, or request
  bodies;
- escaping rules for arbitrary POSIX paths;
- response format for metadata and errors;
- binary transfer format for file contents;
- idempotency semantics for write operations;
- conditional request support using ETag, inode metadata, or modification time;
- pagination or cursoring for large directories;
- cancellation behavior for long-running operations.

Hard point: POSIX paths are arbitrary byte sequences except for `/` and NUL.
HTTP APIs and JSON strings are generally Unicode-oriented. The API must define
how non-UTF-8 paths are represented.

Hard point: HTTP method semantics and filesystem semantics do not always map
cleanly. For example, `PUT` can represent full replacement, but append, partial
write, atomic rename, and recursive delete need explicit API design.

## 10. Streaming Requirements

NetixFS must support streaming file reads, including a `tail -f`-style mode.

The streaming design must specify:

- transport: chunked HTTP response, Server-Sent Events, WebSocket, or another
  mechanism;
- whether streamed data is raw bytes or framed events;
- how clients receive errors after a stream has started;
- idle timeout behavior;
- heartbeat behavior, if any;
- backpressure behavior;
- maximum stream duration;
- maximum number of concurrent streams;
- behavior when the file is truncated, replaced, deleted, or rotated.

Hard point: `tail -f` follows a file descriptor, while `tail -F` follows a path
across rotation. NetixFS should choose one behavior explicitly or expose both.

## 11. Concurrency and Consistency

NetixFS must define how it handles concurrent clients operating on the same
paths.

Required decisions:

- whether writes are atomic per request;
- whether the API exposes file locking;
- whether NetixFS uses temporary files plus atomic rename for safe replacement;
- how partial failures are reported;
- whether long-running recursive operations are synchronous or job-based.

Hard point: POSIX filesystems provide different guarantees depending on the
underlying filesystem. NetixFS should avoid promising stronger consistency than
Linux and the mounted filesystem can provide.

## 12. Error Model

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

## 13. Configuration

NetixFS must be configured only through command-line arguments and environment
variables.

Configuration should include:

- bind address and port;
- TLS settings, unless TLS is terminated upstream;
- JWT validation settings;
- accepted issuer and audience;
- JWKS URL or public key path/content;
- allowed filesystem roots;
- default umask;
- maximum request body size;
- maximum read size;
- directory listing limits;
- stream limits and timeouts;
- logging format and level;
- metrics enablement;
- read-only mode;
- dangerous operation enablement, such as recursive delete or ownership change.

Hard point: Environment variables are weak for large structured policy. If
NetixFS grows complex allow/deny rules, the "no configuration file" constraint
may become a significant usability and safety limitation.

## 14. Observability

NetixFS should provide production-grade observability:

- structured logs;
- request IDs and trace IDs;
- audit logs for write operations and permission-changing operations;
- metrics for request counts, latencies, error codes, open streams, bytes read,
  and bytes written;
- health and readiness endpoints.

Audit logs should include the authenticated subject, resolved UID/GID, operation,
target path or redacted path, result, and timestamp.

Hard point: Logs can leak sensitive filenames and directory structures. The
logging policy must define what is logged by default and how redaction works.

## 15. Security Requirements

Security is central to NetixFS because it exposes filesystem operations over a
network.

The design must address:

- JWT validation and trust boundary;
- privilege dropping and UID/GID switching;
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

Hard point: A service that can switch to arbitrary UIDs or bypass DAC using
capabilities can become equivalent to root if compromised. The privilege model
needs careful threat modeling before implementation.

## 16. Implementation Constraints

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

Open question: Should filesystem operations run directly in async handlers,
through blocking task pools, or in separate worker processes after UID/GID
switching?

## 17. Testing Requirements

Testing should cover:

- path normalization and containment;
- symlink traversal and symlink race scenarios;
- JWT validation failures;
- UID/GID mapping failures;
- POSIX permission behavior;
- read and write operations;
- large directory listings;
- streaming reads;
- file truncation and rotation during streaming;
- concurrent writes and reads;
- error mapping;
- configuration parsing;
- Linux capability assumptions.

Security-sensitive behavior should be covered by integration tests, not only
unit tests.

## 18. Open Questions and Design Decisions

The following questions should be resolved before implementation starts:

1. Is the product name NetixFS or NetifFX?
2. Is NetixFS expected to validate JWTs itself, or trust a validating reverse
   proxy?
3. Which JWT claims define UID, GID, and supplementary groups?
4. Are supplementary groups required in version 1?
5. What is the exact Linux privilege model for performing operations as another
   UID/GID?
6. What filesystem roots may be exposed, and how are path escapes prevented?
7. Are symbolic links followed, rejected, or configurable per operation?
8. Is the API path representation required to support non-UTF-8 POSIX names?
9. Which write operations are included in version 1?
10. Are dangerous operations such as recursive delete, chmod, chown, hard links,
    and symlinks included, disabled by default, or out of scope?
11. Should `tail -f`, `tail -F`, or both be supported?
12. Should large recursive operations be synchronous requests or asynchronous
    jobs?
13. Is TLS handled by NetixFS or by an upstream proxy?
14. What information may be logged without violating privacy or security
    requirements?
15. What are the minimum supported Linux kernel and distribution assumptions?

## 19. Initial Version Proposal

A conservative first version should include:

- direct JWT validation or clearly documented trusted-proxy mode;
- one or more configured allowed roots;
- metadata read;
- directory listing with limits;
- file read with range support;
- raw streaming read with `tail -f` semantics;
- file create, overwrite, append, truncate, rename, and delete;
- directory create and remove-empty-directory;
- no recursive delete by default;
- no `chown` by default;
- no hard-link creation by default;
- explicit symlink policy;
- structured JSON errors;
- structured logs and basic metrics;
- Linux integration tests for permission and path-safety behavior.
