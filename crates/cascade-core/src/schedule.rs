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
///
/// `on_failure_unit`, when set, becomes an `OnFailure=` directive so systemd
/// runs that unit if the job fails (see [`build_notify_unit`]). It must be a
/// unit name we control (never user free-text).
pub fn build_units(
    name: &str,
    binary_path: &str,
    argv: &[String],
    on_calendar: &str,
    on_failure_unit: Option<&str>,
) -> ScheduleUnit {
    let id = unit_id(name);
    let service_name = format!("cascade-{id}.service");
    let timer_name = format!("cascade-{id}.timer");
    let desc = one_line(name);
    let exec = exec_start(binary_path, argv);

    let mut unit_section = format!("[Unit]\nDescription=Cascade job: {desc}\n");
    if let Some(u) = on_failure_unit {
        unit_section.push_str(&format!("OnFailure={u}\n"));
    }
    let service = format!(
        "{unit_section}\
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

/// The file name of the shared failure-notification template unit.
pub const NOTIFY_UNIT_FILE: &str = "cascade-notify@.service";

/// The `OnFailure=` instance to attach to a job named `name`. systemd passes the
/// job id as the template instance (`%i`), which the notification shows.
pub fn notify_instance_for(name: &str) -> String {
    format!("cascade-notify@{}.service", unit_id(name))
}

/// Build the shared `cascade-notify@.service` template unit. One instance is
/// started per failing job (via `OnFailure=`); `%i` is the failing job's id.
///
/// `notify_send_path` is the absolute path to `notify-send`; `title` is the
/// (already localized) notification summary. Both are systemd-quoted; `%i` is a
/// systemd specifier and is intentionally left unquoted (the id is a safe slug).
pub fn build_notify_unit(notify_send_path: &str, title: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Cascade scheduled-job failure notification for %i\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={} {} %i\n",
        systemd_quote(notify_send_path),
        systemd_quote(title),
    )
}

/// Read the `OnCalendar=` value out of a `.timer` file's contents.
pub fn parse_on_calendar(timer_contents: &str) -> Option<String> {
    timer_contents.lines().find_map(|l| {
        l.trim()
            .strip_prefix("OnCalendar=")
            .map(|v| v.trim().to_string())
    })
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
///
/// `%` is **not** treated as simple — systemd reads it as a specifier (`%h`,
/// `%i`, …), so it is always doubled (`%%`). Control characters (notably a
/// newline, which would otherwise split the line and let a crafted path inject
/// a new unit directive) are C-escaped inside the quotes.
fn systemd_quote(s: &str) -> String {
    let simple = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@+,".contains(c));
    if simple {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '%' => out.push_str("%%"),
            c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Strip control characters (incl. newlines) so a name can't break the file.
fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
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
        let u = build_units("My Job", "/usr/bin/rclone", &argv, "daily", None);
        assert_eq!(u.service_name, "cascade-my-job.service");
        assert_eq!(u.timer_name, "cascade-my-job.timer");
        assert!(u.service.contains("Type=oneshot"));
        // Binary and a space-containing arg are present and quoted.
        assert!(u
            .service
            .contains("ExecStart=/usr/bin/rclone copy \"/my data\" gdrive:b"));
        assert!(!u.service.contains("OnFailure="));
        assert!(u.timer.contains("OnCalendar=daily"));
        assert!(u.timer.contains("WantedBy=timers.target"));
        assert!(u.timer.contains("Persistent=true"));
    }

    #[test]
    fn on_failure_directive_is_added_when_requested() {
        let unit = notify_instance_for("Nightly Backup");
        assert_eq!(unit, "cascade-notify@nightly-backup.service");
        let u = build_units(
            "Nightly Backup",
            "/usr/bin/rsync",
            &[],
            "daily",
            Some(&unit),
        );
        assert!(u
            .service
            .contains("OnFailure=cascade-notify@nightly-backup.service"));
        // Exactly one OnFailure line, inside [Unit] and before [Service].
        let on_fail = u.service.find("OnFailure=").unwrap();
        let service_hdr = u.service.find("[Service]").unwrap();
        assert!(on_fail < service_hdr);
    }

    #[test]
    fn notify_unit_quotes_path_and_title_but_keeps_specifier() {
        let unit = build_notify_unit("/usr/bin/notify-send", "Scheduled job failed");
        // Title is quoted (has a space); %i is left as a live systemd specifier.
        assert!(unit.contains("ExecStart=/usr/bin/notify-send \"Scheduled job failed\" %i"));
        assert!(unit.contains("Type=oneshot"));
    }

    #[test]
    fn reads_on_calendar_back() {
        let u = build_units(
            "x",
            "/usr/bin/rsync",
            &["-a".into()],
            "Mon *-*-* 09:00",
            None,
        );
        assert_eq!(
            parse_on_calendar(&u.timer).as_deref(),
            Some("Mon *-*-* 09:00")
        );
        assert_eq!(parse_on_calendar("[Timer]\n"), None);
    }

    #[test]
    fn quoting_escapes_special_chars() {
        assert_eq!(systemd_quote("/plain/path"), "/plain/path");
        assert_eq!(systemd_quote("a b"), "\"a b\"");
        assert_eq!(systemd_quote(r#"a"b"#), r#""a\"b""#);
    }

    #[test]
    fn quoting_neutralizes_systemd_specifiers_and_newlines() {
        // '%' must be doubled so it isn't read as a specifier like %h.
        assert_eq!(systemd_quote("back%h"), "\"back%%h\"");
        // A newline must be escaped, not written literally (no directive injection).
        let q = systemd_quote("a\nExecStartPre=/bin/rm");
        assert!(!q.contains('\n'), "newline leaked into the quoted arg");
        assert!(q.contains("\\n"));
        // And it shows up escaped in a full unit too.
        let u = build_units("x", "/usr/bin/rsync", &["a\nb".into()], "daily", None);
        assert!(!u.service.lines().any(|l| l == "ExecStartPre=/bin/rm"));
    }

    #[test]
    fn unit_id_edge_cases() {
        // Non-ASCII and punctuation become dashes, which collapse and trim; only
        // ASCII alphanumerics survive.
        assert_eq!(unit_id("Ünïcode → Bäckup!!!"), "n-code-b-ckup");
        // Digits and underscores/dashes are preserved.
        assert_eq!(unit_id("job_42-v2"), "job_42-v2");
        // Uppercase is lowercased.
        assert_eq!(unit_id("BACKUP"), "backup");
        // Empty / all-separators fall back to "job".
        assert_eq!(unit_id(""), "job");
        assert_eq!(unit_id("   "), "job");
        assert_eq!(unit_id("///"), "job");
    }

    #[test]
    fn systemd_quote_empty_string_is_quoted() {
        // An empty arg must not vanish; it becomes an explicit empty quoted arg.
        assert_eq!(systemd_quote(""), "\"\"");
    }

    #[test]
    fn notify_instance_slugifies_the_name() {
        assert_eq!(
            notify_instance_for("My Nightly Job"),
            "cascade-notify@my-nightly-job.service"
        );
        assert_eq!(NOTIFY_UNIT_FILE, "cascade-notify@.service");
    }

    #[test]
    fn notify_unit_quotes_a_spacey_notify_path() {
        let unit = build_notify_unit("/opt/my tools/notify-send", "Failed");
        assert!(unit.contains("ExecStart=\"/opt/my tools/notify-send\" Failed %i"));
    }

    #[test]
    fn parse_on_calendar_trims_and_handles_absence() {
        assert_eq!(
            parse_on_calendar("[Timer]\nOnCalendar=   *-*-* 02:00:00  \n").as_deref(),
            Some("*-*-* 02:00:00")
        );
        assert_eq!(parse_on_calendar("").as_deref(), None);
        assert_eq!(parse_on_calendar("OnCalendarish=weekly").as_deref(), None);
    }

    #[test]
    fn timer_always_has_persistent_and_install() {
        let u = build_units("j", "/bin/true", &[], "hourly", None);
        assert!(u.timer.contains("Persistent=true"));
        assert!(u.timer.contains("[Install]"));
        assert!(u.timer.contains("WantedBy=timers.target"));
    }

    #[test]
    fn exec_start_quotes_only_args_that_need_it() {
        let argv = vec!["copy".into(), "/plain".into(), "has space".into()];
        let u = build_units("j", "/usr/bin/rclone", &argv, "daily", None);
        assert!(u
            .service
            .contains("ExecStart=/usr/bin/rclone copy /plain \"has space\""));
    }

    #[test]
    fn description_strips_control_chars() {
        let u = build_units(
            "evil\n[Service]\nExecStart=/bin/rm",
            "/bin/true",
            &[],
            "daily",
            None,
        );
        // The injected newline is gone from the Description line.
        assert!(u
            .service
            .contains("Description=Cascade job: evil [Service] ExecStart=/bin/rm"));
    }
}
