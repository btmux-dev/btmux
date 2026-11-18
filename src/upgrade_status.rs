//! Release comparison and in-place upgrade progress, shared with the desktop
//! launcher, which waits on upgrades it starts as `btmux upgrade` does.

/// `x.y.z` from a version or release tag, ignoring a `v` prefix and any suffix.
pub fn parse_version(tag: &str) -> Option<Vec<u64>> {
    let core = tag.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    core.split('.').map(|part| part.parse().ok()).collect()
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

/// How an upgrade stands, from `/api/info` before it and now.
#[cfg_attr(windows, allow(dead_code))]
pub enum Progress {
    Pending,
    Upgraded,
    /// A service manager restarted the server, losing its shells.
    Restarted,
    Failed(String),
}

#[cfg_attr(windows, allow(dead_code))]
pub fn progress(before: &serde_json::Value, now: &serde_json::Value) -> Progress {
    if now["instance"] != before["instance"] {
        // An exec keeps the pid; a service manager's restart doesn't.
        if now["pid"] != before["pid"] {
            Progress::Restarted
        } else {
            Progress::Upgraded
        }
    } else if let Some(error) = now["upgrade_error"].as_str() {
        Progress::Failed(error.to_string())
    } else {
        Progress::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compares_release_versions() {
        assert!(is_newer("v0.0.124", "0.0.123"));
        assert!(is_newer("0.1.0", "0.0.123"));
        assert!(is_newer("v0.0.1000", "0.0.999"));
        assert!(!is_newer("v0.0.123", "0.0.123"));
        assert!(!is_newer("v0.0.9", "0.0.123"));
        assert!(!is_newer("nightly", "0.0.123"));
        assert!(!is_newer("v1.x", "0.0.123"));
        assert_eq!(parse_version("v1.2.3-rc1"), Some(vec![1, 2, 3]));
    }

    #[test]
    fn reads_upgrade_progress() {
        let before = json!({ "instance": "a", "pid": 1, "upgrade_error": null });
        let same = json!({ "instance": "a", "pid": 1, "upgrade_error": null });
        assert!(matches!(progress(&before, &same), Progress::Pending));
        let execed = json!({ "instance": "b", "pid": 1 });
        assert!(matches!(progress(&before, &execed), Progress::Upgraded));
        let restarted = json!({ "instance": "b", "pid": 2 });
        assert!(matches!(progress(&before, &restarted), Progress::Restarted));
        let failed = json!({ "instance": "a", "pid": 1, "upgrade_error": "no exec" });
        assert!(matches!(progress(&before, &failed), Progress::Failed(e) if e == "no exec"));
    }
}
