//! Progress line parsers for rsync and rclone.
//!
//! Each parser is tolerant: a line that doesn't match returns `None` and is
//! treated as ordinary log output. Parsers turn a single line into a
//! [`Progress`] snapshot the UI can render (bar + speed + ETA).

use std::sync::OnceLock;

use regex::Regex;

use crate::job::Progress;

/// `1024^n` factor for a size unit like `k`, `M`, `Gi`, `MiB`, … (binary).
fn unit_factor(unit: &str) -> f64 {
    match unit.chars().next().map(|c| c.to_ascii_uppercase()) {
        Some('K') => 1024.0,
        Some('M') => 1024.0 * 1024.0,
        Some('G') => 1024.0 * 1024.0 * 1024.0,
        Some('T') => 1024.0 * 1024.0_f64.powi(4) / 1024.0, // 1024^4
        _ => 1.0,
    }
}

/// Parse a duration like `1h2m3s`, `2m42s`, `45s`, or `1.5s` into seconds.
fn parse_compact_duration(s: &str) -> Option<u64> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?:(?P<h>\d+)h)?(?:(?P<m>\d+)m)?(?:(?P<s>[\d.]+)s)?").unwrap()
    });
    let caps = re.captures(s)?;
    let h: u64 = caps.name("h").and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
    let m: u64 = caps.name("m").and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
    let sec: f64 = caps.name("s").and_then(|m| m.as_str().parse().ok()).unwrap_or(0.0);
    let total = h * 3600 + m * 60 + sec as u64;
    if total == 0 && caps.name("h").is_none() && caps.name("m").is_none() && caps.name("s").is_none()
    {
        None
    } else {
        Some(total)
    }
}

/// Parse an `H:MM:SS` clock duration into seconds.
fn parse_clock_duration(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let h: u64 = parts[0].parse().ok()?;
    let m: u64 = parts[1].parse().ok()?;
    let sec: u64 = parts[2].parse().ok()?;
    Some(h * 3600 + m * 60 + sec)
}

/// Parse an rsync `--info=progress2` line.
///
/// Example: `   1,234,567  45%   12.34MB/s    0:00:12 (xfr#3, to-chk=10/20)`
pub fn parse_rsync(line: &str) -> Option<Progress> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?P<bytes>[\d,]+)\s+(?P<pct>\d{1,3})%\s+(?P<rate>[\d.]+)(?P<unit>[kKmMgGtT]?)B/s(?:\s+(?P<eta>\d+:\d{2}:\d{2}))?",
        )
        .unwrap()
    });
    let caps = re.captures(line)?;
    let bytes_transferred =
        caps["bytes"].replace(',', "").parse::<u64>().unwrap_or(0);
    let percent = caps["pct"].parse::<f32>().ok();
    let rate: f64 = caps["rate"].parse().unwrap_or(0.0);
    let speed_bps = Some((rate * unit_factor(&caps["unit"])) as u64);
    let eta_secs = caps.name("eta").and_then(|m| parse_clock_duration(m.as_str()));

    Some(Progress {
        percent,
        bytes_transferred,
        files_done: 0,
        speed_bps,
        eta_secs,
    })
}

/// Parse an rclone `--stats-one-line` line.
///
/// Example: `Transferred:   1.234 MiB / 4.567 MiB, 27%, 1.234 MiB/s, ETA 2m42s`
pub fn parse_rclone(line: &str) -> Option<Progress> {
    if !line.contains("Transferred:") {
        return None;
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?P<done>[\d.]+)\s*(?P<dunit>[KMGT]?i?B)\s*/\s*[\d.]+\s*[KMGT]?i?B,\s*(?P<pct>\d{1,3})%,\s*(?P<rate>[\d.]+)\s*(?P<runit>[KMGT]?i?B)/s(?:,\s*ETA\s*(?P<eta>\S+))?",
        )
        .unwrap()
    });
    let caps = re.captures(line)?;
    let done: f64 = caps["done"].parse().unwrap_or(0.0);
    let bytes_transferred = (done * unit_factor(&caps["dunit"])) as u64;
    let percent = caps["pct"].parse::<f32>().ok();
    let rate: f64 = caps["rate"].parse().unwrap_or(0.0);
    let speed_bps = Some((rate * unit_factor(&caps["runit"])) as u64);
    let eta_secs = caps
        .name("eta")
        .map(|m| m.as_str())
        .filter(|s| *s != "-")
        .and_then(parse_compact_duration);

    Some(Progress {
        percent,
        bytes_transferred,
        files_done: 0,
        speed_bps,
        eta_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rsync_line_parses() {
        let p = parse_rsync("   1,234,567  45%   12.34MB/s    0:00:12 (xfr#3, to-chk=10/20)").unwrap();
        assert_eq!(p.bytes_transferred, 1_234_567);
        assert_eq!(p.percent, Some(45.0));
        assert_eq!(p.eta_secs, Some(12));
        assert!(p.speed_bps.unwrap() > 12_000_000 && p.speed_bps.unwrap() < 13_000_000);
    }

    #[test]
    fn rsync_completion_line() {
        let p = parse_rsync("32,768 100% 0.00kB/s 0:00:00").unwrap();
        assert_eq!(p.percent, Some(100.0));
        assert_eq!(p.bytes_transferred, 32_768);
    }

    #[test]
    fn rsync_non_progress_line_is_none() {
        assert!(parse_rsync("sending incremental file list").is_none());
        assert!(parse_rsync("photo1.jpg").is_none());
    }

    #[test]
    fn rclone_line_parses() {
        let p = parse_rclone(
            "Transferred:   \t  1.000 MiB / 4.000 MiB, 25%, 2.000 MiB/s, ETA 2m42s",
        )
        .unwrap();
        assert_eq!(p.percent, Some(25.0));
        assert_eq!(p.bytes_transferred, 1024 * 1024);
        assert_eq!(p.speed_bps, Some(2 * 1024 * 1024));
        assert_eq!(p.eta_secs, Some(162));
    }

    #[test]
    fn rclone_eta_dash_is_none() {
        let p = parse_rclone("Transferred: 0 B / 0 B, 0%, 0 B/s, ETA -").unwrap();
        assert_eq!(p.eta_secs, None);
    }

    #[test]
    fn rclone_non_stats_line_is_none() {
        assert!(parse_rclone("2024/01/01 INFO  : something happened").is_none());
    }
}
