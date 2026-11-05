# Trace-driven HPC capacity experiments (research prototype)

This extension provides an aggregated-inference replay entry point for transient
HPC workers, with their existing startup delay and abrupt, unannounced worker
reclamation. It is intended to compare four controlled experiments rather than
simulate the HPC batch scheduler.

## Rust API

Use `dynamo_mocker::replay::simulate_trace_requests_with_hpc_events` with
`WorkerAvailabilityEvent`, `WorkerAvailabilityAction` and
`WorkerRecoveryPolicy`. Supply the existing `MockerConfig`, original
`Vec<DirectRequest>`, optional KV-router config and optional prefill load
estimator as you would to `simulate_trace_requests_with_router_mode`.

```rust
use dynamo_mocker::replay::{
    ReplayRouterMode, SlaThresholds,
    WorkerAvailabilityAction, WorkerAvailabilityEvent,
    WorkerRecoveryMode, WorkerRecoveryPolicy,
    simulate_trace_requests_with_hpc_events,
};

// Assume args and requests are your existing MockerConfig and Vec<DirectRequest>.
// args.startup_time is in seconds, e.g. Some(30.0).
let events = vec![
    WorkerAvailabilityEvent {
        time_ms: 100_000.0,
        change: WorkerAvailabilityAction::Add { count: 4 },
    },
    WorkerAvailabilityEvent {
        time_ms: 200_000.0,
        change: WorkerAvailabilityAction::Reclaim {
            worker_ids: vec![4, 5, 6, 7],
        },
    },
];
let report = simulate_trace_requests_with_hpc_events(
    args, None, None, requests,
    4, 1.0, ReplayRouterMode::KvRouter,
    events,
    WorkerRecoveryPolicy {
        mode: WorkerRecoveryMode::Restart,
        max_restarts: 1,
    },
    SlaThresholds::default(),
)?;
println!("{:?}", report.worker_availability);
```

## Experiment matrix

| Experiment | Input | Recovery |
| --- | --- | --- |
| E1 stable-only | No availability events; 4 initial workers | None |
| E2 added capacity, no reclamation | Add events only | None |
| E3 immediate reclaim | Add + Reclaim | None |
| E4 full-request restart | Same Add + Reclaim trace as E3 | Restart |

Keep request traces, random seeds, engine settings and routing mode identical
across comparisons. E3 and E4 should differ only in the recovery policy.

Worker IDs are monotonic: the initial four workers are 0–3, so the next Add
of four workers allocates IDs 4–7. Reclamation is atomic at its timestamp,
before completion and routing work at the same instant.

When recovery is disabled, unfinished worker-owned requests receive a
`Failed` terminal status. In Restart mode, the original prompt and full
generation budget are resubmitted using the *same logical request UUID*.
Partial output from failed attempts is discarded. End-to-end latency retains
the original arrival time; a request still counts once in the report.
The retry limit is per logical request, and an attempt without any surviving
ready worker becomes permanently failed.

The optional `report.worker_availability` object counts added/reclaimed
workers, reclamation events, interrupted attempts, restarted attempts and
permanent failures. The normal request counts, output throughput and per-request
status can be used alongside these metrics.

## Implementation boundaries

- Aggregated inference only. Disaggregated KV handoffs are not implemented.
- External availability events are not combined with demand-driven Planner
  scaling; worker additions reuse the existing startup timer.
- Reclamation has no warning and no failure detection delay.
- A restarted request discards any partially emitted response; this is not the
  production Dynamo token-preserving migration protocol.
- Original request payloads are retained for eligible restarts, which adds memory
  overhead when Restart mode is enabled.
- The request model assumes a compatible surviving worker is immediately
  available. There is no separate wait-for-capacity recovery queue.
- GPU work wasted by an *in-progress* forward pass is reflected in provisioned
  worker-time but is not yet a separately priced abandoned-pass counter.

## Build and compatibility

This research branch depends on `pw-02/aisimulate` via the renamed Git
dependency `aisim-core`, whose Rust library name is `aisimulate_core`.
The fork's core Cargo package is v0.14.0; this differs from the previously
published, pinned v0.13 development crate. Resolve and validate the new lockfile
before a reproducible build. The research implementation has not been verified
against a full integrated cargo test run in this environment.
