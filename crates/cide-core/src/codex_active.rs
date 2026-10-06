//! Codex 0.160 retains writers for previously visited threads and fires model hooks lazily.
//! Identify a resume from a settings-application record in a rollout owned by this PTY process,
//! never from the newest file in the user's global sessions directory. This also separates two
//! consoles in the same directory. Ambiguous or inaccessible observations produce no switch.
use std::path::{Path, PathBuf};

pub fn thread_of(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?.strip_suffix(".jsonl")?;
    let id = name.get(name.len().checked_sub(36)?..)?;
    (name.starts_with("rollout-") && id.parse::<uuid::Uuid>().is_ok()).then(|| id.to_owned())
}

/// Pick only an unambiguous newest settings activation, not mtime (model output grows files).
pub fn latest(paths: impl IntoIterator<Item = (PathBuf, i64)>) -> Option<(PathBuf, i64)> {
    let mut latest: Option<(PathBuf, i64)> = None;
    let mut ambiguous = false;
    for (path, stamp) in paths {
        match &latest {
            Some((previous, time)) if *time == stamp && *previous != path => ambiguous = true,
            Some((_, time)) if *time >= stamp => {}
            _ => {
                latest = Some((path, stamp));
                ambiguous = false;
            }
        }
    }
    (!ambiguous).then_some(latest).flatten()
}

#[cfg(target_os = "linux")]
pub fn owned_rollouts(pid: u32) -> Vec<PathBuf> {
    let mut pids = vec![pid];
    let mut paths = Vec::new();
    // Include launcher children, bounded to avoid following an arbitrary tool process tree.
    for index in 0..32 {
        let Some(pid) = pids.get(index).copied() else {
            break;
        };
        if let Ok(children) = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")) {
            for child in children
                .split_whitespace()
                .filter_map(|p| p.parse::<u32>().ok())
            {
                if pids.len() < 32 && !pids.contains(&child) {
                    pids.push(child);
                }
            }
        }
        for entry in std::fs::read_dir(format!("/proc/{pid}/fd"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let Ok(path) = std::fs::read_link(entry.path()) else {
                continue;
            };
            if thread_of(&path).is_none() {
                continue;
            }
            let flags = std::fs::read_to_string(format!(
                "/proc/{pid}/fdinfo/{}",
                entry.file_name().to_string_lossy()
            ))
            .ok()
            .and_then(|info| {
                info.lines()
                    .find_map(|line| line.strip_prefix("flags:\t").map(str::to_owned))
            })
            .and_then(|flags| u32::from_str_radix(flags.trim(), 8).ok());
            // A tool reading a transcript is not its owner. Require a writable rollout handle.
            if flags.is_some_and(|flags| flags & 3 != 0) && !paths.contains(&path) {
                paths.push(path);
            }
        }
        // Once the console owns a rollout, its tool descendants cannot name the console.
        if !paths.is_empty() {
            return paths;
        }
    }
    paths
}

#[cfg(target_os = "macos")]
pub fn owned_rollouts(pid: u32) -> Vec<PathBuf> {
    let table = crate::process_tree::snapshot();
    let pids = std::iter::once(pid)
        .chain(
            crate::process_tree::descendants(pid, &table)
                .into_iter()
                .take(31)
                .map(|p| p.pid),
        )
        .collect::<Vec<_>>();
    let pid_argument = pids
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let mut command = std::process::Command::new("/usr/sbin/lsof");
    command.args(["-a", "-p", &pid_argument, "-Fpfan"]);
    let Ok(output) = crate::child_env::run_filter_with(
        command,
        None,
        std::time::Duration::from_millis(300),
        &[],
    ) else {
        return Vec::new();
    };
    let mut writable = false;
    let mut owner = None;
    let mut paths: std::collections::HashMap<u32, Vec<PathBuf>> = Default::default();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(pid) = line.strip_prefix('p') {
            owner = pid.parse::<u32>().ok();
            writable = false;
        }
        if line.starts_with('f') {
            writable = false;
        }
        if let Some(access) = line.strip_prefix('a') {
            writable = matches!(access, "w" | "u");
        }
        if let Some(name) = line.strip_prefix('n') {
            let path = PathBuf::from(name);
            if writable
                && thread_of(&path).is_some()
                && let Some(owner) = owner
            {
                let owned = paths.entry(owner).or_default();
                if !owned.contains(&path) {
                    owned.push(path);
                }
            }
        }
    }
    // Prefer the console or its launcher child over any tool that starts another Codex.
    pids.into_iter()
        .find_map(|pid| paths.remove(&pid))
        .unwrap_or_default()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn owned_rollouts(_pid: u32) -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activation_needs_a_real_id_and_an_unambiguous_latest_record() {
        let a = PathBuf::from(format!("rollout-date-{}.jsonl", uuid::Uuid::new_v4()));
        let b = PathBuf::from(format!("rollout-date-{}.jsonl", uuid::Uuid::new_v4()));
        assert!(thread_of(&a).is_some());
        assert!(thread_of(Path::new("rollout-invalid.jsonl")).is_none());
        assert_eq!(
            latest([(a.clone(), 1), (b.clone(), 2)]),
            Some((b.clone(), 2))
        );
        assert_eq!(latest([(a.clone(), 2), (b.clone(), 2)]), None);
        assert_eq!(
            latest([(a.clone(), 3), (b.clone(), 2)]),
            Some((a.clone(), 3))
        );
        assert_eq!(
            latest([(a.clone(), 1), (b.clone(), 1), (a.clone(), 2)]),
            Some((a, 2))
        );
    }
}
