# 3. Concurrency, lifecycle, and persistence

[Guide index](README.md) · Previous: [Architecture](architecture.md) · Next: [RAP](rap.md)

## Three kinds of concurrency, not one

**Across conversations:** a local router polls many thread drivers concurrently. Each driver owns its loaded history and serializes its own steps. In step mode, `AgentSystem::step` partitions inputs by `group_id`, preserves order within a group, and joins independent steps. A caller running multiple host instances must still serialize the same thread; the Rust borrow on one system does not protect another process. Sources: [router][router], [builder][builder].

**Between reasoning and external work:** an asynchronous tool dispatch returns without waiting for its final result. RAP's HTTP acknowledgement is just delivery acknowledgement. A callback later becomes input for another step. The tool server can keep a process, network operation, or subscription running while the agent is not requesting inference. Sources: [`step_no_defer`][thread], [`invoke_rap_tool`][rap-tool].

**Inside a model stream:** the engine accepts the first tool call and ignores subsequent non-final chunks. It does not launch every call the provider emits in parallel. A tool marked synchronous can execute inline, append a result, and loop into another provider request in the same step. Thus “one completion round” is not a fixed bound of one HTTP model request or one small amount of work. Source: [`run_completion`][engine], especially `has_emitted_tool_call`, `supports_sync`, and `should_loop_back`.

## Local lifecycle: idle, waiting, stopped, and closed differ

| Situation | Local driver behavior | What remains |
| --- | --- | --- |
| Queued user input | Load/respawn thread; prepare input and start completion | Stores plus live driver |
| Streaming a model response | Poll stream and input/subscriber channels | In-flight stream and uncommitted turn buffer |
| Waiting for an unanswered tool call | Park the driver on its input channel | Loaded history and driver remain resident |
| Passive sleep tool pending | Events may interrupt it; still counts as awaiting a result for local liveness | Driver remains resident until settled/interrupted |
| No queued work and no unanswered call | Final synchronous queue check, then drop driver | Stored history, subscription state, host-owned resources |
| Subscription active but initial call already answered | Subscription alone does not keep driver live | A later event can respawn it |
| Thread closed | Persist closed flag; late inputs are not normal new work | Historical data remains; closure is not deletion |
| Host marks session stopped | Admission policy can reject non-user events | Host state remains; stopped is a host policy, not the `Idle` event |

The special `close_thread` call expects no answer, so it does not pin a driver forever. The built-in tool closes only the calling child thread, not an arbitrary target or a root. Sources: `awaiting_tool_result`, `pending_active_tool_call` in [thread.rs][thread]; `check_before_idle`, `DriverGuard` in [driver.rs][driver]; [thread tools][thread-tools]; [router admission][router].

**Why the final idle check matters:** the router and driver run on the same `LocalSet`. `check_before_idle` does not await. An input is either already in the driver channel and gets drained, or arrives after its receivers close and causes the router to respawn the driver. Existing tests cover idle exit and respawn; the no-yield race argument comes from the implementation. This is a local scheduling invariant, not a cross-process lock.

`ThreadLifecycleState::Live` / `Idle` report driver residency. `AgentEvent::CompletionStarted` / `CompletionFinished` report model-round progress. Neither pair is a task-completion protocol for the user's goal.

## What interruption does

During an in-flight local completion, arriving plain user text cancels the stream. The driver prioritizes that user input and adds `<interrupt>` to the first interrupting text. Non-user arrivals are normally queued until the in-flight step finishes. Cancellation preserves valid partial assistant text but trims incomplete trailing reasoning; a retryable stream failure discards the partial turn before retrying. Terminal errors can preserve text too. Source: [driver][driver] and `flush_turn_trimming_reasoning`, `discard_turn` in [engine][engine].

While an **active** asynchronous tool is pending, the local `InMemoryDeferQueue` holds subscription events, child reports, and parent messages. Processing those immediately would insert an “interrupted” result to keep the transcript well-formed and make a later real result stale. Passive sleep tools are interruptible by events. A correct result that settles the pending call releases deferred work; a stale result should not. Sources: [deferral contract][defer], `filter_deferrable` in [thread][thread].

The included Lambda handler passes `NoDeferral`, which explicitly processes such events immediately and accepts interruption semantics. Its behavior is **not identical to the local driver**. A durable transport could implement `DeferQueue`, but that is an extension point, not an implementation demonstrated by this adapter. Source: [Lambda event handler][lambda].

Cancelling a model request or notifying a tool server does not undo a side effect that already happened. The RAP notifier is best-effort; its calls are awaited and servers are visited sequentially. A pending call can therefore be “interrupted” in history while external work continues. Sources: [thread][thread], [notifier][notifier].

## History staging prevents one class of poisoned restart

`HistoryManager` separates three states:

1. **Unvalidated inputs:** accepted user text/results in the current view, not yet safe to persist.
2. **Turn buffer:** streamed assistant content, not yet committed as a completed or safely interrupted turn.
3. **Pending items:** known-safe messages ready for `sync` to append.

Once the model produces output, inputs that preceded it can be promoted. If oversized input causes context overflow before any usable output, the engine need not persist the offending payload and reload it forever. There are recovery branches for replacing oversized tool results, discarding failed partial turns, and reporting errors. This is input/retry hygiene, **not a proof that arbitrary prompts are safe or that all provider failures recover**. Source: `mark_inputs_model_validated`, `flush_turn`, `replace_unvalidated_tool_results`, `run_completion` in [engine][engine].

Transient/throttled failures have retry logic, but the retry counter resets after successful stream chunks. There is no core stream-inactivity timeout in this loop. An endlessly stalling provider or repeatedly failing-after-output stream therefore has different behavior from a provider that immediately returns a bounded sequence of errors. Provider adapters and hosts may impose their own limits; this exploration did not test those failure modes against live providers.

## “Persist before dispatch” is useful, but not exactly-once

The normal asynchronous path orders work this way:

```text
prepare inputs; stream model; flush turn
append known-safe history to ConversationStore
record selected processed IDs in StateStore
emit CompletionFinished
dispatch asynchronous Tool::execute
later: receive result and run another step
```

This lets a callback find the originating call in history. The learning example checks that observable property at dispatch. However:

- History append and processed-ID recording are separate writes. `HistoryManager::sync` currently ignores the error from `add_processed_message_ids`.
- A crash after persisting the call but before dispatch can leave an unanswered call with no external operation. No transactional dispatch outbox was found in the inspected path.
- A network failure after a server accepts a call is ambiguous: a reported dispatch error does not establish that no external side effect occurred.
- Sync tools also call `history.sync()` before execution, inside `run_completion`. This is still not an atomic transaction around the tool's own writes and messages.
- One-off tool-result deduplication checks history; selected user/event IDs use state. Neither turns the queue, store, and tool server into one transaction.

Sources: [`HistoryManager::sync`][engine], [`Thread::step_no_defer`][thread], [`invoke_rap_tool`][rap-tool]. These are source-derived failure windows, not injected crash tests or observed production incidents.

## The persistence choices have different durability

| Host/store | Stored state | What to assume |
| --- | --- | --- |
| Core in-memory stores | Maps for conversations/hierarchy/summaries and runtime state, shared with `Arc<Mutex<...>>` | Survives rebuilding an agent system only while handles remain in the same process; does not survive process exit |
| Infinity Code daemon | Per-thread JSON history plus `.meta.json`, `.state.json`, `.views.json`; separate session data | Intended restart persistence, but several writes discard I/O errors; inspected writes do not use atomic rename/fsync as a durability protocol |
| Lambda | DSQL history/hierarchy/compaction summaries; DynamoDB metadata/processed IDs/subscriptions/choices | External storage enables slice reload, but writes across services and external effects are not atomic |

Sources: [core stores][stores], [daemon store][daemon-store], [DSQL][dsql], [DynamoDB][dynamo]. The DSQL append implementation reads the maximum message order before beginning the insert transaction. Its primary key is `(session_id, message_order)`, not a uniqueness constraint on the delivery ID. Correct per-thread serialization matters; database presence alone does not make concurrent writers safe.

The SQS sender sets a message group per thread and appends a timestamp to its deduplication ID; the callback receiver uses the bridge's prepared ID. Those are transport-specific choices. In particular, a subscription callback gets a new random ID at the bridge, so resending identical HTTP content can produce another event. See [SQS sender][sqs], [callback receiver][receiver], and [RAP](rap.md#identity-and-redelivery).

## Compaction shortens model context, not necessarily storage

The local driver triggers background compaction when reported total tokens exceed 75% of the model's configured context window; a zero window disables this trigger. A compaction child inherits a safe prefix and reports a summary back. The parent stores the summary with a cutoff and gets a completion notification. Later context reconstruction uses the summary plus the uncovered tail.

Old stored messages are not thereby deleted. Looking up an original tool call can still scan the thread's full own history, important for subscription and parent/child references after compaction. Spawn cutoffs are counts of stored messages; a child must not accidentally inherit a later parent summary extending beyond its spawn point. Sources: [driver][driver], [thread tools][thread-tools], [`load_history_with_ancestors`][traits], `find_tool_call` in [engine][engine].

**Tradeoff (inference):** summarization trades exact context for model-generated compression, and retaining old rows preserves references at a storage cost. The background threshold belongs to the local driver; do not assume the Lambda step wrapper runs the same auto-compaction policy.

[router]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/local/router.rs
[builder]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/builder.rs
[thread]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/thread.rs
[rap-tool]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/tools/rap_tool.rs
[engine]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/event_processor.rs
[driver]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/local/driver.rs
[thread-tools]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/tools/thread.rs
[traits]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/traits.rs
[defer]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/defer.rs
[lambda]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-lambda/src/event_handler.rs
[notifier]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/rap-client/src/notifier.rs
[stores]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/stores.rs
[daemon-store]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-daemon/src/memory_store.rs
[dsql]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-lambda/src/conversation_history.rs
[dynamo]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-lambda/src/state_store.rs
[sqs]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-lambda/src/tools/sqs_sender.rs
[receiver]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-lambda/src/bin/rap-receiver.rs
