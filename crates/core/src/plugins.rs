//! Vendor plugin approval integrity.
//!
//! A plugin approval is only meaningful for the exact code it reviewed. This
//! module provides the primitives that bind an approval to a plugin's code and
//! fetch allowlist:
//!
//! - [`plugin_code_fingerprint`]: canonical hash of a plugin's files plus its
//!   `allowed_hosts` declaration.
//! - [`reconcile_plugin_approval`]: given the stored row and newly arrived
//!   code, decide whether the stored approval still applies or the plugin must
//!   go back to `pending` for re-approval.
//! - [`plugin_run_allowed`]: the run gate used by every execution path.

use crate::types::{VendorPluginInstall, VendorPluginRow};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Canonical fingerprint (hex SHA-256) of a plugin's code plus its fetch
/// allowlist declaration.
///
/// The files map is ordered canonically, so JSON key order never changes the
/// hash. The allowlist contributes as one of three distinct states —
/// not-computed (`None`), declared-none (JSON `null`, i.e. unrestricted), or a
/// declared list — so silently dropping `allowed_hosts` changes the fingerprint
/// and forces re-approval.
///
/// Returns `None` when the inputs cannot be parsed; callers treat that as a
/// change (fail closed).
pub fn plugin_code_fingerprint(
    files_json: &str,
    allowed_hosts_json: &Option<String>,
) -> Option<String> {
    let files: BTreeMap<String, String> = serde_json::from_str(files_json).ok()?;

    // Distinct allowlist states, so restricted <-> unrestricted flips the hash.
    let hosts_state: String = match allowed_hosts_json {
        None => "unknown".to_string(),
        Some(json) => {
            let hosts: Option<Vec<String>> = serde_json::from_str(json).ok()?;
            match hosts {
                None => "unrestricted".to_string(),
                Some(mut list) => {
                    list.sort();
                    list.dedup();
                    format!("restricted:{}", list.join(","))
                }
            }
        }
    };

    let mut hasher = Sha256::new();
    hasher.update(b"nisaba-plugin-fingerprint/v1\0");
    for (name, content) in &files {
        hasher.update(b"file\0");
        hasher.update(name.as_bytes());
        hasher.update(b"\0");
        hasher.update(content.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(b"allowed_hosts\0");
    hasher.update(hosts_state.as_bytes());
    Some(format!("{:x}", hasher.finalize()))
}

/// Decide the review state for a plugin whose code is being replaced —
/// via re-import, re-submission, or a P2P merge.
///
/// Returns `(status, approved_by, code_hash)`:
/// - code and allowlist unchanged since the stored row: the stored status and
///   approver are preserved verbatim (an approval survives re-imports of the
///   identical bytes);
/// - anything changed, or the fingerprint cannot be computed: the plugin goes
///   back to `pending`, the approver is cleared, and `code_hash` is set to the
///   new fingerprint so the review UI can show exactly what the admin is being
///   asked to approve.
///
/// The stored `code_hash` (bound at the last review) is preferred over
/// re-hashing the stored files; legacy rows that predate the column fall back
/// to hashing what is stored.
pub fn reconcile_plugin_approval(
    existing: &VendorPluginRow,
    new_files_json: &str,
    new_allowed_hosts_json: &Option<String>,
) -> (String, Option<String>, Option<String>) {
    let new_fp = plugin_code_fingerprint(new_files_json, new_allowed_hosts_json);
    let old_fp = existing
        .code_hash
        .clone()
        .or_else(|| plugin_code_fingerprint(&existing.files_json, &existing.allowed_hosts_json));

    let unchanged = match (&old_fp, &new_fp) {
        (Some(old), Some(new)) => old == new,
        // Fail closed: an uncomputable fingerprint is treated as a change.
        _ => false,
    };

    if unchanged {
        (
            existing.status.clone(),
            existing.approved_by.clone(),
            existing.code_hash.clone(),
        )
    } else {
        ("pending".to_string(), None, new_fp)
    }
}

/// Whether the plugin may execute on this machine: installed and enabled
/// locally, and carrying a current approval. An approval is bound to the code
/// hash it reviewed, so a code change that resets the plugin to `pending`
/// revokes its run eligibility until an admin re-approves.
pub fn plugin_run_allowed(plugin: &VendorPluginRow, install: Option<&VendorPluginInstall>) -> bool {
    let installed = install.map(|i| i.installed && i.enabled).unwrap_or(false);
    installed && plugin.status == "approved"
}

/// Whether a plugin is due for an auto-sync run (N-7.5).
///
/// A plugin with no per-plugin interval (`None`) follows the global sync
/// cycle and is always due. With an interval, it is due when it has never
/// run, or when at least `interval_minutes` have elapsed since its last
/// recorded run started. Timestamps are `"%Y-%m-%d %H:%M:%S"` UTC strings;
/// an unparseable timestamp counts as due (fail open for freshness, never
/// wedge a plugin out of sync forever). Manual runs never consult this.
pub fn plugin_sync_due(
    interval_minutes: Option<i64>,
    last_run_started_at: Option<&str>,
    now: &str,
) -> bool {
    let Some(interval) = interval_minutes else {
        return true;
    };
    if interval <= 0 {
        return true;
    }
    let Some(last) = last_run_started_at else {
        return true;
    };
    let fmt = "%Y-%m-%d %H:%M:%S";
    let (Ok(last), Ok(now)) = (
        chrono::NaiveDateTime::parse_from_str(last, fmt),
        chrono::NaiveDateTime::parse_from_str(now, fmt),
    ) else {
        return true;
    };
    (now - last).num_minutes() >= interval
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files_json(entries: &[(&str, &str)]) -> String {
        let map: BTreeMap<&str, &str> = entries.iter().copied().collect();
        serde_json::to_string(&map).unwrap()
    }

    fn approved_row(files_json: &str, allowed_hosts_json: Option<&str>) -> VendorPluginRow {
        VendorPluginRow {
            id: "p1".to_string(),
            plugin_file: "plugin.zip".to_string(),
            display_name: "P".to_string(),
            description: String::new(),
            files_json: files_json.to_string(),
            version: "1.0.0".to_string(),
            config_fields: None,
            config_json: None,
            status: "approved".to_string(),
            submitted_by: None,
            approved_by: Some("onion-abc".to_string()),
            category: "vendor".to_string(),
            icon: None,
            include_vendor_stock: false,
            created_at: "2026-10-20 00:00:00".to_string(),
            updated_at: "2026-10-20 00:00:00".to_string(),
            allowed_hosts_json: allowed_hosts_json.map(str::to_string),
            code_hash: None,
        }
    }

    #[test]
    fn fingerprint_is_stable_regardless_of_file_order() {
        let a = plugin_code_fingerprint(
            &files_json(&[("index.ts", "code"), ("lib/a.ts", "x")]),
            &Some(r#"["b.example.com", "a.example.com"]"#.to_string()),
        );
        let b = plugin_code_fingerprint(
            &files_json(&[("lib/a.ts", "x"), ("index.ts", "code")]),
            &Some(r#"["a.example.com", "b.example.com"]"#.to_string()),
        );
        assert_eq!(a, b);
    }

    #[test]
    fn fingerprint_distinguishes_code_and_allowlist_changes() {
        let code = files_json(&[("index.ts", "code")]);
        let hosts = Some(r#"["api.example.com"]"#.to_string());
        let base = plugin_code_fingerprint(&code, &hosts).unwrap();

        // Changed code.
        assert_ne!(
            plugin_code_fingerprint(&files_json(&[("index.ts", "code2")]), &hosts).unwrap(),
            base
        );
        // Added file.
        assert_ne!(
            plugin_code_fingerprint(
                &files_json(&[("index.ts", "code"), ("extra.ts", "y")]),
                &hosts
            )
            .unwrap(),
            base
        );
        // Changed allowlist.
        assert_ne!(
            plugin_code_fingerprint(&code, &Some(r#"["other.example.com"]"#.to_string())).unwrap(),
            base
        );
        // Silently dropped allowlist (restricted -> unrestricted) must flip the hash.
        assert_ne!(
            plugin_code_fingerprint(&code, &Some("null".to_string())).unwrap(),
            base
        );
        // Unknown (never computed) is a third distinct state.
        assert_ne!(plugin_code_fingerprint(&code, &None).unwrap(), base);
        // Unparseable files fail closed.
        assert!(plugin_code_fingerprint("not json", &hosts).is_none());
    }

    #[test]
    fn reimport_with_unchanged_code_preserves_status() {
        let code = files_json(&[("index.ts", "code")]);
        let hosts = Some(r#"["api.example.com"]"#);
        let existing = approved_row(&code, hosts);
        // Same bytes, JSON written in a different key order.
        let reordered = files_json(&[("index.ts", "code")]);
        let (status, approved_by, code_hash) =
            reconcile_plugin_approval(&existing, &reordered, &hosts.map(str::to_string));
        assert_eq!(status, "approved");
        assert_eq!(approved_by.as_deref(), Some("onion-abc"));
        assert_eq!(code_hash, None);
    }

    #[test]
    fn reimport_with_changed_code_resets_to_pending() {
        let existing = approved_row(
            &files_json(&[("index.ts", "code")]),
            Some(r#"["api.example.com"]"#),
        );
        let (status, approved_by, code_hash) = reconcile_plugin_approval(
            &existing,
            &files_json(&[("index.ts", "code // changed")]),
            &Some(r#"["api.example.com"]"#.to_string()),
        );
        assert_eq!(status, "pending");
        assert_eq!(approved_by, None);
        // The new fingerprint is surfaced so the reviewer sees what changed.
        assert!(code_hash.is_some());
    }

    #[test]
    fn reimport_that_drops_the_allowlist_resets_to_pending() {
        let existing = approved_row(
            &files_json(&[("index.ts", "code")]),
            Some(r#"["api.example.com"]"#),
        );
        let (status, _, _) = reconcile_plugin_approval(
            &existing,
            &files_json(&[("index.ts", "code")]),
            &Some("null".to_string()),
        );
        assert_eq!(status, "pending");
    }

    #[test]
    fn reimport_of_a_pending_plugin_stays_pending() {
        let mut existing = approved_row(&files_json(&[("index.ts", "code")]), None);
        existing.status = "pending".to_string();
        existing.approved_by = None;
        let (status, approved_by, _) =
            reconcile_plugin_approval(&existing, &files_json(&[("index.ts", "different")]), &None);
        assert_eq!(status, "pending");
        assert_eq!(approved_by, None);
    }

    #[test]
    fn stored_code_hash_is_preferred_over_rehashing_stored_files() {
        let mut existing = approved_row(&files_json(&[("index.ts", "code")]), None);
        existing.code_hash = Some("bound-at-approval".to_string());
        // The stored files happen to hash differently (e.g. files were touched by a
        // migration); the bound hash is the source of truth, so this is a change.
        let (status, _, _) =
            reconcile_plugin_approval(&existing, &files_json(&[("index.ts", "code")]), &None);
        assert_eq!(status, "pending");
        // ...but the identical bound hash preserves the approval.
        let mut same = approved_row(&files_json(&[("index.ts", "other")]), None);
        same.code_hash = plugin_code_fingerprint(&files_json(&[("index.ts", "other")]), &None);
        let (status, approved_by, _) =
            reconcile_plugin_approval(&same, &files_json(&[("index.ts", "other")]), &None);
        assert_eq!(status, "approved");
        assert_eq!(approved_by.as_deref(), Some("onion-abc"));
    }

    fn install(installed: bool, enabled: bool) -> VendorPluginInstall {
        VendorPluginInstall {
            plugin_id: "p1".to_string(),
            installed,
            enabled,
            installed_at: "2026-10-20 00:00:00".to_string(),
            timeout_minutes: None,
            sync_interval_minutes: None,
        }
    }

    #[test]
    fn run_gate_requires_installed_enabled_and_approved() {
        let plugin = approved_row(&files_json(&[("index.ts", "x")]), None);
        let on = install(true, true);
        assert!(plugin_run_allowed(&plugin, Some(&on)));
        assert!(!plugin_run_allowed(&plugin, Some(&install(true, false))));
        assert!(!plugin_run_allowed(&plugin, Some(&install(false, true))));
        assert!(!plugin_run_allowed(&plugin, None));

        let mut pending = plugin.clone();
        pending.status = "pending".to_string();
        assert!(!plugin_run_allowed(&pending, Some(&on)));

        let mut rejected = plugin.clone();
        rejected.status = "rejected".to_string();
        assert!(!plugin_run_allowed(&rejected, Some(&on)));
    }

    #[test]
    fn sync_due_without_interval_is_always_due() {
        assert!(plugin_sync_due(None, None, "2026-10-21 10:00:00"));
        assert!(plugin_sync_due(
            None,
            Some("2026-10-21 09:59:00"),
            "2026-10-21 10:00:00"
        ));
        // A non-positive interval is treated as "no interval".
        assert!(plugin_sync_due(
            Some(0),
            Some("2026-10-21 09:59:00"),
            "2026-10-21 10:00:00"
        ));
    }

    #[test]
    fn sync_due_with_interval_waits_for_it_to_elapse() {
        let now = "2026-10-21 10:00:00";
        // Never run: due immediately.
        assert!(plugin_sync_due(Some(60), None, now));
        // Ran 30 minutes ago with a 60-minute interval: not due.
        assert!(!plugin_sync_due(Some(60), Some("2026-10-21 09:30:00"), now));
        // Exactly at the interval and beyond: due.
        assert!(plugin_sync_due(Some(60), Some("2026-10-21 09:00:00"), now));
        assert!(plugin_sync_due(Some(60), Some("2026-10-21 08:00:00"), now));
        // Unparseable history fails open.
        assert!(plugin_sync_due(Some(60), Some("not a date"), now));
    }
}
