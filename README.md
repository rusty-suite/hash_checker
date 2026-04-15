# Hash Checker

A fast and ergonomic file integrity verifier built in Rust.
Supports automatic checksum file detection, multiple hash algorithms, drag-and-drop, right-click shell integration on Windows and Linux, and a clean dual-mode interface — GUI and CLI.

---

## Usage

### GUI mode

Launch without arguments to open the graphical interface.

```
hash_checker.exe
```

- Drag and drop a file onto the window, or click to select one.
- The application automatically looks for a matching checksum file in the same directory.
- If no checksum file is found, manually select one or paste the expected hash value.
- Click **Verify** to compare.

### CLI mode

```
hash_checker.exe <file> [options]
```

| Option | Description |
|--------|-------------|
| `<file>` | File to verify |
| `--checksum <file>` | Checksum file to use (auto-detected if omitted) |
| `--hash <value>` | Expected hash value (manual comparison) |
| `--algo <algo>` | Algorithm: `md5`, `sha1`, `sha256`, `sha512`, etc. |
| `--compute` | Just compute and print the hash, no comparison |

**Examples**

```bash
# Auto-detect checksum file in the same directory
hash_checker.exe ubuntu-24.04.iso

# Provide checksum file manually
hash_checker.exe ubuntu-24.04.iso --checksum SHA256SUMS

# Provide hash value directly (algorithm auto-detected from length)
hash_checker.exe ubuntu-24.04.iso --hash a1b2c3d4...

# Just compute the SHA-256 hash
hash_checker.exe ubuntu-24.04.iso --compute --algo sha256
```

**Exit codes**

| Code | Meaning |
|------|---------|
| `0` | Verification passed — file is intact |
| `1` | Error (file not found, unreadable checksum, etc.) |
| `2` | Verification failed — file is corrupted or modified |

---

## Supported algorithms

MD5, SHA-1, SHA-224, SHA-256, SHA-384, SHA-512, CRC32

---

## Supported checksum file formats

- `<hash>  <filename>` — standard GNU coreutils (`sha256sum`, `md5sum`)
- `<hash> *<filename>` — binary mode variant
- `<filename>:<hash>` and `<filename>=<hash>` — alternative formats

Auto-detected file names: `SHA256SUMS`, `sha256sums`, `MD5SUMS`, `checksums.txt`, `<filename>.sha256`, etc.

---

## Right-click integration

Open the application, go to **Settings** and toggle the context menu integration for your platform.

| Platform | Method |
|----------|--------|
| Windows | Registry (`HKCU\Software\Classes\*\shell\`) |
| Linux GNOME | Nautilus script (`~/.local/share/nautilus/scripts/`) |
| Linux KDE | Service menu (`~/.local/share/kio/servicemenus/`) |
| Linux XFCE | Thunar custom action (`~/.config/Thunar/uca.xml`) |

---

## Download

Pre-built binaries are available on the [Releases](../../releases) page.

| Platform | File |
|----------|------|
| Windows x64 | `hash_checker-windows-x64.exe` |
| Linux x64 | `hash_checker-linux-x64` |

---

## Build from source

**Requirements:** [Rust](https://rustup.rs) 1.75 or later.

```bash
# Clone the repository
git clone https://github.com/rusty-suite/hash_checker.git
cd hash_checker

# Debug build (development)
cargo build

# Release build (optimized, smaller binary)
cargo build --release
```

The compiled binary will be in `target/release/`.

**Cross-compile for Linux from Windows** (requires the target to be installed):

```bash
rustup target add x86_64-unknown-linux-gnu
cargo build --release --target x86_64-unknown-linux-gnu
```

---

## License

PolyForm Noncommercial
