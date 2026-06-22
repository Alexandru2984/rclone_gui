//! Generating systemd **user** units to schedule a job.
//!
//! Instead of running our own always-on daemon, Cascade exports a job as a
//! `oneshot` `.service` plus a `.timer` under `~/.config/systemd/user/`. systemd
//! becomes the scheduler (the modern cron); scheduled runs are independent of
//! the app and visible via `journalctl --user`.
//!
//! This module only produces the unit *contents* and file names — writing them
//! and invoking `systemctl --user` is the caller's job.

/// The two unit files that make up a scheduled job.
#[derive(Debug, Clone)]
pub struct ScheduleUnit {
    pub service_name: String,
    pub timer_name: String,
    pub service: String,
    pub timer: String,
}

/// Turn an arbitrary job name into a safe systemd unit id fragment.
pub fn unit_id(name: &str) -> String {
    let mut id: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while id.contains("--") {
        id = id.replace("--", "-");
    }
    let id = id.trim_matches('-').to_string();
    if id.is_empty() {
        "job".to_string()
    } else {
        id
    }
}

/// Build the `.service` + `.timer` contents for a job.
///
/// `binary_path` should be absolute; `argv` is the exact argument vector (the
/// same one the runner uses). `on_calendar` is a systemd `OnCalendar=` value
/// such as `daily`, `hourly`, or `*-*-* 02:00:00`.
pub fn build_units(
    name: &str,
    binary_path: &str,
    argv: &[String],
    on_calendar: &str,
) -> ScheduleUnit {
    let id = unit_id(name);
    let service_name = format!("cascade-{id}.service");
    let timer_name = format!("cascade-{id}.timer");
    let desc = one_line(name);
    let exec = exec_start(binary_path, argv);

    let service = format!(
        "[Unit]\n\
         Description=Cascade job: {desc}\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={exec}\n"
    );
    let timer = format!(
        "[Unit]\n\
         Description=Cascade schedule: {desc}\n\
         \n\
         [Timer]\n\
         OnCalendar={on_calendar}\n\
         Persistent=true\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n"
    );

    ScheduleUnit {
        service_name,
        timer_name,
        service,
        timer,
    }
}

/// Build a systemd `ExecStart=` line from a binary and argv, quoting as needed.
fn exec_start(binary_path: &str, argv: &[String]) -> String {
    let mut out = systemd_quote(binary_path);
    for a in argv {
        out.push(' ');
        out.push_str(&systemd_quote(a));
    }
    out
}

/// Quote a single argument for a systemd `ExecStart=` line.
fn systemd_quote(s: &str) -> String {
    let simple = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c));
    if simple {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// Collapse newlines so a name can't break the unit file.
fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_id_is_slugified() {
        assert_eq!(unit_id("Nightly Backup → Drive"), "nightly-backup-drive");
        assert_eq!(unit_id("  --weird-- "), "weird");
        assert_eq!(unit_id("***"), "job");
    }

    #[test]
    fn units_contain_expected_directives() {
        let argv = vec![
            "copy".to_string(),
            "/my data".to_string(),
            "gdrive:b".to_string(),
        ];
        let u = build_units("My Job", "/usr/bin/rclone", &argv, "daily");
        assert_eq!(u.service_name, "cascade-my-job.service");
        assert_eq!(u.timer_name, "cascade-my-job.timer");
        assert!(u.service.contains("Type=oneshot"));
        // Binary and a space-containing arg are present and quoted.
        assert!(u
            .service
            .contains("ExecStart=/usr/bin/rclone copy \"/my data\" gdrive:b"));
        assert!(u.timer.contains("OnCalendar=daily"));
        assert!(u.timer.contains("WantedBy=timers.target"));
        assert!(u.timer.contains("Persistent=true"));
    }

    #[test]
    fn quoting_escapes_special_chars() {
        assert_eq!(systemd_quote("/plain/path"), "/plain/path");
        assert_eq!(systemd_quote("a b"), "\"a b\"");
        assert_eq!(systemd_quote(r#"a"b"#), r#""a\"b""#);
    }
}
