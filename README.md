# WFS - Web File System

A simple HTTP server that manages user-specific files based on authentication tokens. Each user gets their own directory under a configurable root path.

## Features

- **Per-user file storage**: Files are stored in user-specific subdirectories based on the provided token
- **Configuration file**: Default root path can be set via TOML configuration in `~/.config/wfs/config.toml`
- **Command-line override**: Root path can be specified via `--root-path` argument
- **HTTP Methods**:
  - `GET`: Read a file
  - `POST`: Create or overwrite a file
  - `DELETE`: Remove a file

## Configuration

### TOML Configuration File

Create a configuration file at `~/.config/wfs/config.toml`:

```toml
root_path = "/path/to/your/data/directory"
```

If the file doesn't exist, the server defaults to `./data` in the current working directory.

### Command-Line Argument

You can override the root path when starting the server:

```bash
cargo run -- --root-path /custom/path
```

## Usage

Start the server:

```bash
cargo run
```

The server listens on `http://0.0.0.0:3000` by default.

### Authentication

All requests must include an `Authorization` header with a Bearer token:

```
Authorization: Bearer <your-token>
```

The token is used as the subdirectory name under the root path. For example, with root path `/data` and token `user123`, files are stored in `/data/user123/`.

### Endpoints

All endpoints use the request path as the file path relative to the user's directory.

#### GET

Read a file's content:

```bash
curl -H "Authorization: Bearer mytoken" http://localhost:3000/path/to/file.txt
```

#### POST

Create or overwrite a file:

```bash
curl -X POST \
  -H "Authorization: Bearer mytoken" \
  -H "Content-Type: text/plain" \
  --data "file content" \
  http://localhost:3000/path/to/file.txt
```

Maximum file size: 10MB

#### DELETE

Remove a file:

```bash
curl -X DELETE \
  -H "Authorization: Bearer mytoken" \
  http://localhost:3000/path/to/file.txt
```

## Project Structure

```
wfs/
├── Cargo.toml
├── README.md
└── src/
    └── main.rs
```

## Dependencies

- axum 0.8.9
- tokio (with net, rt, rt-multi-thread, tokio-macros, fs features)
- serde (with derive feature)
- toml 1.1.2
