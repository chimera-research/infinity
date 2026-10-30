# Learning Hydro Infinity from its source

Infinity is an **event-driven Rust agent runtime**, a **callback-based tool protocol (RAP)**, and an application built on them, **Infinity Code**. Its central technique is to turn an agent's continuation into conversation data plus incoming messages, rather than keep one uninterrupted tool-calling loop alive forever.

This is an independent source-reading guide prepared for Chimera Research. It is about [hydro-project/infinity](https://github.com/hydro-project/infinity), **not the unrelated embeddings-serving project also named Infinity**. It is not an integration proposal or a replacement for the upstream documentation.

## Revision and attribution

- Inspected upstream `main` at [`1abc6fcff1f186124f924b56aa92c3a3684c90c5`](https://github.com/hydro-project/infinity/commit/1abc6fcff1f186124f924b56aa92c3a3684c90c5), committed **2026-10-27**; exploration performed **2026-10-30**.
- Source citations below are pinned to that revision. Symbol names in the walkthroughs refer to that source, not to an assumed future API.
- Infinity remains the work of **Hydro Contributors**. The upstream [NOTICE](../../NOTICE) also credits Amazon Web Services as the initial developer of some portions. [Apache-2.0 license](../../LICENSE.txt), NOTICE, and [third-party notices](../../THIRD-PARTY) are unchanged. These additional learning materials use the repository's Apache-2.0 license.
- The only intended additions are this guide, a link from the root README, and a credential-free learning example. Runtime behavior is unchanged.

## Reading routes

| What you want to learn | Read |
| --- | --- |
| What the system does, what an agent/thread means, and where it is useful | [1. Conceptual model and features](concepts.md) |
| How the crates fit together and which layer owns each responsibility | [2. Architecture and implementation choices](architecture.md) |
| What really runs concurrently, what survives, and what can fail | [3. Concurrency, lifecycle, and persistence](concurrency-and-lifecycle.md) |
| How RAP works on the wire, including subscriptions and MCP adaptation | [4. Reactive Agent Protocol](rap.md) |
| Where to read the implementation, in execution order | [5. Source walkthroughs](walkthroughs.md) |
| How to learn by running local examples without credentials, then try a real model | [6. Getting started and exercises](getting-started.md) |
| What was actually tested, what remains unverified, and delivery limitations | [7. Evidence and limitations](evidence.md) |

**First pass:** concepts, the lifecycle state table, then the no-credential example. **Second pass:** architecture, RAP, and the source walkthroughs with an editor open. **Systems review:** read the durability and protocol caveats before interpreting the marketing claims.

## How to read claims in this guide

- **Implemented** means a behavior is present in the inspected source. It does not imply that every deployment was exercised.
- **Validated locally** means an executed check supported the statement; commands and results appear in [the evidence record](evidence.md).
- **Upstream claim/specification** means documentation describes an intention, requirement, or measurement. It is not automatically a property of every adapter.
- **Inference** means a consequence reasoned from source, rather than an observed production failure or a demonstrated benchmark.

The distinction matters. The source really does release idle local drivers and supports serverless steps. It does **not** imply zero total resource use, exactly-once external side effects, or parallel execution of every tool call in one model response.

## Three facts to keep in mind

1. **The concurrency unit is the conversation thread.** The runtime runs separate threads concurrently; one completion round dispatches at most one asynchronous tool call. Synchronous tools can loop back within a slice.
2. **A tool acknowledgement is not its result.** RAP acknowledges delivery over HTTP, then sends the result to a callback URL. A new input message resumes reasoning.
3. **Persistence is an adapter contract, not magic.** In-memory stores, the daemon's JSON files, and the Lambda deployment's databases have different failure properties. A persisted call and an external tool side effect are not one transaction.

Sources: [`Thread::step_no_defer`](https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/system/thread.rs), [`run_completion`](https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/crates/infinity-agent-core/src/event_processor.rs), and the [RAP transport specification](https://github.com/hydro-project/infinity/blob/1abc6fcff1f186124f924b56aa92c3a3684c90c5/docs/docs/rap/spec/basic/transport.md).

**Repository:** these independent learning artifacts live in [chimera-research/infinity](https://github.com/chimera-research/infinity), a verified fork of Hydro's upstream repository. The initial fork-creation permission blocker was resolved before publication. Source citations remain pinned to the inspected upstream revision; see [delivery details](evidence.md#delivery-state).
