//! Backup Assistant: predefined scenarios that pre-configure a [`JobSpec`].
//!
//! Each scenario picks the right tool, operation and delete behavior, and gives
//! plain-language guidance plus a risk level. The GUI collects the source and
//! destination and hands the resulting spec to the New Job screen, where the
//! usual dry-run / destructive-confirmation gates apply.

use crate::job::{AdvancedOptions, JobSpec, OpKind};
use crate::security::destructive::RiskLevel;
use crate::Tool;

/// A guided backup scenario.
#[derive(Debug, Clone, Copy)]
pub struct Scenario {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// Extra caution text shown for risky scenarios (empty when not needed).
    pub note: &'static str,
    pub tool: Tool,
    pub op: OpKind,
    pub delete: bool,
    /// Whether the UI should strongly recommend a dry-run first.
    pub recommend_dry_run: bool,
    pub source_hint: &'static str,
    pub dest_hint: &'static str,
    /// Default exclude patterns applied to the generated spec.
    pub excludes: &'static [&'static str],
}

impl Scenario {
    /// Build a concrete spec from user-provided source/destination.
    pub fn to_spec(&self, source: &str, destination: &str) -> JobSpec {
        JobSpec {
            name: self.title.to_string(),
            tool: self.tool,
            op: self.op,
            source: source.to_string(),
            destination: destination.to_string(),
            dry_run: false,
            delete: self.delete,
            options: AdvancedOptions {
                excludes: self.excludes.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
        }
    }

    /// Risk level of this scenario (uses the same classifier as everything else).
    pub fn risk(&self) -> RiskLevel {
        // Build a throwaway spec just to reuse the risk classification.
        self.to_spec(self.source_hint, self.dest_hint).risk()
    }
}

/// The built-in scenarios, in display order.
pub fn builtin_scenarios() -> Vec<Scenario> {
    use OpKind::*;
    use Tool::*;
    vec![
        Scenario {
            id: "backup_photos",
            title: "Back up photos",
            description: "Copy a photo folder to another disk or location. Never deletes.",
            note: "",
            tool: Rsync,
            op: Copy,
            delete: false,
            recommend_dry_run: false,
            source_hint: "~/Pictures",
            dest_hint: "/mnt/backup/Photos",
            excludes: &[],
        },
        Scenario {
            id: "backup_projects",
            title: "Back up programming projects",
            description: "Copy your code projects to a backup location. Never deletes.",
            note: "Tip: in Advanced mode you can exclude node_modules, target/, etc.",
            tool: Rsync,
            op: Copy,
            delete: false,
            recommend_dry_run: false,
            source_hint: "~/Projects",
            dest_hint: "/mnt/backup/Projects",
            excludes: &["node_modules", "target", ".venv", "__pycache__"],
        },
        Scenario {
            id: "backup_personal",
            title: "Back up a personal folder",
            description: "Copy a folder such as Documents to a backup location.",
            note: "",
            tool: Rsync,
            op: Copy,
            delete: false,
            recommend_dry_run: false,
            source_hint: "~/Documents",
            dest_hint: "/mnt/backup/Documents",
            excludes: &[],
        },
        Scenario {
            id: "backup_to_gdrive",
            title: "Back up to Google Drive",
            description: "Copy a local folder to a Google Drive remote. Never deletes.",
            note: "Requires a configured rclone remote (e.g. gdrive:).",
            tool: Rclone,
            op: Copy,
            delete: false,
            recommend_dry_run: false,
            source_hint: "~/Documents",
            dest_hint: "gdrive:Backup",
            excludes: &[],
        },
        Scenario {
            id: "backup_to_vps_ssh",
            title: "Back up to a VPS over SSH",
            description: "Copy a local folder to a server via rsync over SSH.",
            note: "Set the destination as user@host:/path. SSH keys must be set up.",
            tool: Rsync,
            op: Copy,
            delete: false,
            recommend_dry_run: false,
            source_hint: "~/Documents",
            dest_hint: "user@server:/home/user/backup",
            excludes: &[],
        },
        Scenario {
            id: "sync_two_local",
            title: "Sync two local folders (mirror)",
            description: "Make the destination identical to the source.",
            note: "DESTRUCTIVE: files only in the destination will be deleted. Dry-run first!",
            tool: Rsync,
            op: Sync,
            delete: true,
            recommend_dry_run: true,
            source_hint: "~/folderA",
            dest_hint: "~/folderB",
            excludes: &[],
        },
        Scenario {
            id: "mirror_local_to_remote",
            title: "Mirror a folder to a remote",
            description: "Make a cloud remote an exact mirror of a local folder.",
            note: "DESTRUCTIVE: extra files on the remote will be deleted. Dry-run first!",
            tool: Rclone,
            op: Sync,
            delete: true,
            recommend_dry_run: true,
            source_hint: "~/Documents",
            dest_hint: "gdrive:Mirror",
            excludes: &[],
        },
        Scenario {
            id: "restore_from_backup",
            title: "Restore from a backup",
            description: "Copy files back from a backup location to their original folder.",
            note: "Always dry-run first to confirm what will be written.",
            tool: Rsync,
            op: Copy,
            delete: false,
            recommend_dry_run: true,
            source_hint: "/mnt/backup/Documents",
            dest_hint: "~/Documents",
            excludes: &[],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_scenarios_with_unique_ids() {
        let scenarios = builtin_scenarios();
        assert!(scenarios.len() >= 6);
        let mut ids: Vec<&str> = scenarios.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), scenarios.len(), "scenario ids must be unique");
    }

    #[test]
    fn copy_scenarios_are_not_destructive() {
        let photos = builtin_scenarios()
            .into_iter()
            .find(|s| s.id == "backup_photos")
            .unwrap();
        assert_eq!(photos.risk(), RiskLevel::Caution);
        assert!(!photos.recommend_dry_run);
    }

    #[test]
    fn mirror_scenarios_are_destructive_and_recommend_dry_run() {
        for id in ["sync_two_local", "mirror_local_to_remote"] {
            let sc = builtin_scenarios()
                .into_iter()
                .find(|s| s.id == id)
                .unwrap();
            assert_eq!(
                sc.risk(),
                RiskLevel::Destructive,
                "{id} should be destructive"
            );
            assert!(sc.recommend_dry_run, "{id} should recommend a dry-run");
        }
    }

    #[test]
    fn to_spec_carries_tool_op_and_paths() {
        let sc = builtin_scenarios()
            .into_iter()
            .find(|s| s.id == "backup_to_gdrive")
            .unwrap();
        let spec = sc.to_spec("/home/u/Documents", "gdrive:Backup");
        assert_eq!(spec.tool, Tool::Rclone);
        assert_eq!(spec.op, OpKind::Copy);
        assert_eq!(spec.source, "/home/u/Documents");
        assert_eq!(spec.destination, "gdrive:Backup");
        assert!(!spec.dry_run);
    }
}
