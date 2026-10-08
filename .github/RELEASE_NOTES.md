First tagged release.

- Binary: `soroban-state-sentinel` (crate: `sentinel-cli`)
- JSON schema version: `1.1.0` (see [SCHEMA.md](../SCHEMA.md))
- Trust boundary: read-only RPC scanning and unsigned XDR generation only — there is no signing capability anywhere in this repository (see [SECURITY.md](../SECURITY.md)).

## Assets

Each platform is published as `soroban-state-sentinel-<tag>-<target>.tar.gz`
containing the `soroban-state-sentinel` binary plus `LICENSE` and `README.md`,
with a matching `.sha256` checksum file.

| Target | Runner |
| --- | --- |
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` |
| `aarch64-apple-darwin` | `macos-15` |
| `x86_64-apple-darwin` | `macos-15-intel` |

Verify a download before running it:

```bash
sha256sum --check soroban-state-sentinel-<tag>-<target>.tar.gz.sha256
```
