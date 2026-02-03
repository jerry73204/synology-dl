# synology-dl

Download files from Synology Drive public sharing links.

## Install

```bash
cargo install synology-dl
```

Don't have cargo? Install it from [rustup.rs](https://rustup.rs/).

## Usage

```bash
synology-dl <URL> [OUTPUT_DIR]
```

### Options

- `-i, --info` - Show file info only, don't download
- `-f, --force` - Overwrite existing files without prompting

### Examples

```bash
# Download to current directory
synology-dl 'https://nas.example.com/drive/d/s/ABC123/XYZ789'

# Download to specific directory
synology-dl 'https://nas.example.com/drive/d/s/ABC123/XYZ789' ./downloads

# Show file info only
synology-dl --info 'https://nas.example.com/drive/d/s/ABC123/XYZ789'
```

## License

MIT
