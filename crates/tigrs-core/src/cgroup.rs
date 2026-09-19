// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Linux cgroup memory pressure monitoring.
//!
//! Enforces:
//! - Bounded memory inspection via cgroup v2 (`memory.current`, `memory.max`),
//!   cgroup v1 fallback (`memory.usage_in_bytes`, `memory.limit_in_bytes`),
//!   and `/proc/meminfo`.

use std::fs;
use std::path::{Path, PathBuf};

/// Discrete memory pressure categories derived from cgroup limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemoryPressureLevel {
    /// Memory consumption is within safe operational thresholds (< 75% of limit).
    Normal,
    /// Memory consumption is approaching the ceiling (>= 75% of limit).
    High,
    /// Memory consumption is critical (>= 90% of limit), immediate shedding required.
    Critical,
}

/// Snapshot of current memory utilization and configured ceiling in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryUsage {
    /// Currently allocated / resident memory in bytes.
    pub current_bytes: u64,
    /// Configured upper ceiling in bytes, or `None` if unlimited ("max").
    pub limit_bytes: Option<u64>,
}

impl MemoryUsage {
    /// Evaluates the pressure level against 75% (High) and 90% (Critical) thresholds.
    #[must_use]
    pub fn pressure_level(&self) -> MemoryPressureLevel {
        if let Some(limit) = self.limit_bytes
            && limit > 0
        {
            let pct = (u128::from(self.current_bytes) * 100) / u128::from(limit);
            if pct >= 90 {
                return MemoryPressureLevel::Critical;
            }
            if pct >= 75 {
                return MemoryPressureLevel::High;
            }
        }
        MemoryPressureLevel::Normal
    }
}

/// Reads the relative cgroup path for this process from `/proc/self/cgroup`.
#[must_use]
pub fn read_process_cgroup_path() -> Option<PathBuf> {
    let content = fs::read_to_string("/proc/self/cgroup").ok()?;
    parse_cgroup_proc_content(&content)
}

/// Parses the relative cgroup path from `/proc/self/cgroup` content.
pub fn parse_cgroup_proc_content(content: &str) -> Option<PathBuf> {
    for line in content.lines() {
        let parts: Vec<&str> = line.splitn(3, ':').collect();
        if parts.len() == 3 {
            // cgroup v2 format is "0::<path>"
            if parts[0] == "0" && !parts[2].is_empty() {
                let trimmed = parts[2].trim_start_matches('/');
                return Some(PathBuf::from(trimmed));
            }
            // cgroup v1 format with "memory" controller
            if parts[1].split(',').any(|c| c == "memory") && !parts[2].is_empty() {
                let trimmed = parts[2].trim_start_matches('/');
                return Some(PathBuf::from(trimmed));
            }
        }
    }
    None
}

/// Parses a byte count from a cgroup metric file (handles "max" and raw numbers).
pub fn parse_cgroup_bytes(raw: &str) -> Option<Option<u64>> {
    let s = raw.trim();
    if s == "max" || s == "infinity" {
        return Some(None);
    }
    s.parse::<u64>().ok().map(Some)
}

/// Reads memory usage and limits from Linux cgroup filesystem.
#[must_use]
pub fn read_cgroup_memory_usage() -> Option<MemoryUsage> {
    let rel = read_process_cgroup_path();
    read_cgroup_memory_usage_from(
        Path::new("/sys/fs/cgroup"),
        rel.as_deref(),
        Path::new("/proc/meminfo"),
    )
}

/// Parses `inactive_file` bytes from a cgroup v2 `memory.stat` content string.
fn parse_cgroup_inactive_file_bytes(stat_content: &str) -> u64 {
    for line in stat_content.lines() {
        if let Some(rest) = line.strip_prefix("inactive_file ")
            && let Ok(val) = rest.trim().parse::<u64>()
        {
            return val;
        }
    }
    0
}

/// Internal helper allowing path overrides for testing cgroup hierarchies.
pub(crate) fn read_cgroup_memory_usage_from(
    base: &Path,
    rel_path: Option<&Path>,
    meminfo_path: &Path,
) -> Option<MemoryUsage> {
    // 1. Try cgroup v2 under base: walk from leaf up to base to find the closest
    // `memory.current` (adjusted for reclaimable `inactive_file` page cache) and
    // the minimum non-unlimited `memory.max` across all ancestor slices.
    if base.exists() {
        let start_dir = match rel_path {
            Some(rel) => base.join(rel),
            None => base.to_path_buf(),
        };
        let mut current_dir = start_dir;
        let mut leaf_current: Option<u64> = None;
        let mut effective_limit: Option<u64> = None;

        while current_dir.starts_with(base) {
            if leaf_current.is_none() {
                let cur_file = current_dir.join("memory.current");
                if cur_file.exists()
                    && let Ok(cur_str) = fs::read_to_string(&cur_file)
                    && let Some(Some(raw_cur)) = parse_cgroup_bytes(&cur_str)
                {
                    let inactive_file = fs::read_to_string(current_dir.join("memory.stat"))
                        .ok()
                        .map_or(0, |s| parse_cgroup_inactive_file_bytes(&s));
                    leaf_current = Some(raw_cur.saturating_sub(inactive_file));
                }
            }

            let max_file = current_dir.join("memory.max");
            if let Ok(max_str) = fs::read_to_string(&max_file)
                && let Some(Some(limit)) = parse_cgroup_bytes(&max_str)
            {
                effective_limit = Some(match effective_limit {
                    Some(prev) => prev.min(limit),
                    None => limit,
                });
            }

            if current_dir == base {
                break;
            }
            if let Some(parent) = current_dir.parent() {
                current_dir = parent.to_path_buf();
            } else {
                break;
            }
        }

        if let Some(current_bytes) = leaf_current {
            return Some(MemoryUsage {
                current_bytes,
                limit_bytes: effective_limit,
            });
        }
    }

    // 2. Try cgroup v1 under base/memory
    let v1_dir = base.join("memory");
    let v1_cur = v1_dir.join("memory.usage_in_bytes");
    let v1_lim = v1_dir.join("memory.limit_in_bytes");
    if v1_cur.exists()
        && let Ok(cur_str) = fs::read_to_string(v1_cur)
        && let Some(Some(current_bytes)) = parse_cgroup_bytes(&cur_str)
    {
        let limit_bytes = fs::read_to_string(v1_lim)
            .ok()
            .and_then(|s| parse_cgroup_bytes(&s))
            .flatten();

        return Some(MemoryUsage {
            current_bytes,
            limit_bytes,
        });
    }

    // 3. Fallback to /proc/meminfo
    read_proc_meminfo_from(meminfo_path)
}

/// Internal helper for reading memory from arbitrary meminfo formatted files.
pub(crate) fn read_proc_meminfo_from(path: &Path) -> Option<MemoryUsage> {
    let content = fs::read_to_string(path).ok()?;
    let mut total_kb = None;
    let mut avail_kb = None;

    for line in content.lines() {
        if line.starts_with("MemTotal:") {
            total_kb = parse_meminfo_kb(line);
        } else if line.starts_with("MemAvailable:") {
            avail_kb = parse_meminfo_kb(line);
        }
    }

    if let (Some(total), Some(avail)) = (total_kb, avail_kb) {
        let total_bytes = total * 1024;
        let avail_bytes = avail * 1024;
        let current_bytes = total_bytes.saturating_sub(avail_bytes);
        Some(MemoryUsage {
            current_bytes,
            limit_bytes: Some(total_bytes),
        })
    } else {
        None
    }
}

/// Extracts the numeric KB value from a `/proc/meminfo` line.
fn parse_meminfo_kb(line: &str) -> Option<u64> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 2 {
        parts[1].parse::<u64>().ok()
    } else {
        None
    }
}

/// Evaluates current system/cgroup memory pressure.
#[must_use]
pub fn evaluate_memory_pressure() -> MemoryPressureLevel {
    read_cgroup_memory_usage().map_or(MemoryPressureLevel::Normal, |usage| usage.pressure_level())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_usage_pressure_levels() {
        let normal = MemoryUsage {
            current_bytes: 500 * 1024 * 1024,
            limit_bytes: Some(1024 * 1024 * 1024), // 48.8%
        };
        assert_eq!(normal.pressure_level(), MemoryPressureLevel::Normal);

        let high = MemoryUsage {
            current_bytes: 800 * 1024 * 1024,
            limit_bytes: Some(1024 * 1024 * 1024), // 78.1%
        };
        assert_eq!(high.pressure_level(), MemoryPressureLevel::High);

        let critical = MemoryUsage {
            current_bytes: 950 * 1024 * 1024,
            limit_bytes: Some(1024 * 1024 * 1024), // 92.7%
        };
        assert_eq!(critical.pressure_level(), MemoryPressureLevel::Critical);

        let unlimited = MemoryUsage {
            current_bytes: 500 * 1024 * 1024,
            limit_bytes: None,
        };
        assert_eq!(unlimited.pressure_level(), MemoryPressureLevel::Normal);
    }

    #[test]
    fn test_parse_cgroup_proc_content() {
        let v2 = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/test.service\n";
        assert_eq!(
            parse_cgroup_proc_content(v2),
            Some(PathBuf::from(
                "user.slice/user-1000.slice/user@1000.service/app.slice/test.service"
            ))
        );

        let v1 = "2:memory:/docker/1234567890abcdef\n1:cpu:/docker/1234567890abcdef\n";
        assert_eq!(
            parse_cgroup_proc_content(v1),
            Some(PathBuf::from("docker/1234567890abcdef"))
        );
    }

    #[test]
    fn test_parse_cgroup_bytes() {
        assert_eq!(parse_cgroup_bytes("1048576\n"), Some(Some(1_048_576)));
        assert_eq!(parse_cgroup_bytes("max\n"), Some(None));
        assert_eq!(parse_cgroup_bytes("infinity"), Some(None));
        assert_eq!(parse_cgroup_bytes("invalid"), None);
    }

    #[test]
    fn test_read_cgroup_memory_usage_on_host() {
        let usage = read_cgroup_memory_usage();
        #[cfg(target_os = "linux")]
        {
            assert!(
                usage.is_some(),
                "read_cgroup_memory_usage should read cgroup or meminfo on Linux"
            );
            let u = usage.unwrap();
            assert!(u.current_bytes > 0);
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert!(usage.is_none());
            assert_eq!(evaluate_memory_pressure(), MemoryPressureLevel::Normal);
        }
    }

    #[test]
    fn test_parse_meminfo_kb() {
        assert_eq!(
            parse_meminfo_kb("MemTotal:       32850944 kB"),
            Some(32_850_944)
        );
        assert_eq!(
            parse_meminfo_kb("MemAvailable:   21568764 kB"),
            Some(21_568_764)
        );
        assert_eq!(parse_meminfo_kb(""), None);
        assert_eq!(parse_meminfo_kb("MemTotal:"), None);
        assert_eq!(parse_meminfo_kb("MemTotal: invalid kB"), None);
    }

    #[test]
    fn test_memory_usage_zero_limit() {
        let usage_zero_limit = MemoryUsage {
            current_bytes: 50,
            limit_bytes: Some(0),
        };
        assert_eq!(
            usage_zero_limit.pressure_level(),
            MemoryPressureLevel::Normal
        );
    }

    #[test]
    fn test_parse_cgroup_proc_content_edge_cases() {
        assert_eq!(parse_cgroup_proc_content(""), None);
        assert_eq!(parse_cgroup_proc_content("invalid line"), None);
        assert_eq!(parse_cgroup_proc_content("0::\n"), None);
        assert_eq!(parse_cgroup_proc_content("1:cpu:/not-memory\n"), None);
        assert_eq!(parse_cgroup_proc_content("2:memory:\n"), None);

        let multi = "1:cpu:/cpu\n0::/system.slice/tigrs.service\n";
        assert_eq!(
            parse_cgroup_proc_content(multi),
            Some(PathBuf::from("system.slice/tigrs.service"))
        );
    }

    #[test]
    fn test_read_process_cgroup_path() {
        // Runs on host - returns Option<PathBuf>
        let _ = read_process_cgroup_path();
    }

    #[test]
    fn test_read_proc_meminfo_from_mock() {
        let tmp = std::env::temp_dir().join(format!("tigrs_test_meminfo_{}", std::process::id()));
        let _ = fs::remove_file(&tmp);

        // Missing file
        assert!(read_proc_meminfo_from(&tmp).is_none());

        // Incomplete content (missing MemAvailable)
        fs::write(&tmp, "MemTotal:       1000 kB\n").unwrap();
        assert!(read_proc_meminfo_from(&tmp).is_none());

        // Complete content
        fs::write(
            &tmp,
            "MemTotal:       2000 kB\nMemFree:        500 kB\nMemAvailable:   1500 kB\n",
        )
        .unwrap();
        let u = read_proc_meminfo_from(&tmp).unwrap();
        assert_eq!(u.limit_bytes, Some(2000 * 1024));
        assert_eq!(u.current_bytes, (2000 - 1500) * 1024);

        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn test_read_cgroup_memory_usage_from_cgroup_v2() {
        let tmp = std::env::temp_dir().join(format!("tigrs_cg_v2_{}", std::process::id()));
        let sub = tmp.join("user.slice").join("app.slice");
        fs::create_dir_all(&sub).unwrap();

        // Put memory.current and memory.max in sub
        fs::write(sub.join("memory.current"), "52428800\n").unwrap();
        fs::write(sub.join("memory.max"), "104857600\n").unwrap();

        let meminfo = tmp.join("meminfo");
        fs::write(&meminfo, "MemTotal: 1000 kB\nMemAvailable: 500 kB\n").unwrap();

        let u =
            read_cgroup_memory_usage_from(&tmp, Some(Path::new("user.slice/app.slice")), &meminfo)
                .unwrap();
        assert_eq!(u.current_bytes, 52_428_800);
        assert_eq!(u.limit_bytes, Some(104_857_600));

        // Test with max = "max"
        fs::write(sub.join("memory.max"), "max\n").unwrap();
        let u_unlimited =
            read_cgroup_memory_usage_from(&tmp, Some(Path::new("user.slice/app.slice")), &meminfo)
                .unwrap();
        assert_eq!(u_unlimited.current_bytes, 52_428_800);
        assert_eq!(u_unlimited.limit_bytes, None);

        // Test directory traversal: remove files from sub, put in parent
        let _ = fs::remove_file(sub.join("memory.current"));
        let _ = fs::remove_file(sub.join("memory.max"));
        fs::write(tmp.join("user.slice").join("memory.current"), "123456\n").unwrap();
        fs::write(tmp.join("user.slice").join("memory.max"), "789012\n").unwrap();

        let u_parent =
            read_cgroup_memory_usage_from(&tmp, Some(Path::new("user.slice/app.slice")), &meminfo)
                .unwrap();
        assert_eq!(u_parent.current_bytes, 123_456);
        assert_eq!(u_parent.limit_bytes, Some(789_012));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_read_cgroup_memory_usage_from_cgroup_v1() {
        let tmp = std::env::temp_dir().join(format!("tigrs_cg_v1_{}", std::process::id()));
        let mem_dir = tmp.join("memory");
        fs::create_dir_all(&mem_dir).unwrap();

        fs::write(mem_dir.join("memory.usage_in_bytes"), "2048000\n").unwrap();
        fs::write(mem_dir.join("memory.limit_in_bytes"), "4096000\n").unwrap();

        let meminfo = tmp.join("meminfo");
        fs::write(&meminfo, "MemTotal: 1000 kB\nMemAvailable: 500 kB\n").unwrap();

        let u = read_cgroup_memory_usage_from(&tmp, None, &meminfo).unwrap();
        assert_eq!(u.current_bytes, 2_048_000);
        assert_eq!(u.limit_bytes, Some(4_096_000));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_evaluate_memory_pressure_on_host() {
        let level = evaluate_memory_pressure();
        assert!(matches!(
            level,
            MemoryPressureLevel::Normal | MemoryPressureLevel::High | MemoryPressureLevel::Critical
        ));
    }
}
