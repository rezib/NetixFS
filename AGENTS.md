# NetixFS Agents Guide

## Development Guidelines

### Code Style

- Avoid using `unsafe` Rust.
- Follow Rust's standard style (enforced by `cargo fmt`)
- Use `clippy` for linting: `cargo clippy`
- Prefer explicit error handling with `eyre`
- Use `tracing` for logging (not `println!`)
- Keep only one crate (`netixfs`) with eventually multiple binaries or
  libraries in it.
- Always collapse if statements per
  <https://rust-lang.github.io/rust-clippy/master/index.html#collapsible_if>
- Always inline format! args when possible per
  <https://rust-lang.github.io/rust-clippy/master/index.html#uninlined_format_args>
- Use method references over closures when possible per
  <https://rust-lang.github.io/rust-clippy/master/index.html#redundant_closure_for_method_calls>
- Avoid bool or ambiguous Option parameters that force callers to write
  hard-to-read code such as foo(false) or bar(None). Prefer enums, named methods,
  newtypes, or other idiomatic Rust API shapes when they keep the callsite
  self-documenting.
- When possible, make match statements exhaustive and avoid wildcard arms.
- Newly added traits should include doc comments that explain their role and
  how implementations are expected to use them.
- When writing tests, prefer comparing the equality of entire objects over
  fields one by one.
- Do not create small helper methods that are referenced only once.
- Avoid large modules:
  - Prefer adding new modules instead of growing existing ones.
  - Target Rust modules under 500 LoC, excluding tests.
  - If a file exceeds roughly 800 LoC, add new functionality in a new module
    instead of extending the existing file unless there is a strong documented
    reason not to.

## Testing Strategy

Per **SPECS.md Section 15**, testing should cover:

- Path normalization and containment
- Symlink traversal and symlink race scenarios
- JWT validation failures
- Username claim extraction failures
- Local username resolution failures
- POSIX permission behavior
- Read and write operations
- Large directory listings
- Streaming reads
- Concurrent writes and reads
- Error mapping
- Configuration parsing
- Worker process reuse, expiration, and identity isolation
- CORS behavior

Security-sensitive behavior **must** be covered by integration tests on real
Linux filesystems.

## Contributing as an Agent

When contributing to NetixFS:

1. **Read SPECS.md first** - It's the source of truth for all requirements
2. **Follow the implementation plan** - Work on steps in order when possible
3. **Write tests** - Especially for security-sensitive functionality
4. **Use structured logging** - Include request context in logs
5. **Handle errors properly** - Return structured JSON errors with request IDs
6. **Respect POSIX semantics** - Delegate authorization to the Linux kernel
7. **Keep it simple** - Avoid unnecessary complexity; the spec is already comprehensive

### Common Pitfalls to Avoid

- Hardcoding paths or root directories
- Performing filesystem operations in the supervisor process
- Trusting JWT claims for UID/GID (must resolve via NSS)
- Not validating paths for `..` components or symlink escapes
- Not including request IDs in errors and logs
- Using `CAP_DAC_OVERRIDE` or other broad filesystem capabilities

### Recommended Workflow

1. Identify a task from the implementation plan
2. Read the relevant SPECS.md sections
3. Explore existing code for patterns
4. Implement the feature
5. Write tests (especially integration tests)
6. Verify with `cargo clippy` and `cargo test`
