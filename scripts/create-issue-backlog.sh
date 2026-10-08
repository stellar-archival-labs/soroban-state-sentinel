#!/usr/bin/env bash
set -euo pipefail
REPO="${GITHUB_REPOSITORY:-stellar-archival-labs/soroban-state-sentinel}"

mk() {
    gh issue create --repo "$REPO" --title "$1" --body "$2"
}

mk "Support multi-contract monitoring via bulk RPC requests" "$(cat <<'BODY'
## Summary
Currently, the sentinel monitors one contract/entry at a time. To support larger deployments, it should batch `getLedgerEntries` requests to check multiple contracts in a single RPC round-trip.
## Acceptance Criteria
- Accept a list of contract IDs or a config file mapping multiple entries.
- Use bulk RPC queries to fetch entry TTLs simultaneously.
- Output aggregate scan results in JSON format.
## Tech Stack
Rust, Soroban RPC, async/await
BODY
)"

mk "Add Prometheus metrics endpoint for health bands" "$(cat <<'BODY'
## Summary
For production operations, teams need to monitor TTL health bands using standard observability tools. The sentinel should expose a `/metrics` endpoint with Prometheus gauges for ledgers_remaining.
## Acceptance Criteria
- Add an HTTP server mode that runs in the background.
- Expose a Prometheus-compatible `/metrics` endpoint.
- Export metrics for each monitored contract's current TTL and health band.
## Tech Stack
Rust, actix-web / axum, prometheus crate
BODY
)"

mk "Implement exponential backoff for testnet RPC rate limits" "$(cat <<'BODY'
## Summary
Public testnet RPCs frequently rate-limit connections. The sentinel currently fails immediately if an RPC call is rejected. It should implement automatic retries with exponential backoff.
## Acceptance Criteria
- Detect HTTP 429 Too Many Requests.
- Automatically retry up to 5 times using exponential backoff (e.g., 1s, 2s, 4s, 8s, 16s).
- Log warnings on each retry attempt.
## Tech Stack
Rust, reqwest, tokio-retry
BODY
)"

mk "Publish prebuilt release binaries via GitHub Actions" "$(cat <<'BODY'
## Summary
Consumers currently have to compile the sentinel from source using Rust. Releasing prebuilt binaries for common platforms would significantly lower the barrier to entry.
## Acceptance Criteria
- Add a GitHub Actions workflow that triggers on tagged releases.
- Cross-compile for x86_64 Linux and macOS (Apple Silicon + Intel).
- Attach the binaries to the GitHub Release.
## Tech Stack
GitHub Actions, cross, Rust toolchain
BODY
)"

mk "Allow overriding the default ledger retention window dynamically" "$(cat <<'BODY'
## Summary
The ledger retention window is currently hardcoded or assumes testnet defaults. When running on standalone or mainnet, this window may differ. The sentinel should query the network configuration directly or allow user overrides.
## Acceptance Criteria
- Add a `--retention-window` CLI flag to override the default.
- Ideally, attempt to fetch the true network config automatically before falling back to defaults.
## Tech Stack
Rust, clap, Soroban RPC
BODY
)"

echo "5 backlog issues created on $REPO"
