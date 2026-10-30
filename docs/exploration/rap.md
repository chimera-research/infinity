# 4. Reactive Agent Protocol: acknowledge now, resume later

[Guide index](README.md) · Previous: [Concurrency and lifecycle](concurrency-and-lifecycle.md) · Next: [Source walkthroughs](walkthroughs.md)

## The problem RAP addresses

The runtime should not need a continuously open tool-request connection to receive a result hours later. RAP separates **invocation acknowledgement** from **result delivery**, and carries conversation identity and a callback address across that boundary. A tool can also become a source of future events.

RAP is an HTTP/JSON protocol, not a scheduler, database, or distributed transaction. Its source of truth for Rust shapes is [`rap-protocol/src/lib.rs`][types]; the normative language lives in the [specification][spec]. The included clients sometimes implement less than the specification recommends.

## Discover, invoke, callback

1. `ToolsetLoader` fetches `/.well-known/rap-toolset`. A manifest has `name`, `endpoint`, `tools`, and optional metadata. Each tool supplies `name`, `description`, `inputSchema`, optional annotations, and optionally a `displayScript`. These become model-facing tool definitions; the loader has a session-scoped cache. [Discovery implementation][loader].
2. `invoke_rap_tool` posts a `RapInvocation` to the manifest's endpoint. `operation` selects the tool; `arguments` carries the model's JSON. [Invocation implementation][invoke].
3. The server acknowledges promptly. The specification requires HTTP 200; this client accepts any 2xx response and does not read a result from its body. A callback, not the acknowledgement, settles the call.
4. The server posts a `RapCallback` to `callback_url`. The bridge turns it into an `InputMessage`, or routes a view update outside agent history. [Bridge][bridge] and [conversion][conversion].

Illustrative invocation, matching the inspected wire shape (example domains are placeholders, not runnable endpoints):

```json
{
  "operation": "lookup",
  "arguments": {"key": "color"},
  "id": "lookup-1",
  "call_id": null,
  "callback_url": "https://runtime.example/callback",
  "group_id": "thread-7",
  "thread_ancestors": ["root-1"]
}
```

Its later one-off result:

```json
{
  "type": "tool_result",
  "group_id": "thread-7",
  "id": "lookup-1",
  "text": "coral"
}
```

`group_id` is the conversation thread, not a user account. `id` is the runtime tool-call identity. Optional `call_id` preserves a provider's own correlation identity. `thread_ancestors` lists root through immediate parent, excluding the current thread. `user_id`, if supplied, comes from runtime metadata; merely carrying it is not proof of authorization. The callback URL is an opaque endpoint whose authentication and lifetime belong to the host. Sources: [types][types], [invocation][invoke], [transport spec][transport].

## Five callbacks with different semantics

| Callback | Meaning in the inspected runtime | Does it resume the model? |
| --- | --- | --- |
| `tool_result` | Settle the corresponding pending call; optionally mark it as a subscription | Normally yes, if accepted rather than duplicate/stale |
| `subscription_event` | New information associated with an earlier subscribing call | Yes, either in the parent or a child, subject to deferral/admission |
| `oauth` | Give the host an authorization URL | Out-of-band UI notification; not ordinary model text |
| `user_choice` | Prompt/options/default plus a response URL; persist pending choice for host interaction | Choice handling is out-of-band; later tool result resumes work |
| `view_update` | Replace a named view's JSON state for clients | No; deliberately kept out of model history |

`tool_result.content` can carry text and base64 images. Nonempty structured content takes precedence in the callback converter; it falls back to `text`, then empty text. The type comments say a tool must provide text or content, but deserialization and conversion do not enforce that as a nonempty-payload requirement. `display_as` is a prioritized human presentation (text, diff, image), separate from model content. A view is yet another channel: the daemon can persist/fan it out, while the Lambda receiver acknowledges it without using it. Sources: [types][types], [conversion][conversion], [thread preparation][thread], [daemon callback][daemon-callback], [Lambda receiver][receiver].

## Subscriptions create new inputs, not extra answers

The initial result includes `"subscription": true`, which settles the invocation and adds active-subscription state. Future callbacks use `tool_call_id` to refer back to it:

```json
{
  "type": "subscription_event",
  "group_id": "thread-7",
  "tool_call_id": "lookup-1",
  "text": "The watched value changed to amber",
  "associative": true,
  "final": false
}
```

- **Associative:** add a synthetic call/result pair to the subscribing thread. A fresh invocation ID keeps the model transcript valid; this is not a second result for the original call.
- **Non-associative (default):** spawn an event child with the relevant context and event payload. That child can report to the parent using the original subscription reference.
- **Final:** remove active subscription tracking while still delivering the event.
- **Cancel:** the built-in cancellation tool updates tracking and can notify servers. Notification is best-effort, not a guarantee that no later message will arrive.

Source: `prepare_input` in [event_processor.rs][engine], [subscription tool][cancel], and [message conversion][messages]. The inspected preparation path checks that the originating tool call exists; it is not a general gate requiring current active-subscription membership. Hosts and tools must not treat the tracking set alone as replay protection.

The coding sandbox provides a concrete implementation. It acknowledges the RAP invocation and spawns processing. For `execute_command`, it internally waits up to five seconds for short commands. A longer command yields a subscription result and then associative output events, including a final event. The HTTP acknowledgement has already happened; the five seconds is not the acknowledgement timeout. Source: [sandbox server][sandbox] and its `execute_command` branch.

## Identity and redelivery

`prepare_callback` assigns stable IDs for one-off callbacks, such as `rap-tool-result-<id>`. In contrast, each received `subscription_event` gets a new UUID because the wire format has no per-event identity. Two identical payloads may be two legitimate events; the bridge intentionally preserves both. Its test is named `repeated_subscription_events_are_not_deduplicated`. Source: [bridge][bridge].

Within the core, an already-answered tool result is skipped using history, and selected inputs use processed-message IDs. But the non-associative subscription path creates a child and returns **before** the ordinary `handle_content` deduplication path. Therefore “all callback redelivery is harmless” is not supported even if a caller tries to reuse one dedup ID for every delivery. Sources: `prepare_input`, `HistoryManager::handle_content` in [engine][engine].

**Inference for tool authors:** if events represent non-repeatable work, use an explicit application-level event identity/ledger at an appropriate boundary. The current wire protocol and generic adapter do not provide end-to-end exactly-once event processing. This guide does not implement or propose an integration layer.

## Specification versus the clients actually included

| Topic | Specification/intent | Inspected behavior |
| --- | --- | --- |
| Invocation success | Immediate HTTP 200 acknowledgement | Any 2xx accepted by `invoke_rap_tool` |
| Transient delivery failure | Exponential-backoff retries recommended | No invocation retry loop here; non-2xx becomes an error result, transport error returns `Err` |
| Callback delivery | Retry transient failures, reject permanent errors | `PlainCallbackClient` sends once and does not reject non-success HTTP status |
| Closure/cancellation notifications | Best-effort lifecycle signals; spec describes nonblocking dispatch | `RapNotifier` awaits each server sequentially; logs returned status without treating non-2xx as an error |
| Arguments | JSON Schema published in manifest; tools must validate | Generic dispatch passes JSON through; validation is the tool's responsibility, not a generic schema-validator step |
| Callback security | Authentication/replay protection recommended | Local callback listener binds loopback without auth; AWS receiver uses a Function URL configured with IAM and signed clients |

Sources: [transport spec][transport], [closure spec][closure], [invocation][invoke], `PlainCallbackClient` in [protocol][types], [notifier][notifier], [local callback server][listener], [CDK construct][cdk]. These distinctions describe this revision, not every possible RAP implementation. Local loopback binding is a trust boundary, not a reason to expose an unauthenticated listener to a public network. The local router also fails open on store errors during existence/stopped admission checks, logging warnings; it is not an authentication boundary. [Router][router].

## What MCP compatibility means here

`McpToolSet` exposes adapter tools for listing and invoking tools on a lazily connected stdio or Streamable HTTP MCP server. `McpClient` holds its session behind an async mutex, serializing session requests. Invoking a tool spawns work and eventually delivers an Infinity input, so the *agent* yields, but the adapter still waits for the underlying MCP response. The daemon's RAP-facing MCP proxy provides another hosting surface for the same client.

The bridge normalizes MCP text and emits text markers for images/resources rather than promising full fidelity for every MCP content or capability. This is useful tool reuse, not evidence of blanket MCP feature parity, serverless execution of the MCP server itself, or native RAP subscriptions for every MCP tool. Sources: [MCP bridge][mcp], [daemon proxy][proxy].

[types]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/rap-protocol/src/lib.rs
[spec]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/docs/docs/rap/spec/overview.md
[transport]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/docs/docs/rap/spec/basic/transport.md
[loader]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/rap-client/src/toolset_loader.rs
[invoke]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/tools/rap_tool.rs
[bridge]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-rap-bridge/src/lib.rs
[conversion]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-rap-bridge/src/callback.rs
[thread]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/thread.rs
[daemon-callback]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-daemon/src/rap_callback.rs
[receiver]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-lambda/src/bin/rap-receiver.rs
[engine]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/event_processor.rs
[cancel]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/tools/cancel_subscription.rs
[messages]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/message.rs
[sandbox]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/sandbox-core/src/server.rs
[closure]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/docs/docs/rap/spec/basic/thread-closure.md
[notifier]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/rap-client/src/notifier.rs
[listener]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/rap-client/src/callback_server.rs
[cdk]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/agent/lib/infinity-agents/index.ts
[router]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/local/router.rs
[mcp]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-mcp-bridge/src/lib.rs
[proxy]: https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-daemon/src/mcp_proxy.rs
