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

## CLI availability trace integration (research branches)

The `aisimulate predict --stack dynamo` YAML interface now accepts a
predict-only `hpc:` mapping for timestamped aggregated replay:

```yaml
hpc:
  availability_trace: experiments/dynosim/traces/hpc_availability_smoke.jsonl
  reclamation: immediate        # immediate | ignored
  recovery: full_request_restart # none | full_request_restart
  max_restarts: 2
```

The JSON Lines trace uses one event per line:

```jsonl
{"timestamp_ms":500,"event":"available","workers":[1]}
{"timestamp_ms":2000,"event":"reclaimed","workers":[1]}
```

Timestamps are nonnegative, in **simulated milliseconds** relative to
the replay start, sorted nondecreasing; workers must be unique in each
record. IDs `0..replicas-1` belong to stable workers. Opportunistic IDs
must start at or above `replicas`. An external worker ID may become
available again after reclamation; a fresh native worker incarnation is
allocated internally. For E2 use `reclamation: ignored` and
`recovery: none`. For E3 use `immediate` and `none`; E4 uses
`immediate` and `full_request_restart` with positive `max_restarts`.
E1 omits the `hpc:` section.

Worker startup is **not** encoded as separate trace events. The ordinary
`engine.workers.aggregated.startup_seconds` config controls when an
opportunistic worker becomes routable after its `available` event.

The CLI bridge supports **offline aggregated** predictions with
independent synthetic requests at timestamped arrival rates and Mooncake
(or Mooncake-delta) traces. It rejects unsupported agentic/disaggregated,
concurrency, online, and Planner configurations. The CLI output includes
`worker_availability` native statistics in prediction metadata.

The CLI source owner is **AISimulate**, not Dynamo. Both research source
branches must be installed together in the same `dynosim` environment:

```bash
conda activate dynosim

cd ~/projects/aisimulate
git switch malleserve-aisim-013-nightly-backport
git pull --ff-only origin malleserve-aisim-013-nightly-backport
cd python/aisimulate
maturin develop --release

cd ~/projects/dynamo
git switch malleserve-dynosim-013-backport
git pull --ff-only origin malleserve-dynosim-013-backport
maturin develop --release -m lib/bindings/python/Cargo.toml
uv pip install --no-deps -e .
```

The Dynamo branch's Python dependency pin is adjusted to the editable
`aisimulate==0.13.0` research build. Use `--no-deps` on the Dynamo
editable reinstall to avoid resolver changes to unrelated packages.

Run a smoke test from the MalleServe root:

```bash
cd ~/projects/malle-serve
git pull --ff-only origin pw
python experiments/dynosim/scripts/run_e2_availability_smoke.py
python experiments/dynosim/scripts/run_e3_reclaim_smoke.py
python experiments/dynosim/scripts/run_e4_restart_smoke.py
```

**Validation is required:** the source edits are not considered
reproducible until the AISimulate Python tests, `cargo test -p aisimulate-core hpc_`,
Dynamo mocker tests, Python binding build, and CLI end-to-end runs pass.
