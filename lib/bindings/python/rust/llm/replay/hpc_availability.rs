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
