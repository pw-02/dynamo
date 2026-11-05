# MalleServe HPC availability experiments on AISimulate 0.13

This branch is an **unvalidated research backport**. The AISimulate
source branch was initialized from upstream commit `d9620e0` on
October 28, 2026, immediately before the October 29 nightly window.
This is closer to Dynamo's pinned development release than the published
October 21 release branch, but the source SHA is **not yet proven identical**
to the published October 29 crate. Do not merge or report measurements until both
the native live regression suite and HPC tests pass.

## Repositories

- AISimulate backport: `pw-02/aisimulate` branch `malleserve-aisim-013-nightly-backport`
- Dynamo integration: `pw-02/dynamo` branch `malleserve-dynosim-013-backport`
- Original baseline: Dynamo `main`, pinned `aisimulate-core =0.13.0-dev.202610290000000087`

This branch intentionally keeps the Cargo **package name**
`aisimulate-core`. It does **not** use the incompatible 0.14 package rename
(`aisim-core`), and it does not change Dynamo's production live scheduler.

## E1–E4 experiment matrix

| ID | Initial workers | Availability events | Recovery |
| --- | --- | --- | --- |
| E1 | Stable only | None | None |
| E2 | Stable | Add only | None |
| E3 | Stable | Add + Reclaim | None |
| E4 | Stable | Identical Add + Reclaim as E3 | Restart |

Use the public Rust entry point
`dynamo_mocker::replay::simulate_trace_requests_with_hpc_events`.
For E1/E2, use no events/add-only events and recovery=None.
For E3/E4, use *the same* request trace and availability events, changing
only the recovery policy.

```rust
use dynamo_mocker::replay::{
    ReplayRouterMode, SlaThresholds,
    WorkerAvailabilityAction, WorkerAvailabilityEvent,
    WorkerRecoveryMode, WorkerRecoveryPolicy,
    simulate_trace_requests_with_hpc_events,
};

// args: your existing MockerConfig; requests: Vec<DirectRequest>.
// args.startup_time uses seconds; worker event timestamps use milliseconds.
let events = vec![
    WorkerAvailabilityEvent {
        time_ms: 100_000.0,
        change: WorkerAvailabilityAction::Add { count: 4 },
    },
    WorkerAvailabilityEvent {
        time_ms: 200_000.0,
        change: WorkerAvailabilityAction::Reclaim { worker_ids: vec![4, 5, 6, 7] },
    },
];
let report = simulate_trace_requests_with_hpc_events(
    args, None, None, requests, 4, 1.0,
    ReplayRouterMode::KvRouter,
    events,
    WorkerRecoveryPolicy {
        mode: WorkerRecoveryMode::Restart,
        max_restarts: 2,
    },
    SlaThresholds::default(),
)?;
println!("{:?}", report.worker_availability);
```

Initial workers receive IDs 0–3; subsequent Add events allocate monotonically
increasing IDs. Reclaim cancels worker-owned attempts without advance notice.
Whole-request restart discards partial output and re-submits the original
prompt/output budget through placement under the original logical request UUID.
A restarted request is not counted as an additional user arrival. A failed
attempt with no surviving ready worker is marked permanently failed; this
prototype does not wait for new capacity or migrate partial KV state.

## Validation commands

```bash
cd ~/projects/aisimulate
git fetch origin
git switch --track origin/malleserve-aisim-013-backport
cargo +stable test -p aisimulate-core hpc_

cd ~/projects/dynamo
git fetch origin
git switch --track origin/malleserve-dynosim-013-backport
cargo +stable check -p dynamo-mocker
cargo +stable test -p dynamo-mocker --lib
```

If a local branch name already exists, use `git switch <branch>` and
`git pull --ff-only` instead of `--track`.

**Outstanding validation**: exact compatibility with the pinned Oct 29
development crate, all existing Dynamo live tests, deterministic E1–E4
reporting, and runtime zero-warning failure behaviour. Both code branches
are intentionally kept separate from the original defaults until validated.
