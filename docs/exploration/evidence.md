# 7. Evidence, limits, and delivery state

[Guide index](README.md) · Previous: [Getting started](getting-started.md)

## Scope and provenance

This exploration inspected **Hydro's Rust Infinity**, upstream commit [`1abc6fcff1f186124f924b56aa92c3a3684c90c5`](https://github.com/hydro-project/infinity/commit/1abc6fcff1f186124f924b56aa92c3a3684c90c5), dated 2026-10-27 (`fix: enable aws-config sso feature so SSO profiles work (#176)`). Work was performed on 2026-10-30 in a Linux x86-64 orb with Rust `1.99.1 (58cf8f9c3 2026-10-13)`.

The exploration directly read the core builder/router/driver/thread engine, history and store contracts, built-in thread/subscription/timer tools, model protocol/adapters, RAP types/clients/bridge/spec, MCP bridge, daemon storage/migration/callback code, Lambda handler/stores/receiver, CDK constructs, sandbox command path, Slack dataflow, and relevant tests/examples. This is an architectural and implementation exploration, **not a line-by-line audit of every crate**, a security certification, or a production reliability assessment.

The additions are eight Markdown guide files, one root README link, and `crates/infinity-agent-core/examples/learning_slices.rs`. They do not change production runtime code, dependencies, lockfiles, deployment configuration, or upstream license/notice files. Citations target upstream source rather than pretending the new example existed at that commit.

## Executed checks

Commands were run from the repository root. Cargo compile chatter is omitted below; the outcomes are the actual observed results, not expected examples.

| Check | Observed result |
| --- | --- |
| `cargo test --locked -p infinity-agent-core -p infinity-rap-bridge -p rap-client -p rap-protocol` | **108 core tests and 15 bridge tests passed; 0 failed.** Client/protocol crates had 0 unit tests. One core doctest was ignored; no doctests executed in the other three crates. Repeated before publication with the same pass counts |
| `cargo run --locked -p infinity-agent-core --example learning_slices` | Exit 0; asserted call exists at dispatch, stored history grows from 2 to 4 messages after reloading, expected assistant text emitted, duplicate result skipped with history unchanged |
| `cargo clippy --locked -p infinity-agent-core --example learning_slices -- -D warnings` | Exit 0; no warnings |
| `cargo fmt --all --check` | Exit 0 |
| `AGENTS=40 TURNS=3 WAVE=10 cargo run --locked -p infinity-agent-core --example agent_scale` | Exit 0; 240 scripted completions; elapsed 0.5 seconds as printed by the program |
| `./check.bash` | **Exit 1 at prerequisites: missing `jj`.** The full workspace check did not run. Its script also requires `bwrap` or `sandbox-exec` |

Documentation validation checked all eight guide pages: **61 distinct pinned source paths**, line ranges, reference definitions, **46 relative links** and their heading anchors, three JSON examples, and paired code fences passed. Source-path checks used the pinned local Git tree, not live HTTP requests to every citation. `git diff --check` passed, and license/notice/lockfile comparison showed no changes.

The learning example printed:

```text
dispatch: the tool call is already in the conversation store
slice 1 ended: 2 stored messages, result still in host queue
slice 2 reloaded history: 4 stored messages, reply emitted
duplicate result: skipped, still 4 stored messages
PASS: commit-before-dispatch, reload, and duplicate-result handling
```

The small **debug-build smoke run**, not a capacity benchmark, printed these RSS samples:

```csv
agents,rss_bytes
0,7290880
10,9474048
20,9637888
30,9814016
40,9953280
```

The final sample is about 9.5 MiB. Process overhead is included; no real model, Raspberry Pi, long-running external tool or large-scale backpressure was exercised. The 75k-agent upstream headline remains **unreproduced**.

## Passing tests have a meaningful exception

The core suite includes `parent_message_after_fatal_error_during_active_tool_call` as an explicit `#[should_panic]` expected-failure marker. Its [source comment](https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/tests.rs#L1812-L1829) documents consumed-but-unpersisted inputs after a fatal provider error, leaving a dangling active call against which later parent messages defer indefinitely. The test passed by panicking as expected. This known limitation is not fixed by these documentation additions, and the test totals must not be interpreted as universal recovery coverage.

## What is source-supported, and what remains unverified

| Conclusion | Evidence strength |
| --- | --- |
| Idle local drivers can exit and later respawn | Source plus executed existing local lifecycle tests |
| Pending tool calls keep the local driver resident | Source plus executed `driver_stays_alive_waiting_for_tool_result` |
| Subscription alone does not keep a driver resident | Source plus executed `subscribed_thread_idles_and_event_respawns_it` |
| Async dispatch occurs after history synchronization | Source plus learning-example assertion at dispatch |
| Duplicate one-off result is skipped | Source, existing tests, and learning example |
| Identical subscription callbacks are preserved as separate events | Source plus executed bridge test; not a claim of exactly-once delivery |
| Lambda uses the shared step engine but `NoDeferral` | Inspected source only; not deployed |
| Daemon JSON/DSQL/DynamoDB implement persistence adapters | Inspected source only; no disk-failure, restart, database or migration experiment |
| History writes and external tool effects are not one transaction | Source-derived failure-window analysis; no injected process crash |
| RAP's included clients differ from some spec recommendations | Direct comparison of protocol docs with invocation/callback/notifier implementations |

Not exercised: live inference or provider credentials, AWS deployment, real DSQL/DynamoDB/SQS, local coding sandbox commands, MCP server compatibility, SSH migration, UI rendering, mobile clients, or sustained-load/resource-exhaustion tests. No broad throughput, cost, security, or durability guarantee follows from this exploration.

## Delivery state

The publication destination is **[chimera-research/infinity](https://github.com/chimera-research/infinity)**, a GitHub fork of **[hydro-project/infinity](https://github.com/hydro-project/infinity)**. Initial fork creation was blocked by HTTP 403, `Resource not accessible by integration`. After the user confirmed that the organizational fork had been created, publication resumed using the preserved local artifacts.

Before publication, GitHub reported `fork: true`, with both `parent.full_name` and `source.full_name` equal to `hydro-project/infinity`, and confirmed push access. The fork had only an unprotected `main` branch and no open pull requests. Fetching both remotes showed that the fork's `main`, upstream `main`, and the local base all pointed to the inspected revision, [`1abc6fcff1f186124f924b56aa92c3a3684c90c5`](https://github.com/hydro-project/infinity/commit/1abc6fcff1f186124f924b56aa92c3a3684c90c5). No revision drift required reconciliation, and neither new artifact path existed in the destination tree.

The publication adds the guide, root README link, and learning example on top of that history. It preserves production runtime code, dependencies, deployment workflows, [LICENSE.txt](../../LICENSE.txt), [NOTICE](../../NOTICE), and [THIRD-PARTY](../../THIRD-PARTY). The destination is the fork's `main` branch through a normal fast-forward push, without force-pushing, replacing another repository, or rewriting upstream history.

Read the [guide index](README.md) in the fork for the published artifacts. The source analysis remains pinned to the upstream revision above rather than silently treating later upstream changes as inspected. No cloud deployment or release is part of this publication.
