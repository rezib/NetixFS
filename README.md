# NetixFS

A Linux-only service that exposes selected POSIX filesystem operations through
an HTTP(S) API. The service acts as an HTTP proxy for filesystem access,
allowing remote clients to perform file and directory operations while
preserving POSIX permission semantics.

## Status

Experimental, currently in development.

## Features

### Core Architecture

- **Supervisor/Worker model**: Supervisor handles authentication, routing, and
  worker lifecycle; workers execute filesystem operations under resolved local
  identities
- **POSIX permission preservation**: Delegates authorization to the Linux
  kernel via per-worker UID/GID switching
- **Root squash support**: Works on shared filesystems where root privileges
  are insufficient
- **Minimal capabilities**: Requires only `CAP_SETUID`, `CAP_SETGID`,
  `CAP_KILL`, and optionally `CAP_NET_BIND_SERVICE`

### Security & Authentication

- **JWT authentication**: Validates tokens from external identity providers
  (Keycloak, Authentik, etc.)
- **JWT validation**: Signature verification, expiration/not-before checks,
  optional issuer and audience validation
- **Multiple key sources**: Static public key (local/remote), JWKS
  (local/remote) with automatic refresh
- **NSS identity resolution**: Maps JWT username claim to local UID, primary
  GID, and supplementary groups via Name Service Switch
- **Path containment**: Rejects `..` components, validates symlink traversal,
  enforces mount-boundary checks
- **No broad capabilities**: Explicitly avoids `CAP_DAC_OVERRIDE`,
  `CAP_FOWNER`, `CAP_SYS_ADMIN`

### Filesystem Operations

- **File operations**: Read, write (atomic replacement), delete, stat, stream
  (tail -f style)
- **Directory operations**: List (with pagination), create, delete, rename,
  copy
- **Link operations**: Read symlinks, create symlinks, create hard links
- **Metadata operations**: Change mode (chmod), change group (chgrp), read
  extended attributes
- **Atomic writes**: File replacements use temporary files + atomic rename
- **Range requests**: Partial content reads with `Range` header support
- **ETags & preconditions**: HTTP precondition support (`If-Match`,
  `If-None-Match`, `If-Unmodified-Since`)

### API Features

- **Versioned API**: All endpoints under `/api/v1/roots/{root_id}/` prefix
- **Multiple root support**: Configure multiple named filesystem roots
- **Path encoding**: Supports both UTF-8 `path` and base64url `path_b64` for
  non-UTF-8 paths
- **Streaming**: Text file streaming with inotify-based change detection,
  backpressure support
- **Structured errors**: JSON error responses with request IDs, error codes,
  and HTTP status
- **CORS support**: Optional, disabled by default with explicit allowed origins
  configuration
- **Read-only mode**: Disables write operations for safe inspection deployments

### Configuration

- **Multi-source**: TOML configuration file, environment variables (`NETIXFS_*`
  prefix), command-line arguments
- **Precedence**: Command-line > Environment > TOML file
- **TLS support**: Native TLS or upstream TLS-terminating proxy
- **Worker pool**: Configurable max workers, idle timeout, request timeout
- **Limits**: Request body size, read size, concurrent requests, concurrent
  streams
- **Streaming limits**: Idle timeout, maximum duration, optional heartbeat
- **Logging**: JSON or human-readable formats, configurable log level, optional
  path redaction

### Observability

- **Health endpoints**: `/healthz` (liveness), `/readyz` (readiness)
- **Metrics**: Prometheus-compatible endpoint with request counts, latencies,
  errors, worker stats
- **Structured logging**: Request IDs, operation names, root IDs, subjects,
  UIDs, GIDs in all logs
- **Audit logging**: Security-relevant actions (writes, permission changes,
  auth failures, identity resolution failures)
- **Runtime configuration**: `/configz` endpoint shows effective configuration
  with value provenance

### Deployment

- **System service**: Deployable as a Linux system service
- **Containerized**: Works in containers with required filesystem mounts and
  capabilities
- **Reverse proxy**: Designed for deployment behind API gateways or
  identity-aware proxies
