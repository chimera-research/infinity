# 6. Learn by running small, observable experiments

[Guide index](README.md) · Previous: [Source walkthroughs](walkthroughs.md) · Next: [Evidence](evidence.md)

## Start without credentials or infrastructure

Use a checkout **containing this guide and its added example**. The example is an exploration addition, not part of the pinned upstream revision by itself. From the repository root, use a recent stable Rust toolchain, a C build toolchain for native dependencies, Cargo network access for the initial dependency fetch, and Linux for the optional RSS example. This exploration used Rust 1.99.1 on Linux x86-64; it does not establish the minimum supported Rust version.

```bash
rustc --version
cargo run --locked -p infinity-agent-core --example learning_slices
```

The model and lookup are scripted. The runtime, stores, input queue, observer, and `AgentSystem::step` API are the actual library. It makes no model request, starts no sandbox, reads no cloud credentials, and changes no shared infrastructure. Expected program output, excluding Cargo build messages:

```text
dispatch: the tool call is already in the conversation store
slice 1 ended: 2 stored messages, result still in host queue
slice 2 reloaded history: 4 stored messages, reply emitted
duplicate result: skipped, still 4 stored messages
PASS: commit-before-dispatch, reload, and duplicate-result handling
```

Read [`learning_slices.rs`](../../crates/infinity-agent-core/examples/learning_slices.rs) in this order:

1. `Queue` implements `InputSender` with an unbounded Tokio channel. The host deliberately decides when to deliver its contents. It is not a durable queue.
2. `ScriptedModel` requests `lookup` unless the last message is a tool result. Its determinism makes the state transition inspectable; it is not reasoning intelligence.
3. `Lookup::execute` checks that the stored tail already contains its call, then queues a result. It does not directly mutate the agent's context.
4. `main` runs slice one and checks exactly two stored messages: input and pending call. It drops the system before delivering the result.
5. A rebuilt system reuses the same store handles and processes the callback. Four messages now exist: input, call, result, reply. The observer must contain the expected assistant text.
6. Replaying the result must return `Skipped` and leave the history unchanged.

**What this does not prove:** dropping the system is not killing a process. The shared in-memory store handles remain alive. This is a reload-of-runtime-state demonstration, not a disk durability, crash-recovery, network, or exactly-once side-effect test.

## Use upstream tests as executable explanations

The focused suite avoids building the entire coding application and deploying any services:

```bash
cargo test --locked -p infinity-agent-core -p infinity-rap-bridge -p rap-client -p rap-protocol
```

To isolate three particularly useful questions:

```bash
cargo test --locked -p infinity-agent-core driver_stays_alive_waiting_for_tool_result
cargo test --locked -p infinity-agent-core subscribed_thread_idles_and_event_respawns_it
cargo test --locked -p infinity-rap-bridge repeated_subscription_events_are_not_deduplicated
```

The first two distinguish pending work from an active subscription. The third intentionally preserves identical subscription events; it is not a test of universal deduplication. Source pointers and more test names appear in [the walkthroughs](walkthroughs.md). Inspect assertions alongside test names, and remember that the core suite contains an expected-panic case.

## Try local multi-thread routing, without a performance claim

The existing [`agent_scale` example][scale] uses the resident local API with scripted models and immediate scripted tool results. A small run is enough to see many conversations go through the real router:

```bash
AGENTS=40 TURNS=3 WAVE=10 cargo run --locked -p infinity-agent-core --example agent_scale
```

It prints `agents,rss_bytes` rows, then a completion count on stderr. This configuration produces 240 model completions: 40 agents × 3 turns × 2 completions per turn. It measures RSS after waves, waits briefly for drivers to idle, drains lifecycle notifications, and calls `malloc_trim` where supported. It does **not** use a real model or run the shell command printed in its scripted tool result.

This is a smoke exercise, not a reproduction of the 75k-agent Raspberry Pi headline. At this small size process overhead dominates; neither total RSS divided by agent count nor one run's timing is a general capacity estimate. Debug/release builds, allocator, machine, history size, observer behavior and pending work all matter. See [actual results](evidence.md#executed-checks).

## Move to a real model only after the state transitions make sense

Follow the pinned upstream [first-agent quickstart][quickstart], then [adding tools][tools-guide] and [RAP/MCP connections][connections]. The first example uses Bedrock; it requires an AWS account, valid local credentials, a usable region/model and enabled model access. Inference can incur cost. Do not put credentials in source files or this guide.

The important changes from the scripted step example are:

- Construct a real `ModelProvider` and select a model from its catalog with `StaticModel`.
- Use `AgentSystemBuilder::new_local` for the resident router and run it inside `tokio::task::LocalSet`. Otherwise `spawn_local` cannot run correctly.
- Launch a thread, send text, consume observer events and shut down deliberately.
- Keep receiving across tool rounds when tools are enabled. The upstream minimal example breaks on the first `CompletionFinished`; that is appropriate only when you deliberately want one round, not a generic “task finished” detector.
- Keep the core stores in memory while learning; adopting persistence is a separate decision requiring the [durability analysis](concurrency-and-lifecycle.md).

For Infinity Code itself, follow the upstream [Code documentation][code-guide] and read the daemon/sandbox boundaries first. Running a coding agent can launch commands and modify a repository; use a disposable checkout and understand the selected tools. For Lambda, read the [serverless quickstart][lambda-guide] and CDK source without deploying until you intend to create billable AWS resources. No live-model, coding-sandbox, or cloud deployment was performed for this exploration.

## Exercises that reveal the design

These are optional edits to a disposable copy of the learning example or focused reads—not extra implemented features.

| Exercise | Predict first | What to inspect |
| --- | --- | --- |
| Deliver the same result twice | Does the second delivery add another assistant reply? | It should be skipped; compare store length and observer text |
| Rebuild with entirely new stores | Can the callback reconstruct the missing user/call history? | No; the result lacks a matching call. Store ownership, not the system object's lifetime, preserves the context |
| Read the test that emits two tool calls in one stream | Do both run in parallel? | Only the first survives; compare `has_emitted_tool_call` and the test's action count |
| Compare active-tool waiting with passive sleep | Will a parent message be deferred in both cases? | Read `pending_active_tool_call`, `filter_deferrable`, and local tests |
| Compare fresh and inherited children | Will new parent messages automatically appear in either? | No; a stored cutoff or fresh boundary controls inherited history, while explicit messages carry later updates |
| Compare callback IDs across identical subscriptions | Is payload equality an event identity? | No; the bridge assigns fresh IDs intentionally |

## Common setup traps

- **Missing `jj` or `bwrap`:** the full root `./check.bash` requires coding-sandbox prerequisites before any checks. The targeted commands above do not need those executables. Do not report the whole workspace green just because the subset passes.
- **Dependency/toolchain build errors:** this is a Rust 2024 workspace with a locked dependency graph. Use `--locked`; do not “fix” a learning build by casually upgrading dependencies or rewriting the lockfile.
- **Local callback address unreachable:** the local bridge binds loopback. A remote tool needs an intentionally reachable, authenticated callback surface; copying a local URL to a cloud server will not create one.
- **Waiting forever:** a pending call is not a settled subscription. Check whether the server actually delivered a callback and whether its IDs target the correct thread/call; HTTP acknowledgement alone is insufficient.
- **Example missing from an upstream-only checkout:** `learning_slices` is an addition in [chimera-research/infinity](https://github.com/chimera-research/infinity), not the inspected upstream commit. Use the organizational fork containing this guide; [the evidence record](evidence.md#delivery-state) records its provenance.

[scale]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/examples/agent_scale.rs
[quickstart]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/docs/docs/infinity-runtime/quickstart/launching-your-first-agent.md
[tools-guide]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/docs/docs/infinity-runtime/quickstart/adding-tools.md
[connections]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/docs/docs/infinity-runtime/quickstart/connecting-rap-and-mcp.md
[code-guide]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/docs/docs/infinity-code/overview.md
[lambda-guide]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/docs/docs/infinity-runtime/serverless/quickstart.mdx
