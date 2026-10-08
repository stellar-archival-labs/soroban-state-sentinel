#!/usr/bin/env bash
#
# create-issue-backlog.sh — idempotently file the contributor-ready backlog for
# soroban-state-sentinel.
#
# Idempotent: skips any issue whose exact title already exists (open or closed),
# and uses `gh label create --force` so labels are created/updated safely.
# Re-running this script never creates duplicates.
#
# Usage:
#   ./scripts/create-issue-backlog.sh            # defaults to the org repo
#   GITHUB_REPOSITORY=owner/repo ./scripts/create-issue-backlog.sh
#
set -euo pipefail

REPO="${GITHUB_REPOSITORY:-stellar-archival-labs/soroban-state-sentinel}"
WAVE_LABEL="Stellar Wave"

gh label create "$WAVE_LABEL" --repo "$REPO" --force \
  --color "6f42c1" --description "Scoped for a Drips Wave contributor sprint" >/dev/null

EXISTING_TITLES="$(gh issue list --repo "$REPO" --state all --limit 500 --json title --jq '.[].title')"

mk() {
  local title="$1" body="$2"
  if printf '%s\n' "$EXISTING_TITLES" | grep -Fxq "$title"; then
    echo "skip (already exists): $title"
    return 0
  fi
  gh issue create --repo "$REPO" --title "$title" --label "$WAVE_LABEL" --body "$body" >/dev/null
  echo "created: $title"
}

mk "Measure recent ledger close time instead of assuming 5s" "$(cat <<'BODY'
## Summary
RPC does not expose the network's *actual* average ledger close time, so `scan`
falls back to a 5-second default labelled `default`. The sentinel already reads
`getLatestLedger.closeTime`; it can derive a real observed close time from a
short window of recent ledgers and label it `measured`.

## Why it matters
Every "days remaining" figure is derived from the close time. On a network whose
cadence is not exactly 5s, the default silently skews the ETA. Measuring makes
the projection honest without asking the user for `--ledger-close-seconds`.

## Acceptance Criteria
- [ ] Fetch `getLatestLedger` twice (or read the last N ledgers' close times) and compute an observed average.
- [ ] Use it as the default when `--ledger-close-seconds` is not passed.
- [ ] Label the source as `measured` (distinct from `default` / `explicit`) in `scan --json` and the human output.
- [ ] Keep `--ledger-close-seconds` as an explicit override that wins.
- [ ] Unit test the averaging + labelling; update SCHEMA.md's `ledger_close_seconds_source` values.

## Tech Stack / files to touch
Rust. `crates/rpc-client` (`getLatestLedger`), `crates/ttl-scanner`, `crates/cli` (args + output), `SCHEMA.md`.

## Out of scope
Changing the protocol-level 5s target; that stays as the documented fallback.
BODY
)"

mk "Add a --keys-file input for persistent storage keys" "$(cat <<'BODY'
## Summary
`--keys` takes repeatable base64-XDR SCVals on the command line. For contracts
with many keys this is unwieldy and shell-quoting-prone; accept a file of keys
(one per line, or a small YAML/JSON map) instead.

## Why it matters
Automation (cron, CI, keepers) needs a stable, reviewable input. A file is
easier to generate from a contract's known key set than a growing flag list.

## Acceptance Criteria
- [ ] New `--keys-file <PATH>` accepted alongside/replacing `--keys`.
- [ ] One base64-XDR SCVal per line; blank lines and `#` comments ignored.
- [ ] `--keys` and `--keys-file` may be combined; duplicates de-duplicated.
- [ ] Clear error (exit 2) on a malformed key, naming the file and line.
- [ ] README documents the format; unit test the parser.

## Tech Stack / files to touch
Rust. `crates/cli/src/args.rs`, `crates/cli/src/commands/scan.rs`, README.

## Out of scope
Auto-discovering keys (RPC has no list-all-keys method — see README Limitations).
BODY
)"

mk "Generate shell completions for bash, zsh, and fish" "$(cat <<'BODY'
## Summary
The CLI is clap-based, so shell completions are cheap to add but currently
absent. Ship a `completions <shell>` subcommand and document how to install them.

## Why it matters
Tab-completion of subcommands, flags, and thresholds measurably improves the
day-to-day experience for keepers running the tool interactively.

## Acceptance Criteria
- [ ] `soroban-state-sentinel completions bash|zsh|fish` writes a completion script to stdout.
- [ ] README shows the one-line install for each shell.
- [ ] A test asserts each shell emits non-empty output containing the subcommand names.
- [ ] `--help` mentions the subcommand.

## Tech Stack / files to touch
Rust, clap (`clap_complete`). `crates/cli/src/main.rs`, `crates/cli/src/commands/`, README.

## Out of scope
Packaging completions into the release tarball (can follow later).
BODY
)"

mk "Add macOS to the CI matrix" "$(cat <<'BODY'
## Summary
`.github/workflows/ci.yml` runs fmt/clippy/test on `ubuntu-latest` only, while
the release workflow builds for macOS too. Run the check job on macOS as well so
platform-specific breakage is caught before a tag.

## Why it matters
The tool is distributed as macOS binaries; a Linux-only CI matrix cannot catch
macOS-specific compile or test failures until release time.

## Acceptance Criteria
- [ ] `ci.yml` `check` job runs on a macOS runner in addition to Ubuntu (matrix).
- [ ] fmt/clippy/test all pass on both.
- [ ] The required branch-protection check name is updated if the job name changes.

## Tech Stack / files to touch
GitHub Actions. `.github/workflows/ci.yml`.

## Out of scope
Cross-compiling; this is native-runner CI only.
BODY
)"

mk "Document a mainnet usage guide" "$(cat <<'BODY'
## Summary
The docs cover testnet well but say little about running against mainnet, where
the stakes and the safe operating posture differ. Add a focused mainnet guide.

## Why it matters
Operators need explicit guidance on read-only scanning against mainnet, why the
tool never signs, how to hand the unsigned XDR to a signer, and what to verify
before extending/restoring real value.

## Acceptance Criteria
- [ ] New `docs-site/guides/for-keeper-operators.md` (or a new page) section covering mainnet.
- [ ] States the trust boundary: scanning is read-only; the tool produces unsigned XDR only.
- [ ] Covers choosing safe `extend_to` / threshold values on mainnet.
- [ ] Links from the README's docs list.

## Tech Stack / files to touch
Markdown. `docs-site/`, `README.md`.

## Out of scope
Automating signing or submission — deliberately excluded from this repo.
BODY
)"

mk "Add Prometheus metrics endpoint for health bands" "$(cat <<'BODY'
## Summary
Add an optional long-running mode that exposes a Prometheus `/metrics` endpoint
with gauges for each monitored contract's ledgers-remaining and health band, so
teams can alert on archival risk from their existing observability stack.

## Why it matters
CI-scheduled scans are pull-based and coarse; production teams want continuous
metrics they can graph and alert on alongside their other services.

## Acceptance Criteria
- [ ] New opt-in mode (e.g. `serve --listen 127.0.0.1:9464 --interval 60s`).
- [ ] `/metrics` exposes per-contract `ledgers_remaining` and a band gauge/label.
- [ ] Read-only: the mode never signs or submits.
- [ ] Documented in the CLI reference; unit test the metric rendering.

## Tech Stack / files to touch
Rust, a light HTTP server (e.g. `axum`) + `prometheus` crate. `crates/cli`.

## Out of scope
Long-term storage / dashboards; just expose the metrics.
BODY
)"

mk "Implement exponential backoff for testnet RPC rate limits" "$(cat <<'BODY'
## Summary
Public testnet RPC endpoints rate-limit. The client currently surfaces the RPC
error immediately; add bounded retry with exponential backoff for transient
failures (HTTP 429/5xx, connection resets).

## Why it matters
A scan that fails on one transient 429 wastes a scheduled run and produces a
misleading red. Backoff makes scheduled monitoring robust.

## Acceptance Criteria
- [ ] Retry on 429 and 5xx, up to a configurable max attempts.
- [ ] Exponential backoff with jitter; a `core.warning`-style log per retry.
- [ ] Non-retryable errors (bad args, malformed XDR) still fail fast.
- [ ] Unit test the retry policy (attempt count + that it eventually gives up).

## Tech Stack / files to touch
Rust. `crates/rpc-client/src/client.rs`, `crates/rpc-client/src/error.rs`.

## Out of scope
Client-side rate limiting / caching of responses.
BODY
)"

echo "backlog complete for $REPO"
