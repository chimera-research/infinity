# 5. Source walkthroughs: follow one message across boundaries

[Guide index](README.md) · Previous: [RAP](rap.md) · Next: [Getting started](getting-started.md)

These reading routes are pinned to [the inspected revision](README.md#revision-and-attribution). Start with one route rather than reading the entire large `event_processor.rs` linearly. Each route names the question the code answers, the important decision, and an existing test to read alongside it.

## Route A: from user text to an asynchronous tool

### 1. Choose the host, not a different engine

Open [`system/builder.rs`, `new_local` and `step`][builder]. `new_local` arranges an in-process channel; `start` launches the convenience local API. The general constructor accepts an `InputSender`, and `build` returns a step-mode system. Both share `SystemInner`, including conversation/state stores, model source, config source, sender, callback URL, and built-in-tool switches.

Read `AgentSystem::step` closely: grouping is by thread, loads/filtering happen before the concurrent `join_all`, and errors are collected after all launched steps finish. A load/filter error can still return before those steps launch. This is why “one thread failure never cancels a sibling mid-commit” should not be expanded to “every input always runs regardless of all errors.”

### 2. See where the resident loop lives

Open [`route_loop`][router] and [`drive_thread`][driver]. The router owns one worker channel pair per live conversation and a pool of driver futures. `spawn_local` starts the router itself. When an existing worker receiver has closed, the router can create a new driver rather than requiring the caller to retain a thread task.

Inside the driver, find the `tokio::select!` that polls the in-flight step alongside incoming messages and subscription requests. User text cancels the step; other messages wait. Then find `check_before_idle`: the no-await final check is the teardown boundary. Tests to pair with this read: `routes_to_separate_threads`, `respawns_driver_after_idle`, and `driver_stays_alive_waiting_for_tool_result` in [system tests][tests].

### 3. Locate the commit barrier

Open [`Thread::step_no_defer`][step]. Preparation handles every input before model invocation. Configuration is cached on the loaded thread; model selection is resolved for the step. Observer events stream synchronously while `run_completion` yields engine events. At the end, the code says:

```rust
self.history.sync().await?;
observer.on_event(&thread_id, &AgentEvent::CompletionFinished { usage });
```

Only afterward does it call `execute_action_with_error_result`. This ordering explains both why callback correlation works and why `CompletionFinished` is too early to mean “tool work is done.” Compare the executable [learning example](../../crates/infinity-agent-core/examples/learning_slices.rs), whose tool checks the store *at dispatch time*.

### 4. Separate one stream from the completion loop

Open [`run_completion`][completion]. `has_emitted_tool_call` ensures only the first tool call from a stream is used. `Final` still passes through to finalize the round; trailing text, reasoning and additional calls are suppressed so they cannot displace the pending call at the history tail.

For a known `supports_sync()` tool, the engine synchronizes history, computes `safe_spawn_point`, executes it inline, adds its result and sets `should_loop_back`. An unknown tool also gets an injected error and a loopback. For an asynchronous tool, the engine yields a `CompletionAction` for the caller to dispatch. Read `concurrent_tool_calls_suppress_trailing_stream_content`, `multiple_sync_tool_calls_chain`, and `async_tool_call_yields_execute_action` in [engine tests][completion-tests].

**What this route teaches:** the agent is a conversation plus a step function, but the local host still has a real cooperative scheduler, queues, cancellation rules, and resource lifetimes.

## Route B: from tool callback to the next model context

Start at [`RapCallback`][protocol], then [`prepare_callback`][bridge]. The latter separates view state immediately, assigns stable IDs to one-off callbacks and random IDs to subscription events. `callback.rs` converts wire payloads to internal user content and synthetic-event tags.

Follow `Thread::prepare_one` to [`prepare_input`][prepare]. Before ordinary history handling it checks closure, handles compaction, updates metadata, and extracts OAuth/choice requests. Synthetic reports/events find the originating tool call and either form an injected event or create a child. This means input handling is not just “deserialize and append.”

Next read [`HistoryManager::handle_content`][history-input]. The pending call is represented by history shape. A matching result settles it; stale or duplicate results do not start a useful model round. A new user/synthetic input may require an interrupted-result placeholder to preserve call/result alternation.

Finally open [`InfinityMessage::into_messages`][message]. Stored application messages are translated into provider protocol messages. A synthetic event can become a model-facing call/result pair; presentation metadata is not simply dumped into the prompt. This is the boundary at which to ask “what did the model actually see?” rather than reading a UI event log.

Read the bridge tests `stable_dedup_for_single_callbacks`, `repeated_subscription_events_are_not_deduplicated`, and `view_update_remains_out_of_agent_history`. Then read `subscription_event_deferred_during_async_tool_wait` and `stale_result_does_not_flush_deferred_events_during_async_tool_wait` in [system tests][tests]. They expose why delivery identity and scheduling policy are different concerns.

## Route C: spawn, inherit, report, compact

Open [`SpawnThreadTool::execute_synchronous`][spawn]. The tool verifies `child_of` against the actual thread stack. This is a check against the model confusing parent and child context, not a user-authentication mechanism. It chooses `SpawnContext::Fresh` or `InheritUpTo(safe_spawn_point)`, creates the child, stores a copy of the spawn call in the child's own history, and sends the instructions as its result. The parent gets a synchronous result containing the child identity.

Why put a spawn call in the child's own rows? Later parent messages need a stable originating call even if ancestor context or an inherited prefix has been summarized. The child owns that reference instead of depending only on a parent's old transcript slice.

Now open [`ConversationStore::load_history_with_ancestors`][traits]. History bounds are counts, not arbitrary SQL sequence values. A fresh-context boundary cuts off older ancestors. Otherwise the loader finds an eligible compaction summary that does not exceed the relevant ancestor cutoff, then adds uncovered ancestor slices and leaf rows.

Follow `ReportToParentTool` and `CloseThreadTool` in [thread tools][spawn]. Reports route through the input sender; closing checks that the target is the calling child. A compaction child closes with a summary, saves it against the parent cutoff, and sends the special compaction-complete input. The parent can then replace its model-visible prefix without deleting all old storage.

Useful paired tests: `fresh_context_spawn_has_no_parent_history`, `nested_spawn_inherits_only_fresh_parent_context`, `child_close_report_wakes_sleeping_parent_after_compaction`, and `parent_message_reaches_child_after_child_compaction` in [system tests][tests]. Read the assertions, not only names: some older test names describe the bug they now prevent.

## Route D: inspect a failure without confusing buffers and storage

Open [`HistoryManager`][history-input] and locate `unvalidated_inputs`, `turn_buffer`, and `pending_items`. Trace one user input until a successful model chunk calls `mark_inputs_model_validated`. Trace an overflow before that point: the recovery branch can drop unvalidated user text or replace a tool result with a placeholder. Then trace a transient error after partial text: the turn is discarded before retry; terminal/cancel paths can preserve visible text while trimming reasoning.

Read [`sync`][sync] as two independent writes, not a transaction:

```rust
self.conversation_store
    .append_messages(&self.thread_id, msgs)
    .await
    .map_err(|e| Box::new(e) as BoxError)?;
```

The subsequent selected-ID write uses `let _ = ...await`. That error-handling difference is material to the durability claims. For the host side, compare [in-memory stores][stores], [daemon files][daemon-store], and [DSQL append][dsql]. This guide does not modify them or claim fault-injection coverage.

Pair the read with `oversized_user_input_is_dropped_and_not_persisted`, `oversized_tool_result_is_replaced_with_placeholder_and_retried`, and `broken_stream_discards_partial_turn_before_retry` in [engine tests][completion-tests]. There is also an explicit expected-panic regression case, `parent_message_after_fatal_error_during_active_tool_call`, in [system tests][tests]. A green suite includes that expected panic; it is not evidence that every fatal-error/event interaction is supported.

## Route E: compare a cloud slice and a real tool server

Open [`infinity-agent-lambda/src/event_handler.rs`][lambda]. The host builds the same core around database stores and an SQS sender, then calls `step` with `NoDeferral`. Compare its durable timer tools with Tokio sleep in the local builder. Read the [RAP receiver][receiver] to see callbacks converted before SQS enqueue; views are acknowledged without becoming agent input.

Then open [`sandbox-core/src/server.rs`][sandbox]. Find the invocation handler's spawned task and the `execute_command` dispatch branch. The tool process owns command execution, streaming, and subscription completion. These tasks do not vanish when an agent finishes a model round. Finally inspect [the CDK construct][cdk] for FIFO grouping and callback authentication rather than inferring deployment properties from the generic Rust API.

**What this route teaches:** the shared runtime is real, but the transport, durability, process boundary, timer policy, and tool workload remain concrete infrastructure responsibilities.

[builder]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/builder.rs#L297-L494
[router]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/local/router.rs#L239-L405
[driver]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/local/driver.rs#L87-L365
[tests]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/tests.rs
[step]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/thread.rs#L423-L594
[completion]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/event_processor.rs#L1480-L1910
[completion-tests]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/event_processor.rs#L1980-L5207
[protocol]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/rap-protocol/src/lib.rs#L43-L241
[bridge]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-rap-bridge/src/lib.rs#L40-L143
[prepare]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/event_processor.rs#L889-L1307
[history-input]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/event_processor.rs#L120-L356
[message]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/message.rs
[spawn]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/tools/thread.rs
[traits]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/traits.rs#L26-L154
[sync]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/event_processor.rs#L627-L671
[stores]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/stores.rs
[daemon-store]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-daemon/src/memory_store.rs
[dsql]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-lambda/src/conversation_history.rs
[lambda]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-lambda/src/event_handler.rs
[receiver]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-lambda/src/bin/rap-receiver.rs
[sandbox]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/sandbox-core/src/server.rs
[cdk]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/agent/lib/infinity-agents/index.ts
