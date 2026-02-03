# Project Guidelines

Synology Drive public link downloader CLI tool.

## Build

```bash
just build    # dev-release profile (optimized + debug symbols)
just check    # fmt + clippy
```

## Test

```bash
# Info-only mode (no download)
./target/dev-release/synology-dl --info '<share-url>'
```

## Architecture

Single-binary CLI. All code in `src/main.rs`.

Flow: parse URL → get session cookie → fetch metadata via JS API → stream download with progress bar.

## Dependencies

- `clap` - CLI parsing
- `reqwest` - HTTP client
- `tokio` - async runtime
- `eyre`/`thiserror` - error handling
- `tracing` - logging
