//! rclone Remote Control (RC) API: submit a transfer as an async job on the
//! local `rcd` daemon and poll its rich, per-file statistics.
//!
//! The CLI path parses `--stats-one-line` and only knows an aggregate figure.
//! The RC path drives the transfer *inside* the daemon and reads `core/stats`,
//! which returns a `transferring[]` array — one entry per in-flight file with
//! its own bytes/size/percentage/speed/ETA — so the UI can show a real transfer
//! monitor. All JSON shapes here are matched against live rclone output.
//!
//! Only **argv and JSON** cross the process boundary: credentials go via the
//! environment (see [`super::rcd`]); this module is pure and unit-testable.

use serde::Deserialize;

use super::command::{RcloneOp, RcloneOptions};

/// One in-flight file from `core/stats.transferring[]`.
#[derive(Debug, Clone, PartialEq)]
pub struct RcTransfer {
    pub name: String,
    pub bytes: u64,
    /// Total size of this file, or `None` when the backend can't report it
    /// (rclone uses `-1`).
    pub size: Option<u64>,
    /// 0..=100.
    pub percentage: u8,
    pub speed_bps: u64,
    pub eta_secs: Option<u64>,
}

/// A snapshot of `core/stats` for one transfer group.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RcStats {
    pub bytes: u64,
    pub total_bytes: u64,
    pub speed_bps: u64,
    pub eta_secs: Option<u64>,
    pub errors: u64,
    /// Files finished so far.
    pub transfers_done: u64,
    /// Total files to transfer.
    pub transfers_total: u64,
    pub transferring: Vec<RcTransfer>,
}

impl RcStats {
    /// Overall completion 0.0..=1.0, when the total is known.
    pub fn fraction(&self) -> Option<f64> {
        if self.total_bytes > 0 {
            Some((self.bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0))
        } else {
            None
        }
    }
}

/// The terminal state of an async RC job (`job/status`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RcJobStatus {
    pub finished: bool,
    pub success: bool,
    /// Empty unless the job failed.
    pub error: String,
}

/// The RC method that runs an [`RcloneOp`] as a whole-filesystem transfer, or
/// `None` for ops the RC path does not drive (read-only ops, bisync).
pub fn method_for(op: RcloneOp) -> Option<&'static str> {
    match op {
        RcloneOp::Copy => Some("sync/copy"),
        RcloneOp::Sync => Some("sync/sync"),
        RcloneOp::Move => Some("sync/move"),
        _ => None,
    }
}

/// Build the JSON payload for an async `sync/{copy,sync,move}` RC call.
///
/// `group` scopes `core/stats` to this run (use a unique value per run, e.g.
/// `job/<run_id>`). Options map onto rclone's `_config` (global settings) and
/// `_filter` (include/exclude) objects. Custom `extra_flags` are **not**
/// representable here — the caller should keep those jobs on the CLI path.
pub fn sync_payload(
    src: &str,
    dst: &str,
    group: &str,
    dry_run: bool,
    opts: &RcloneOptions,
) -> String {
    use serde_json::{json, Map, Value};

    let mut config = Map::new();
    if dry_run {
        config.insert("DryRun".into(), json!(true));
    }
    if let Some(t) = opts.transfers {
        config.insert("Transfers".into(), json!(t));
    }
    if let Some(c) = opts.checkers {
        config.insert("Checkers".into(), json!(c));
    }
    if opts.checksum {
        config.insert("CheckSum".into(), json!(true));
    }
    if let Some(b) = &opts.bwlimit {
        config.insert("BwLimit".into(), json!(b));
    }
    if let Some(r) = opts.retries {
        config.insert("Retries".into(), json!(r));
    }
    if let Some(m) = opts.max_delete {
        config.insert("MaxDelete".into(), json!(m));
    }
    if let Some(dir) = &opts.backup_dir {
        config.insert("BackupDir".into(), json!(dir));
    }

    let mut filter = Map::new();
    if !opts.excludes.is_empty() {
        filter.insert("ExcludeRule".into(), json!(opts.excludes));
    }
    if !opts.includes.is_empty() {
        filter.insert("IncludeRule".into(), json!(opts.includes));
    }

    let mut payload = Map::new();
    payload.insert("srcFs".into(), json!(src));
    payload.insert("dstFs".into(), json!(dst));
    payload.insert("_async".into(), json!(true));
    payload.insert("_group".into(), json!(group));
    if !config.is_empty() {
        payload.insert("_config".into(), Value::Object(config));
    }
    if !filter.is_empty() {
        payload.insert("_filter".into(), Value::Object(filter));
    }
    Value::Object(payload).to_string()
}

/// Parse the `{"jobid": N}` response of an `_async` RC submission.
pub fn parse_jobid(json: &str) -> Option<u64> {
    #[derive(Deserialize)]
    struct R {
        jobid: u64,
    }
    serde_json::from_str::<R>(json).ok().map(|r| r.jobid)
}

/// Parse a `core/stats` response into an [`RcStats`].
pub fn parse_core_stats(json: &str) -> Option<RcStats> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        bytes: u64,
        #[serde(default, rename = "totalBytes")]
        total_bytes: u64,
        #[serde(default)]
        speed: f64,
        #[serde(default)]
        eta: Option<f64>,
        #[serde(default)]
        errors: u64,
        #[serde(default)]
        transfers: u64,
        #[serde(default, rename = "totalTransfers")]
        total_transfers: u64,
        #[serde(default)]
        transferring: Vec<RawXfer>,
    }
    #[derive(Deserialize)]
    struct RawXfer {
        #[serde(default)]
        name: String,
        #[serde(default)]
        bytes: u64,
        #[serde(default)]
        size: i64,
        #[serde(default)]
        percentage: u8,
        #[serde(default, rename = "speedAvg")]
        speed_avg: f64,
        #[serde(default)]
        eta: Option<f64>,
    }

    let raw: Raw = serde_json::from_str(json).ok()?;
    let transferring = raw
        .transferring
        .into_iter()
        .map(|x| RcTransfer {
            name: x.name,
            bytes: x.bytes,
            size: if x.size > 0 {
                Some(x.size as u64)
            } else {
                None
            },
            percentage: x.percentage.min(100),
            speed_bps: x.speed_avg.max(0.0) as u64,
            eta_secs: to_eta(x.eta),
        })
        .collect();
    Some(RcStats {
        bytes: raw.bytes,
        total_bytes: raw.total_bytes,
        speed_bps: raw.speed.max(0.0) as u64,
        eta_secs: to_eta(raw.eta),
        errors: raw.errors,
        transfers_done: raw.transfers,
        transfers_total: raw.total_transfers,
        transferring,
    })
}

/// Parse a `job/status` response into an [`RcJobStatus`].
pub fn parse_job_status(json: &str) -> Option<RcJobStatus> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        finished: bool,
        #[serde(default)]
        success: bool,
        #[serde(default)]
        error: String,
    }
    let raw: Raw = serde_json::from_str(json).ok()?;
    Some(RcJobStatus {
        finished: raw.finished,
        success: raw.success,
        error: raw.error,
    })
}

/// rclone reports ETA as seconds, `0` (unknown/none for the overall figure), or
/// `null`. Treat a non-positive/absent value as "unknown".
fn to_eta(v: Option<f64>) -> Option<u64> {
    match v {
        Some(s) if s > 0.0 => Some(s as u64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured verbatim from a live `rclone rc core/stats` mid-transfer.
    const STATS_MID: &str = r#"{
        "bytes": 29384704, "checks": 0, "elapsedTime": 3.01, "errors": 0,
        "eta": 21, "speed": 9762142.07, "totalBytes": 240000000,
        "totalTransfers": 2, "transfers": 0,
        "transferring": [
            {"bytes":16871424,"eta":18,"group":"job/1","name":"big1.bin","percentage":14,"size":120000000,"speed":5625699.5,"speedAvg":5612708.3},
            {"bytes":12513280,"eta":25,"group":"job/1","name":"big2.bin","percentage":10,"size":120000000,"speed":4190675.9,"speedAvg":4149182.3}
        ]
    }"#;

    #[test]
    fn parses_mid_transfer_stats() {
        let s = parse_core_stats(STATS_MID).unwrap();
        assert_eq!(s.bytes, 29_384_704);
        assert_eq!(s.total_bytes, 240_000_000);
        assert_eq!(s.speed_bps, 9_762_142);
        assert_eq!(s.eta_secs, Some(21));
        assert_eq!(s.transfers_total, 2);
        assert_eq!(s.transfers_done, 0);
        assert_eq!(s.transferring.len(), 2);

        let f0 = &s.transferring[0];
        assert_eq!(f0.name, "big1.bin");
        assert_eq!(f0.bytes, 16_871_424);
        assert_eq!(f0.size, Some(120_000_000));
        assert_eq!(f0.percentage, 14);
        assert_eq!(f0.speed_bps, 5_612_708); // speedAvg, truncated
        assert_eq!(f0.eta_secs, Some(18));

        // Overall fraction ~12%.
        let frac = s.fraction().unwrap();
        assert!(frac > 0.11 && frac < 0.13, "fraction was {frac}");
    }

    #[test]
    fn parses_completed_stats_without_transferring() {
        let json = r#"{"bytes":83886080,"totalBytes":83886080,"speed":41943229.0,
            "eta":0,"errors":0,"transfers":2,"totalTransfers":2}"#;
        let s = parse_core_stats(json).unwrap();
        assert!(s.transferring.is_empty());
        assert_eq!(s.transfers_done, 2);
        assert_eq!(s.eta_secs, None); // eta 0 => unknown/done
        assert_eq!(s.fraction(), Some(1.0));
    }

    #[test]
    fn unknown_size_becomes_none() {
        let json = r#"{"transferring":[{"name":"x","bytes":10,"size":-1,"percentage":0,"speedAvg":0,"eta":null}]}"#;
        let s = parse_core_stats(json).unwrap();
        assert_eq!(s.transferring[0].size, None);
        assert_eq!(s.transferring[0].eta_secs, None);
    }

    #[test]
    fn parses_job_status() {
        let running = parse_job_status(r#"{"finished":false,"success":false,"error":""}"#).unwrap();
        assert!(!running.finished);

        let ok = parse_job_status(
            r#"{"duration":0.1,"error":"","finished":true,"group":"job/1","id":1,"success":true}"#,
        )
        .unwrap();
        assert!(ok.finished && ok.success && ok.error.is_empty());

        let failed =
            parse_job_status(r#"{"finished":true,"success":false,"error":"boom"}"#).unwrap();
        assert!(failed.finished && !failed.success);
        assert_eq!(failed.error, "boom");
    }

    #[test]
    fn parses_async_jobid() {
        assert_eq!(parse_jobid(r#"{"jobid":7}"#), Some(7));
        assert_eq!(parse_jobid(r#"{"nope":1}"#), None);
        assert_eq!(parse_jobid("not json"), None);
    }

    #[test]
    fn method_mapping() {
        assert_eq!(method_for(RcloneOp::Copy), Some("sync/copy"));
        assert_eq!(method_for(RcloneOp::Sync), Some("sync/sync"));
        assert_eq!(method_for(RcloneOp::Move), Some("sync/move"));
        assert_eq!(method_for(RcloneOp::Bisync), None);
        assert_eq!(method_for(RcloneOp::Check), None);
    }

    #[test]
    fn payload_maps_options_to_config_and_filter() {
        let opts = RcloneOptions {
            transfers: Some(8),
            checkers: Some(16),
            bwlimit: Some("10M".into()),
            max_delete: Some(5),
            backup_dir: Some("/trash".into()),
            checksum: true,
            excludes: vec!["*.tmp".into()],
            includes: vec!["*.jpg".into()],
            ..Default::default()
        };
        let payload = sync_payload("gdrive:a", "/local/b", "job/42", true, &opts);
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();

        assert_eq!(v["srcFs"], "gdrive:a");
        assert_eq!(v["dstFs"], "/local/b");
        assert_eq!(v["_async"], true);
        assert_eq!(v["_group"], "job/42");
        assert_eq!(v["_config"]["DryRun"], true);
        assert_eq!(v["_config"]["Transfers"], 8);
        assert_eq!(v["_config"]["Checkers"], 16);
        assert_eq!(v["_config"]["BwLimit"], "10M");
        assert_eq!(v["_config"]["MaxDelete"], 5);
        assert_eq!(v["_config"]["BackupDir"], "/trash");
        assert_eq!(v["_config"]["CheckSum"], true);
        assert_eq!(v["_filter"]["ExcludeRule"][0], "*.tmp");
        assert_eq!(v["_filter"]["IncludeRule"][0], "*.jpg");
    }

    #[test]
    fn payload_omits_empty_config_and_filter() {
        let payload = sync_payload("/a", "/b", "g", false, &RcloneOptions::default());
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert!(v.get("_config").is_none(), "empty config must be omitted");
        assert!(v.get("_filter").is_none(), "empty filter must be omitted");
        assert_eq!(v["_async"], true);
    }
}
