# 1. The conceptual model: a conversation resumed by events

[Guide index](README.md) · Next: [Architecture](architecture.md)

## What Infinity is for

Consider an assistant that starts a build, reviews an independent file while that build runs, and revisits the task when a result arrives. The expensive part is not just reasoning; it is managing work that outlives one model request. Infinity makes that work explicit as tool invocations, independent conversation threads, and messages that resume those threads.

The runtime supplies the machinery for this pattern. It does not supply a universal task planner, a guarantee that a model delegates well, or an automatic correctness check for its answers. Models select tools and produce text; Rust code controls ordering, stores, routing, and execution. The built-in [system prompt][prompt] tells the model how to use threads, but prompt guidance is not the same as a runtime invariant.

## Vocabulary tied to concrete types

| Term | What it means here | Source |
| --- | --- | --- |
| Agent system | Shared stores, model source, tool configuration, and input delivery; contains many conversation threads | `AgentSystemBuilder`, `SystemInner` in [builder.rs][builder] |
| Thread | A conversation identity and its history; not an OS thread | `Thread`, `ThreadId` in [thread.rs][thread] and [rap-protocol][protocol] |
| Root thread | The top conversation in a tree; metadata is resolved through this root | `HistoryManager::new_with_history` in [event_processor.rs][engine] |
| Child/subagent | A new conversation with a parent link, spawn-call identity, and inherited history cutoff or fresh context | `SpawnContext`, `ConversationStore::spawn_thread` in [traits.rs][traits] |
| Slice/step | Prepare a batch of inputs, run a completion round if needed, synchronize history, dispatch at most one asynchronous tool call | `Thread::step_no_defer` in [thread.rs][thread] |
| Turn | Assistant content buffered until a flush point; not necessarily the entire user task | `HistoryManager::turn_buffer`, `flush_turn` in [event_processor.rs][engine] |
| Input | User text, tool result, authorization/choice request, or a synthetic event routed by `group_id` | `InputMessage` in [message.rs][message] |
| Observer event | A notification for the host/UI, such as text, thinking, tool activity, or completion finished | `AgentEvent`, `ThreadObserver` in [events.rs][events] and [observer.rs][observer] |
| Subscription | A tool continues sending events after its initial result; tracked by original tool-call ID | `RapSubscriptionEvent` in [rap-protocol][protocol] |

“Completion finished” does **not** mean “the user's whole task is complete.” It can mean that a model round finished and the runtime is about to dispatch a tool whose result will trigger another round.

## A concrete example: follow a build

This is a conceptual scenario, not a claim that a real build was launched during exploration.

1. A user sends text to a root thread. The runtime loads its history and asks the model what to do.
2. The model invokes a build tool. The runtime records the call, then dispatches it. A RAP server acknowledges receipt; it performs the work outside the agent's completion loop.
3. The tool may return a one-off result, or confirm a subscription and continue sending progress events. These are different protocol messages, not repeated results for one pending call.
4. The agent can spawn a child for another task. That child has a separate history and message stream. The parent does not mutate the child's model context directly; messages carry instructions or reports.
5. A non-associative subscription event normally gets a temporary child to inspect it. An associative event is inserted into the subscribing conversation. “Associative” here is a routing flag, not a mathematical guarantee about combining events.
6. The child may report back. The parent sees a synthetic call/result pair identifying the original spawn or subscription. The runtime preserves a model-compatible conversation format instead of attaching a second answer to an already-answered tool call.

Read [`prepare_input`][engine], [`SpawnThreadTool` / `ReportToParentTool`][thread-tools], and [the RAP chapter](rap.md) for the mechanics. The local driver's deferral policy may delay the event until an active tool call is settled; the Lambda adapter does not use that same deferral policy.

## Features present in this checkout

| Capability | What is implemented | Important boundary |
| --- | --- | --- |
| Embedded agent runtime | Tokio-based local router, per-thread driver futures, handles, live observer events, and step mode | The local system needs a `LocalSet`; it is not one OS thread per agent |
| Concurrent subagents | Spawn, fresh/inherited context, parent reports, parent-to-child messages, self-closing child threads | Parallelism comes from separate threads; later calls in one model stream are ignored |
| Long-lived context | Store traits, ancestor reconstruction, compaction summaries | The default example stores are process memory, not durable storage |
| Provider flexibility | `ModelProvider`; Bedrock implementation; Rig adapter; Unix-socket remote provider transport | Model availability and credentials are external; remote transport is Unix-gated |
| Tool flexibility | Rust `Tool` trait, RAP discovery/callback bridge, MCP bridge | Tool servers still need their own execution, lifecycle, and security policies |
| Event-driven work | Subscriptions, timer wakeups, synthetic reports | Subscription event delivery is not universally deduplicated |
| Human-facing control | OAuth and choice events, replay, structured display segments and named views | A choice message is not a universal permission check on every tool |
| Coding application | Daemon, TUI, web UI, sandbox tools, SSH remotes/migration, ACP entry point | These are application layers, not prerequisites for using the core |
| AWS host | SQS-triggered Lambda, DSQL history, DynamoDB state, callback receiver, CDK constructs | Included infrastructure code was inspected, not deployed |

Source map: [architecture](architecture.md); runtime contracts: [builder][builder], [provider API][provider], [tool API][tools].

## What a child inherits—and what it does not

Default child creation stores a cutoff into the parent's history instead of copying the entire ancestor chain into the child's own stored messages. Loading the child reconstructs that prefix and adds the child's messages. New parent messages after the cutoff are **not** automatically visible. `fresh_context: true` cuts off history inheritance, including for descendants below that fresh boundary.

The runtime also seeds the child's own copy of the spawn tool call, paired with child instructions. This is why later parent messages can still refer to the spawn after compaction. Tool/prompt/model inheritance is a **separate configuration concern**: the convenience launcher uses root-scoped overrides; the daemon has its own per-thread model selection policy. Do not infer config inheritance solely from history inheritance. Sources: [traits][traits], [thread tools][thread-tools], [launch.rs][launch], [daemon stores][daemon-store].

## Claims to interpret narrowly

- **“Like async for threads”** is a useful analogy: waiting can be represented as data and resumed by messages. It is not Rust language-level suspension serialized to disk.
- **“75k agents on a Raspberry Pi”** is an upstream headline, not a result reproduced here. The supplied `agent_scale` example measures idle-history RSS after scripted model/tool turns, with allocator trimming and no real inference. It is not a throughput or active-tool-capacity benchmark.
- **“Zero cost while idle”** can describe the absence of an active serverless invocation, not zero storage, queue, tool-server, or model cost. A local pending-tool thread remains resident.
- **“Same code on local and Lambda”** describes shared core interfaces. Timers, persistence, callback authentication, and deferral are platform choices and differ in the included hosts.

Sources: [upstream README][readme], [scaling example][scale], and [lifecycle analysis](concurrency-and-lifecycle.md).

[builder]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/builder.rs
[thread]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/thread.rs
[protocol]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/rap-protocol/src/lib.rs
[engine]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/event_processor.rs
[traits]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/traits.rs
[message]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/message.rs
[events]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/events.rs
[observer]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/observer.rs
[thread-tools]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/tools/thread.rs
[prompt]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/default_prompt.md
[provider]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-provider-protocol/src/lib.rs
[tools]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/tools/mod.rs
[launch]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/local/launch.rs
[daemon-store]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-daemon/src/memory_store.rs
[readme]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/README.md
[scale]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/examples/agent_scale.rs
