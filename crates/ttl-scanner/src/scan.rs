//! The scan engine: fetch a contract's associated ledger entries and their TTL
//! entries over RPC and classify each into a health band.
//!
//! ## What is scanned
//!
//! A scan always covers:
//!
//! 1. **The contract instance** — `LedgerKey::ContractData` with the special
//!    `ScVal::LedgerKeyContractInstance` key (persistent).
//! 2. **The contract code** — `LedgerKey::ContractCode`, discovered from the
//!    wasm hash stored in the instance.
//! 3. **Any explicit storage keys** the caller provides (with an explicit
//!    durability).
//!
//! ## Real limitation
//!
//! The sentinel does **not** enumerate a contract's full persistent key set —
//! Soroban RPC offers no "list all keys of contract X" method, and a scan cannot
//! invent keys it cannot see. Users pass explicit keys for the entries they care
//! about. This is documented in the README and `SCHEMA.md`.

use sentinel_rpc_client::{GetLedgerEntriesResponse, NetworkInfo, RpcClient, RpcError};
use sentinel_xdr_builder::ttl_key_for;
use stellar_xdr::{
    ContractDataDurability, ContractExecutable, ContractId, Hash, LedgerEntryData, LedgerKey,
    LedgerKeyContractCode, LedgerKeyContractData, ScAddress, ScVal,
};

/// Sentinel value used by Soroban RPC for "no live-until info": an entry with
/// `liveUntilLedgerSeq == 0` has no associated live TTL (archived or non-TTL).
use crate::health::{classify, HealthBand, HealthConfig};

/// Default average ledger close time in seconds.
///
/// The Stellar network targets ~5s ledgers. RPC does not expose the *actual*
/// average, so this default is used only when the caller does not supply the
/// value explicitly; the CLI always labels the value as
/// `default|explicit` in its output so this assumption is never silent.
pub const DEFAULT_LEDGER_CLOSE_SECONDS: u64 = 5;

/// What kind of ledger entry a scan result row describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// The contract instance (`ScVal::LedgerKeyContractInstance`).
    ContractInstance,
    /// The contract code (wasm).
    ContractCode,
    /// An explicitly requested storage key.
    ContractData,
}

impl EntryKind {
    /// Stable machine-readable name (JSON schema).
    pub fn as_str(&self) -> &'static str {
        match self {
            EntryKind::ContractInstance => "contract_instance",
            EntryKind::ContractCode => "contract_code",
            EntryKind::ContractData => "contract_data",
        }
    }
}

/// One scanned entry with its TTL status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedEntry {
    /// Stable id for this row (JSON schema): `instance`, `code`, or `key.<index>`.
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// What kind of entry this is.
    pub kind: EntryKind,
    /// Durability, when known (contract data only).
    pub durability: Option<ContractDataDurability>,
    /// The data/code ledger key.
    pub key: LedgerKey,
    /// The TTL ledger key that governs this entry.
    pub ttl_key: LedgerKey,
    /// The live entry data, when present (`None` ⇒ archived/absent).
    pub entry: Option<LedgerEntryData>,
    /// Ledger sequence up to which the entry is live (units: ledger seq).
    pub live_until_ledger_seq: Option<u32>,
    /// Ledgers remaining until archival (units: ledgers).
    pub ledgers_remaining: Option<u32>,
    /// Health band classification.
    pub band: HealthBand,
    /// Approximate whole days remaining (floor), if computable.
    pub days_remaining: Option<u64>,
    /// Estimated Unix time (seconds) at which the entry would archive, if
    /// computable.
    pub estimated_archive_unix: Option<u64>,
    /// XDR size of the live entry in bytes (None if not live).
    pub size_bytes: Option<u32>,
    /// Whether this is a contract-code entry (affects rent fee discount).
    pub is_code_entry: bool,
}

/// Summary counts across all scanned entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanSummary {
    /// Total entries scanned.
    pub entries_scanned: usize,
    /// Count in each band.
    pub healthy: usize,
    pub expiring_soon: usize,
    pub critical: usize,
    pub archived: usize,
    /// True if any entry is Critical or Archived (drives `--fail-on-critical`).
    pub has_critical: bool,
}

/// Full scan result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanResult {
    /// Sequence of the latest ledger at scan time.
    pub latest_ledger: u32,
    /// Network info from `getNetwork`.
    pub network: NetworkInfo,
    /// The band configuration that was applied.
    pub health_config: HealthConfig,
    /// Seconds-per-ledger assumption that was applied.
    pub ledger_close_seconds: u64,
    /// Rows, one per scanned entry.
    pub entries: Vec<ScannedEntry>,
    /// Counts.
    pub summary: ScanSummary,
}

/// Options for a scan.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Contract id (raw 32 bytes) to scan.
    pub contract_id: Hash,
    /// Explicit storage keys to include (SCVal keys).
    pub explicit_keys: Vec<ScVal>,
    /// Durability for the explicit keys.
    pub explicit_durability: ContractDataDurability,
    /// Healthy lower bound in days.
    pub healthy_min_days: u32,
    /// Critical upper bound in days.
    pub critical_max_days: u32,
    /// Average ledger close time in seconds.
    pub ledger_close_seconds: u64,
}

/// Errors from the scan engine.
#[derive(Debug, thiserror::Error)]
pub enum ScannerError {
    /// Underlying RPC failure.
    #[error("RPC error: {0}")]
    Rpc(#[from] RpcError),

    /// The contract instance entry exists but does not contain the expected
    /// `ScVal::ContractInstance` payload.
    #[error("unexpected contract instance payload: {0:?}")]
    UnexpectedInstancePayload(ScVal),

    /// The contract instance ledger entry is not a contract-data entry.
    #[error("contract instance entry has unexpected shape: {0:?}")]
    UnexpectedInstanceEntry(LedgerEntryData),

    /// Ledger close time configuration is invalid (0).
    #[error("ledger_close_seconds must be > 0")]
    InvalidLedgerCloseSeconds,

    /// Invalid health-band configuration.
    #[error("invalid health configuration: {0}")]
    InvalidHealthConfig(String),

    /// Could not build a TTL key.
    #[error("TTL key derivation failed: {0}")]
    TtlKey(#[from] sentinel_xdr_builder::BuildError),
}

/// Build the contract-instance ledger key for a contract id.
pub fn instance_key(contract_id: Hash) -> LedgerKey {
    LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(ContractId(contract_id)),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
    })
}

/// Extract the wasm hash from a contract-instance entry, if it points at wasm.
fn wasm_hash_from_instance(entry: &LedgerEntryData) -> Result<Option<Hash>, ScannerError> {
    match entry {
        LedgerEntryData::ContractData(cd) => match &cd.val {
            ScVal::ContractInstance(instance) => match &instance.executable {
                ContractExecutable::Wasm(hash) => Ok(Some(hash.clone())),
                // Stellar asset / external-ref contracts have no wasm to scan.
                _ => Ok(None),
            },
            other => Err(ScannerError::UnexpectedInstancePayload(other.clone())),
        },
        other => Err(ScannerError::UnexpectedInstanceEntry(other.clone())),
    }
}

/// Group the fetched entries by their requested key for easy lookup.
fn index_entries(
    resp: &GetLedgerEntriesResponse,
) -> std::collections::HashMap<LedgerKey, sentinel_rpc_client::LedgerEntryInfo> {
    resp.entries
        .iter()
        .map(|info| (info.key.clone(), info.clone()))
        .collect()
}

/// Context shared by all rows of one scan.
#[derive(Debug, Clone, Copy)]
struct RowContext {
    latest_ledger: u32,
    close_time: u64,
    ledger_close_seconds: u64,
    config: HealthConfig,
}

#[allow(clippy::too_many_arguments)] // row assembly with explicit fields beats a wide options struct here
/// Build one scan row from an optional entry and its TTL info.
fn build_row(
    ctx: RowContext,
    id: String,
    label: String,
    kind: EntryKind,
    key: LedgerKey,
    ttl_key: LedgerKey,
    entry: Option<LedgerEntryData>,
    live_until: Option<u32>,
) -> ScannedEntry {
    let RowContext {
        latest_ledger,
        close_time,
        ledger_close_seconds,
        config,
    } = ctx;
    let live_until_ledger_seq = live_until;
    let ledgers_remaining = live_until_ledger_seq.map(|until| {
        // Defensive: the RPC should only return live entries (until >= latest),
        // but clamp negatives to 0.
        until.saturating_sub(latest_ledger)
    });

    let band = classify(ledgers_remaining, &config);

    let days_remaining = ledgers_remaining.map(|ledgers| {
        u64::from(ledgers)
            .saturating_mul(ledger_close_seconds)
            .checked_div(86_400)
            .unwrap_or(0)
    });

    let estimated_archive_unix = ledgers_remaining.and_then(|ledgers| {
        close_time.checked_add(u64::from(ledgers).saturating_mul(ledger_close_seconds))
    });
    let size_bytes = entry.as_ref().map(xdr_entry_size);
    let is_code_entry = matches!(entry.as_ref(), Some(LedgerEntryData::ContractCode(_)));

    ScannedEntry {
        id,
        label,
        kind,
        durability: entry.as_ref().and_then(|e| match e {
            LedgerEntryData::ContractData(cd) => Some(cd.durability),
            _ => None,
        }),
        key,
        ttl_key,
        entry,
        live_until_ledger_seq,
        ledgers_remaining,
        band,
        days_remaining,
        estimated_archive_unix,
        size_bytes,
        is_code_entry,
    }
}

/// XDR-encoded size of a ledger entry (data) in bytes.
fn xdr_entry_size(entry: &LedgerEntryData) -> u32 {
    use stellar_xdr::{Limits, WriteXdr};
    match entry.to_xdr(Limits::none()) {
        Ok(bytes) => u32::try_from(bytes.len()).unwrap_or(u32::MAX),
        Err(_) => 0,
    }
}

impl ScanOptions {
    /// Run the scan against the given RPC client.
    pub async fn scan(&self, rpc: &RpcClient) -> Result<ScanResult, ScannerError> {
        if self.ledger_close_seconds == 0 {
            return Err(ScannerError::InvalidLedgerCloseSeconds);
        }
        let health_config = HealthConfig::from_days(
            self.healthy_min_days,
            self.critical_max_days,
            self.ledger_close_seconds,
        )
        .map_err(ScannerError::InvalidHealthConfig)?;

        let latest = rpc.get_latest_ledger().await?;
        let network = rpc.get_network().await?;

        // Round 1: instance + explicit keys. TTL entries are NOT queried
        // directly — Soroban RPC rejects `LedgerKey::Ttl` requests with
        // "ledger ttl entries cannot be queried directly" (verified against the
        // live testnet RPC, protocol 28; also in soroban-rpc v28.0.1's
        // get_ledger_entries.go). The RPC populates `liveUntilLedgerSeq` on the
        // returned entries instead, which `ttl_live_until` reads.
        let instance_key = instance_key(self.contract_id.clone());
        let mut keys: Vec<LedgerKey> = vec![instance_key.clone()];
        let mut explicit_ledger_keys: Vec<LedgerKey> = Vec::new();
        for scval in &self.explicit_keys {
            let k = LedgerKey::ContractData(LedgerKeyContractData {
                contract: ScAddress::Contract(ContractId(self.contract_id.clone())),
                key: scval.clone(),
                durability: self.explicit_durability,
            });
            explicit_ledger_keys.push(k.clone());
            keys.push(k);
        }

        let resp1 = rpc.get_ledger_entries(&keys).await?;
        let mut index = index_entries(&resp1);

        let mut entries_out: Vec<ScannedEntry> = Vec::new();

        let ctx = RowContext {
            latest_ledger: latest.sequence,
            close_time: latest.close_time,
            ledger_close_seconds: self.ledger_close_seconds,
            config: health_config,
        };

        // --- instance row ---
        let instance_info = index.get(&instance_key).cloned();
        let instance_entry = instance_info.as_ref().map(|i| i.entry.clone());
        let instance_ttl = ttl_live_until(&index, &instance_key, instance_info.as_ref());
        entries_out.push(build_row(
            ctx,
            "instance".to_string(),
            "contract instance".to_string(),
            EntryKind::ContractInstance,
            instance_key.clone(),
            ttl_key_for(&instance_key)?,
            instance_entry.clone(),
            instance_ttl,
        ));

        // --- code row (only if instance was readable and is wasm) ---
        // The wasm hash is only known *after* the instance entry comes back, so
        // the code entry needs its own fetch round (round 2). Looking it up in
        // the round-1 response would always report it as archived.
        let wasm_hash = instance_entry
            .as_ref()
            .and_then(|e| wasm_hash_from_instance(e).ok().flatten());
        if let Some(hash) = wasm_hash {
            let code_key = LedgerKey::ContractCode(LedgerKeyContractCode { hash });
            let code_resp = rpc
                .get_ledger_entries(std::slice::from_ref(&code_key))
                .await?;
            index.extend(index_entries(&code_resp));
            let code_ttl_key = ttl_key_for(&code_key)?;
            let code_info = index.get(&code_key).cloned();
            let code_entry = code_info.as_ref().map(|i| i.entry.clone());
            let code_ttl = ttl_live_until(&index, &code_key, code_info.as_ref());
            entries_out.push(build_row(
                ctx,
                "code".to_string(),
                "contract code (wasm)".to_string(),
                EntryKind::ContractCode,
                code_key,
                code_ttl_key,
                code_entry,
                code_ttl,
            ));
        }

        // --- explicit key rows ---
        for (i, key) in explicit_ledger_keys.iter().enumerate() {
            let info = index.get(key).cloned();
            let entry = info.as_ref().map(|i| i.entry.clone());
            let ttl = ttl_live_until(&index, key, info.as_ref());
            entries_out.push(build_row(
                ctx,
                format!("key.{i}"),
                format!("storage key {i}"),
                EntryKind::ContractData,
                key.clone(),
                ttl_key_for(key)?,
                entry,
                ttl,
            ));
        }

        let healthy = entries_out
            .iter()
            .filter(|e| e.band == HealthBand::Healthy)
            .count();
        let expiring_soon = entries_out
            .iter()
            .filter(|e| e.band == HealthBand::ExpiringSoon)
            .count();
        let critical = entries_out
            .iter()
            .filter(|e| e.band == HealthBand::Critical)
            .count();
        let archived = entries_out
            .iter()
            .filter(|e| e.band == HealthBand::Archived)
            .count();

        let summary = ScanSummary {
            entries_scanned: entries_out.len(),
            healthy,
            expiring_soon,
            critical,
            archived,
            has_critical: critical > 0 || archived > 0,
        };

        Ok(ScanResult {
            latest_ledger: latest.sequence,
            network,
            health_config,
            ledger_close_seconds: self.ledger_close_seconds,
            entries: entries_out,
            summary,
        })
    }
}

/// Determine the live-until ledger for a key from the RPC-provided field.
///
/// Soroban RPC refuses to serve `LedgerKey::Ttl` entries directly, but
/// populates `liveUntilLedgerSeq` on the entries it returns, which is the value
/// the sentinel uses. A missing/zero value means the RPC did not surface a TTL
/// for the entry (it may be archived).
fn ttl_live_until(
    _index: &std::collections::HashMap<LedgerKey, sentinel_rpc_client::LedgerEntryInfo>,
    _data_key: &LedgerKey,
    data_info: Option<&sentinel_rpc_client::LedgerEntryInfo>,
) -> Option<u32> {
    data_info
        .and_then(|i| i.live_until_ledger_seq)
        .filter(|seq| *seq > 0)
}

#[cfg(test)]
mod round_two_tests {
    //! Regression coverage for the separate round-2 code-entry fetch.
    //!
    //! The contract-code ledger key is only discoverable *after* the contract
    //! instance has been fetched (its wasm hash lives in the instance entry), so
    //! the code entry MUST be fetched in its own `getLedgerEntries` round.
    //! Looking it up in the round-1 response reports it as archived. This test
    //! observes the actual request rounds and fails if the code key is not
    //! carried in a separate request from the instance key.

    use super::*;
    use crate::health::HealthBand;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use stellar_xdr::{
        ContractCodeEntry, ContractCodeEntryExt, ContractDataEntry, ContractExecutable, ContractId,
        ExtensionPoint, Hash, LedgerKeyContractCode, Limits, ScAddress, ScContractInstance,
        WriteXdr,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// base64 keys of each `getLedgerEntries` request, in request order.
    type Rounds = Arc<Mutex<Vec<Vec<String>>>>;

    fn http_ok(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn jsonrpc_error(msg: String) -> String {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": { "code": -32603, "message": msg }
        })
        .to_string();
        http_ok(&body)
    }

    fn handle(
        rounds: &Rounds,
        entries: &HashMap<String, (String, u32)>,
        latest: u32,
        body: &str,
    ) -> String {
        let req: serde_json::Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return jsonrpc_error(format!("bad request: {e}")),
        };
        let id = req["id"].clone();
        let method = req["method"].as_str().unwrap_or("");

        let result = match method {
            "getLatestLedger" => serde_json::json!({
                "id": "mock",
                "protocolVersion": 28,
                "sequence": latest,
                "closeTime": "1000000"
            }),
            "getNetwork" => serde_json::json!({
                "passphrase": "Test SDF Network ; September 2015",
                "protocolVersion": 28
            }),
            "getLedgerEntries" => {
                let keys: Vec<String> = req["params"]["keys"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|k| k.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                rounds.lock().unwrap().push(keys.clone());

                let mut out = Vec::new();
                for k in &keys {
                    if let Some((xdr, live_until)) = entries.get(k) {
                        out.push(serde_json::json!({
                            "key": k,
                            "xdr": xdr,
                            "lastModifiedLedgerSeq": 900,
                            "liveUntilLedgerSeq": live_until
                        }));
                    }
                }
                serde_json::json!({ "latestLedger": latest, "entries": out })
            }
            other => return jsonrpc_error(format!("unexpected method: {other}")),
        };

        let envelope =
            serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string();
        http_ok(&envelope)
    }

    async fn start_mock(entries: HashMap<String, (String, u32)>, latest: u32) -> (String, Rounds) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let rounds: Rounds = Arc::new(Mutex::new(Vec::new()));
        let rounds_out = rounds.clone();
        let entries = Arc::new(entries);

        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    continue;
                };
                let rounds = rounds.clone();
                let entries = entries.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 128 * 1024];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    let body = request
                        .split("\r\n\r\n")
                        .nth(1)
                        .unwrap_or("")
                        .trim_end()
                        .to_string();
                    let response = handle(&rounds, &entries, latest, &body);
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });

        (format!("http://{addr}"), rounds_out)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn code_entry_is_fetched_in_a_separate_round() {
        let contract = Hash([0x11u8; 32]);
        let wasm_hash = [0xCDu8; 32];
        let latest = 1_000u32;
        let live_until = latest + 600_000;

        let instance_scval = ScVal::ContractInstance(ScContractInstance {
            executable: ContractExecutable::Wasm(Hash(wasm_hash)),
            storage: None,
        });
        let inst_key = instance_key(contract.clone());
        let instance_entry = LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(ContractId(contract.clone())),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: instance_scval,
        });

        let code_key = LedgerKey::ContractCode(LedgerKeyContractCode {
            hash: Hash(wasm_hash),
        });
        let code_entry = LedgerEntryData::ContractCode(ContractCodeEntry {
            ext: ContractCodeEntryExt::V0,
            hash: Hash(wasm_hash),
            code: stellar_xdr::BytesM::try_from(vec![0u8; 8]).expect("bytes"),
        });

        let inst_key_b64 = inst_key.to_xdr_base64(Limits::none()).expect("encode");
        let code_key_b64 = code_key.to_xdr_base64(Limits::none()).expect("encode");

        let mut entries = HashMap::new();
        entries.insert(
            inst_key_b64.clone(),
            (
                instance_entry
                    .to_xdr_base64(Limits::none())
                    .expect("encode"),
                live_until,
            ),
        );
        entries.insert(
            code_key_b64.clone(),
            (
                code_entry.to_xdr_base64(Limits::none()).expect("encode"),
                live_until,
            ),
        );

        let (url, rounds) = start_mock(entries, latest).await;
        let rpc = sentinel_rpc_client::RpcClient::new(&url).expect("client");

        let opts = ScanOptions {
            contract_id: contract,
            explicit_keys: vec![],
            explicit_durability: ContractDataDurability::Persistent,
            healthy_min_days: 30,
            critical_max_days: 7,
            ledger_close_seconds: 5,
        };
        let result = opts.scan(&rpc).await.expect("scan succeeds");

        // The code row must exist and must not be misclassified as archived.
        let code_row = result
            .entries
            .iter()
            .find(|e| e.id == "code")
            .expect("a code row is produced when the instance is a wasm contract");
        assert_ne!(
            code_row.band,
            HealthBand::Archived,
            "code entry was reported archived — it was not fetched in its own round"
        );

        let rounds = rounds.lock().unwrap();
        let instance_round = rounds
            .iter()
            .position(|r| r.contains(&inst_key_b64))
            .expect("instance key was fetched");
        let code_round = rounds
            .iter()
            .position(|r| r.contains(&code_key_b64))
            .expect("code key was never fetched — it must be fetched in a separate round");

        assert_ne!(
            instance_round, code_round,
            "code key must not share a round with the instance key"
        );
        assert!(
            code_round > instance_round,
            "the code round must come after the instance round"
        );
    }
}
