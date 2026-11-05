// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Strict JSONL loader for the MalleServe worker availability trace.
//! External worker IDs are stable across reclaim/re-availability; the native
//! replay engine allocates fresh monotonically increasing IDs for each add.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use dynamo_mocker::replay::{
    WorkerAvailabilityAction, WorkerAvailabilityEvent, WorkerRecoveryMode, WorkerRecoveryPolicy,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HpcConfig {
    pub availability_trace: String,
    pub reclamation: String,
    pub recovery: String,
    pub max_restarts: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AvailabilityRow {
    timestamp_ms: f64,
    event: String,
    workers: Vec<usize>,
}

pub(super) fn load(
    config: HpcConfig,
    initial_workers: usize,
) -> Result<(Vec<WorkerAvailabilityEvent>, WorkerRecoveryPolicy)> {
    let recovery = match config.recovery.as_str() {
        "none" => WorkerRecoveryMode::None,
        "full_request_restart" => WorkerRecoveryMode::Restart,
        other => bail!("unsupported hpc.recovery '{other}'"),
    };
    let ignore_reclamation = match config.reclamation.as_str() {
        "ignored" => true,
        "immediate" => false,
        other => bail!("unsupported hpc.reclamation '{other}'"),
    };
    ensure!(
        !ignore_reclamation || recovery == WorkerRecoveryMode::None,
        "recovery requires immediate reclamation"
    );
    ensure!(
        recovery != WorkerRecoveryMode::Restart || config.max_restarts > 0,
        "full-request restart requires positive max_restarts"
    );
    let path = Path::new(&config.availability_trace);
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read HPC availability trace {}", path.display()))?;
    let mut previous_ms = 0.0_f64;
    let mut source_active = BTreeSet::<usize>::new();
    let mut sim_active = BTreeMap::<usize, usize>::new();
    let mut next_native_id = initial_workers;
    let mut events = Vec::new();
    for (line_index, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let number = line_index + 1;
        let row: AvailabilityRow = serde_json::from_str(line).with_context(|| {
            format!("{}:{number}: invalid availability JSONL", path.display())
        })?;
        ensure!(
            row.timestamp_ms.is_finite() && row.timestamp_ms >= previous_ms,
            "{}:{number}: timestamps must be finite, nonnegative and nondecreasing",
            path.display()
        );
        previous_ms = row.timestamp_ms;
        ensure!(
            !row.workers.is_empty() && row.workers.iter().copied().collect::<BTreeSet<_>>().len() == row.workers.len(),
            "{}:{number}: workers must be a nonempty list of unique IDs",
            path.display()
        );
        ensure!(
            row.workers.iter().all(|id| *id >= initial_workers),
            "{}:{number}: opportunistic worker IDs must be >= initial stable worker count ({initial_workers})",
            path.display()
        );
        match row.event.as_str() {
            "available" => {
                let mut added = 0;
                for worker in row.workers {
                    ensure!(source_active.insert(worker),
                        "{}:{number}: worker {worker} is already available", path.display());
                    if !sim_active.contains_key(&worker) {
                        let native = next_native_id;
                        next_native_id = next_native_id.checked_add(1).context("worker ID overflow")?;
                        sim_active.insert(worker, native);
                        added += 1;
                    }
                }
                if added > 0 {
                    events.push(WorkerAvailabilityEvent {
                        time_ms: row.timestamp_ms,
                        change: WorkerAvailabilityAction::Add { count: added },
                    });
                }
            }
            "reclaimed" => {
                let mut reclaimed = Vec::new();
                for worker in row.workers {
                    ensure!(source_active.remove(&worker),
                        "{}:{number}: worker {worker} was not available", path.display());
                    if !ignore_reclamation {
                        reclaimed.push(
                            sim_active.remove(&worker).expect("available worker is mapped")
                        );
                    }
                }
                if !reclaimed.is_empty() {
                    events.push(WorkerAvailabilityEvent {
                        time_ms: row.timestamp_ms,
                        change: WorkerAvailabilityAction::Reclaim { worker_ids: reclaimed },
                    });
                }
            }
            other => bail!("{}:{number}: unknown event '{other}'; expected available or reclaimed", path.display()),
        }
    }
    Ok((
        events,
        WorkerRecoveryPolicy { mode: recovery, max_restarts: config.max_restarts },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_reavailable_external_worker_to_fresh_native_id() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!(
            "malleserve-hpc-{}-{unique}.jsonl", std::process::id()
        ));
        std::fs::write(&path, concat!(
            "{\"timestamp_ms\":10,\"event\":\"available\",\"workers\":[1]}\n",
            "{\"timestamp_ms\":20,\"event\":\"reclaimed\",\"workers\":[1]}\n",
            "{\"timestamp_ms\":30,\"event\":\"available\",\"workers\":[1]}\n",
            "{\"timestamp_ms\":40,\"event\":\"reclaimed\",\"workers\":[1]}\n"
        )).unwrap();
        let config = HpcConfig {
            availability_trace: path.display().to_string(),
            reclamation: "immediate".to_owned(),
            recovery: "full_request_restart".to_owned(),
            max_restarts: 2,
        };
        let result = load(config, 1);
        std::fs::remove_file(path).unwrap();
        let (events, policy) = result.unwrap();
        assert_eq!(policy.mode, WorkerRecoveryMode::Restart);
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0].change, WorkerAvailabilityAction::Add { count: 1 }));
        assert!(matches!(events[1].change, WorkerAvailabilityAction::Reclaim { ref worker_ids } if worker_ids == &[1]));
        assert!(matches!(events[2].change, WorkerAvailabilityAction::Add { count: 1 }));
        assert!(matches!(events[3].change, WorkerAvailabilityAction::Reclaim { ref worker_ids } if worker_ids == &[2]));
    }

    #[test]
    fn ignores_reclamation_but_does_not_duplicate_retained_worker() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!(
            "malleserve-hpc-retain-{}-{unique}.jsonl", std::process::id()
        ));
        std::fs::write(&path, concat!(
            "{\"timestamp_ms\":10,\"event\":\"available\",\"workers\":[1]}\n",
            "{\"timestamp_ms\":20,\"event\":\"reclaimed\",\"workers\":[1]}\n",
            "{\"timestamp_ms\":30,\"event\":\"available\",\"workers\":[1]}\n"
        )).unwrap();
        let result = load(HpcConfig {
            availability_trace: path.display().to_string(),
            reclamation: "ignored".to_owned(),
            recovery: "none".to_owned(),
            max_restarts: 0,
        }, 1);
        std::fs::remove_file(path).unwrap();
        let (events, _) = result.unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].change, WorkerAvailabilityAction::Add { count: 1 }));
    }

    #[test]
    fn rejects_unknown_reclamation() {
        let cfg = HpcConfig {
            availability_trace: "not-needed.jsonl".to_owned(),
            reclamation: "invalid".to_owned(),
            recovery: "none".to_owned(),
            max_restarts: 0,
        };
        assert!(load(cfg, 1).is_err());
    }
}
