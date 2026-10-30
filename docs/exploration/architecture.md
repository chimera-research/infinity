# 2. Architecture: the core is smaller than the application

[Guide index](README.md) · Previous: [Concepts](concepts.md) · Next: [Concurrency and lifecycle](concurrency-and-lifecycle.md)

## Read the repository as three layers

**The reusable runtime decides how a conversation advances. Host adapters decide where messages and state live. Applications decide how people interact with it.** RAP crosses the boundary from runtime to independently executing tools; the model-provider protocol is a different boundary.

| Layer | Modules to open | Responsibility |
| --- | --- | --- |
| Runtime | [`infinity-agent-core/src/system`][builder], [`event_processor.rs`][engine], [`traits.rs`][traits] | System construction, thread steps, history, routing policies, built-in tools |
| Model contract | [`infinity-provider-protocol`][provider] | Provider-independent messages, streams, model catalogs, errors; optional remote transport |
| Tool wire contract | [`rap-protocol`][rap] | Invocation/callback types, tool manifests, IDs and display content |
| RAP plumbing | [`rap-client`][rap-client], [`infinity-rap-bridge`][rap-bridge] | HTTP discovery, lifecycle notifications, callback listener and conversion into agent input |
| MCP adaptation | [`infinity-mcp-bridge`][mcp] | Lazy stdio/HTTP MCP session; list/invoke operations exposed as Infinity tools |
| Local coding host | [`infinity-daemon`][daemon] | Session management, persistence, provider/tool processes, callbacks, clients, migration |
| Cloud host | [`infinity-agent-lambda`][lambda], [`agent/lib/infinity-agents`][cdk] | SQS slices, DSQL/DynamoDB, durable timers, IAM-authenticated receiver, deployment constructs |
| Coding tools | [`sandbox-core`][sandbox], [`sandbox-local`][sandbox-local], [`sandbox-remote`][sandbox-remote] | RAP server, command/file tools, repository workspaces and platform backends |
| User surfaces | [`infinity-agent-cli`][cli], [`infinity-web`][web], [`infinity-ui`][ui] | TUI/ACP client, web entry point, shared React components |
| Slack application | [`infinity-slack-dataflow`][slack], [`infinity-slack-bot`][slack-bot] | Slack events and daemon commands; separate from the core runtime |

The project belongs to Hydro, but **the core execution loop is ordinary Rust/Tokio, not a Hydro-compiled distributed dataflow**. Hydro's `hydro_lang` is explicitly used by the Slack dataflow crate. Compare the [core manifest][core-manifest] with [the Slack manifest][slack-manifest] and `slack_dataflow` in [flow.rs][slack]. This distinction avoids attributing dataflow guarantees to code that does not use that execution model.

## The shared execution boundary

The high-level runtime converges on the same `Thread` logic in both hosts. This simplified call tree shows ownership, not every call or all await ordering:

```text
Local host: route_loop -> drive_thread
Step host:  AgentSystem::step (partition by group_id)
  Thread::load
    HistoryManager::new_with_history
  Thread::filter_deferrable
  Thread::step_no_defer
    prepare_one -> prepare_input
    ModelSource::resolve
    run_completion
      ModelProvider::invoke_model
    HistoryManager::sync
    execute_action_with_error_result -> Tool::execute
```

The local driver reuses a loaded `Thread` while it remains live. Step mode reloads it for each call and can drop it afterward. `AgentSystem::step` groups the input batch in arrival order per thread, loads/filters each group, then `join_all`s the thread steps. The `&mut self` API serializes calls on one system instance; separate processes still require transport-level serialization. Sources: [builder][builder], [thread][thread], [router][router], [driver][driver].

## Extension points and what each must preserve

| Interface | What you replace | Contract to preserve |
| --- | --- | --- |
| `ConversationStore` | History, thread hierarchy, closed/fresh/compaction flags and summaries | Count-based history bounds and spawn cutoffs; per-thread serialized writes |
| `StateStore` | Processed IDs, root metadata, active subscriptions, pending choices, optional stopped policy | Correct thread ownership; failures matter to recovery semantics |
| `InputSender` | Local delivery or a durable queue | Route by `group_id`; choose meaningful delivery IDs and ordering |
| `ModelProvider` | Inference backend | Return normalized stream chunks, final usage, catalog capabilities and classified errors |
| `ModelSource` | Model selection | Resolved at the start of a step's completion phase; sync loopbacks use that selected model |
| `ThreadConfigSource` | Tools, prompt, RAP notifier | Resolved lazily and cached for that loaded `Thread`; not refreshed on every token or tool call |
| `Tool` | In-process dispatch or network invocation | Async dispatch returns control and arranges a later input; sync-capable tools must actually return a result |
| `DeferQueue` | Where events wait behind an active call | Return whether an event was retained; durable hosts need durable deferral if they want it |
| `ThreadObserver` | Event fan-out and replay | Keep synchronous callbacks fast; install replay and live subscription together |

Sources: [traits][traits], [provider][provider], [thread][thread], [tool API][tools], [deferral][defer], [observer][observer]. These contracts make adaptation possible; they do not make all implementations equivalent.

## Decisions worth understanding

### Local ownership favors cooperative concurrency

`SystemInner` uses `Rc`; `HistoryManager` uses `RefCell` and `Cell`. The router is spawned on a Tokio `LocalSet` and owns drivers in `FuturesUnordered`. Different agents can make progress while peers await I/O without making every history mutation thread-safe. The underlying reusable in-memory stores use `Arc<Mutex<...>>` so cloned handles share state.

**Tradeoff (inference):** a blocking tool implementation, long observer callback, expensive synchronous script, or CPU-heavy step can delay unrelated agents on that local executor. This design is I/O concurrency, not automatic CPU parallelism. Sources: [router][router], [engine][engine], [stores][stores].

### A finished driver is removed, not kept forever

The router polls driver futures directly and removes closed worker channels when they finish. An idle conversation is reloaded on demand. The comments explicitly contrast this with retaining task handles/worker entries until the next message. History, configuration registries, observers and application-owned resources may still occupy memory.

**Tradeoff:** repeated activity after idling reloads history/configuration; active subscriptions can wake a new driver, while unanswered tool calls keep the existing local driver resident. See [router][router] and [driver][driver].

### Channels and collections are simple, not capacity controls

The local router, drivers, observer handles, and callback bridge use unbounded channels. The in-memory deferral queue is a vector. This makes routing straightforward and removes “queue full” branches from the core path.

**Tradeoff (inference):** a high-rate tool event stream can outpace inference and consume memory; the inspected local path does not establish bounded admission/backpressure or a maximum agent count. The headline agent-count benchmark is not a queue-pressure test. Sources: [router][router], [driver][driver], [defer][defer], [RAP bridge][rap-bridge].

### History is richer than the model transcript

`InfinityMessage` preserves display metadata and synthetic event identity. `into_messages` converts it into the model-facing `Message` shape. A stored subscription event can expand into two model messages: a synthetic assistant call and its result. The rare event payload is boxed to avoid inflating every element in a history vector. Consecutive text chunks are coalesced before storage.

**Tradeoff:** model-message counts and stored-row counts are not interchangeable. Compaction and spawn indices must use the store's count convention, not the UI's count. Sources: [message types][messages], [engine][engine], [traits][traits].

### Provider normalization contains backend-specific behavior

The model protocol crate has no model SDK dependency. Bedrock translates requests/chunks and classifies failures; the Rig adapter translates another model abstraction. The engine reacts to `Transient`, `Throttled`, `ContextOverflow`, and `Fatal`, instead of matching provider error strings itself. Unsupported image inputs become text placeholders at the model-facing boundary, not destructive edits to stored images.

**Tradeoff:** correct error classification and capability metadata are crucial. A provider that calls a permanent configuration failure transient causes retries. A model's context size comes from catalog/configuration, not dynamic runtime measurement. Sources: [provider API][provider], [completion types][completion], [Bedrock][bedrock], [Rig][rig], [engine][engine].

### The daemon is an application, not a second core engine

The daemon layers per-thread JSON files, titles, selected models, view state, managed RAP servers, and session lifecycle on core store semantics. Clients speak `infinity-protocol`; model processes speak `infinity-provider-protocol`; tools speak RAP. These are three different protocols with different jobs.

Migration is explicit orchestration: stop source activity, coordinate tool-server migration, serialize/import conversation state, and archive the source after success. It is not a live memory image or a cross-host distributed transaction. Source: [daemon stores][daemon-store], [migration orchestrator][migration].

[builder]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/builder.rs
[engine]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/event_processor.rs
[traits]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/traits.rs
[provider]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-provider-protocol/src/lib.rs
[completion]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-provider-protocol/src/completion.rs
[rap]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/rap-protocol/src/lib.rs
[rap-client]: https://github.com/hydro-project/infinity/tree/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/rap-client/src
[rap-bridge]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-rap-bridge/src/lib.rs
[mcp]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-mcp-bridge/src/lib.rs
[daemon]: https://github.com/hydro-project/infinity/tree/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-daemon/src
[lambda]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-lambda/src/event_handler.rs
[cdk]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/agent/lib/infinity-agents/index.ts
[sandbox]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/sandbox-core/src/server.rs
[sandbox-local]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/sandbox-local/src/backend.rs
[sandbox-remote]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/sandbox-remote/src/backend.rs
[cli]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-cli/src/main.rs
[web]: https://github.com/hydro-project/infinity/tree/1abc6fcff1f186124f924b56aa92c3a3684c90c5/infinity-web
[ui]: https://github.com/hydro-project/infinity/tree/1abc6fcff1f186124f924b56aa92c3a3684c90c5/infinity-ui
[slack]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-slack-dataflow/src/flow.rs
[slack-bot]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-slack-bot/src/main.rs
[core-manifest]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/Cargo.toml
[slack-manifest]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-slack-dataflow/Cargo.toml
[thread]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/thread.rs
[router]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/local/router.rs
[driver]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/local/driver.rs
[tools]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/tools/mod.rs
[defer]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/defer.rs
[observer]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/observer.rs
[stores]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/stores.rs
[messages]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/message.rs
[bedrock]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-provider-bedrock/src/lib.rs
[rig]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-provider-rig/src/lib.rs
[daemon-store]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-daemon/src/memory_store.rs
[migration]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-daemon/src/migrate.rs
