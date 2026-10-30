//! A credential-free walkthrough of dispatch, yield, reload, and deduplication.
//!
//! Run: `cargo run --locked -p infinity-agent-core --example learning_slices`
//! Guide: `docs/exploration/getting-started.md`.
//!
//! The model and lookup are scripted. The runtime, stores, queue, and step API
//! are real. Rebuilding the system is NOT a process restart: the in-memory
//! stores survive only because this program retains their shared handles.

use std::sync::Arc;

use async_trait::async_trait;
use infinity_agent_core::ThreadId;
use infinity_agent_core::message::{InfinityMessage, InputMessage, InputMessageContent};
use infinity_agent_core::stores::{InMemoryConversationStore, InMemoryStateStore};
use infinity_agent_core::system::{
    AgentEvent, AgentSystemBuilder, EventCollector, NoDeferral, StaticModel, StepOutcome,
};
use infinity_agent_core::tools::{Tool, ToolContext};
use infinity_agent_core::traits::{ConversationStore, InputSender};
use infinity_provider_protocol::{
    CompletionError, CompletionModel, CompletionRequest, FinalResponse, Message, ModelEntry,
    ModelStream, SingleModelProvider, StreamChunk, ToolCall, ToolResult, ToolResultContent,
    UserContent,
};
use tokio::sync::mpsc;

type Error = Box<dyn std::error::Error + Send + Sync>;

// A step-mode host supplies its own delivery mechanism. This example uses a
// channel; a serverless host can implement the same trait with a durable queue.
#[derive(Clone)]
struct Queue(mpsc::UnboundedSender<(InputMessage, String)>);

#[async_trait]
impl InputSender for Queue {
    type Error = mpsc::error::SendError<(InputMessage, String)>;

    async fn send_to_input_queue(
        &self,
        message: InputMessage,
        dedup_id: &str,
    ) -> Result<(), Self::Error> {
        self.0.send((message, dedup_id.to_owned()))
    }
}

// First round requests a lookup; the round after its result emits a reply.
struct ScriptedModel;

#[async_trait]
impl CompletionModel for ScriptedModel {
    async fn stream(&self, request: CompletionRequest) -> Result<ModelStream, CompletionError> {
        let has_result = matches!(
            request.chat_history.last(),
            Some(Message::User { content })
                if content.iter().any(|item| matches!(item, UserContent::ToolResult(_)))
        );
        let answer = if has_result {
            StreamChunk::Text("Lookup returned coral.".to_owned())
        } else {
            StreamChunk::ToolCall(ToolCall::new("lookup-1", "lookup", serde_json::json!({})))
        };
        Ok(Box::pin(futures_util::stream::iter([
            Ok(answer),
            Ok(StreamChunk::Final(FinalResponse { usage: None })),
        ])))
    }
}

fn model() -> StaticModel {
    let entry = ModelEntry {
        model_id: "scripted".to_owned(),
        display_name: "Scripted learning model".to_owned(),
        context_window: 0,
        max_output_tokens: None,
        supports_image_input: false,
    };
    StaticModel::from_entry(
        Arc::new(SingleModelProvider::new(entry.clone(), ScriptedModel)),
        &entry,
    )
}

struct Lookup {
    store: InMemoryConversationStore,
}

#[async_trait]
impl Tool<Queue> for Lookup {
    fn name(&self) -> &str {
        "lookup"
    }

    fn description(&self) -> &str {
        "Return a scripted lookup result through the input queue."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn execute(
        &self,
        _args: serde_json::Value,
        id: rap_protocol::ToolCallId,
        call_id: Option<rap_protocol::ProviderCallId>,
        context: &ToolContext<Queue>,
    ) -> Result<(), Error> {
        let stored = self
            .store
            .load_history_up_to(&context.group_id, None, None)
            .await?;
        assert!(matches!(
            stored.last(),
            Some(InfinityMessage::ToolCall { call, .. }) if call.id == id.as_str()
        ));
        println!("dispatch: the tool call is already in the conversation store");

        // Simulate an external result. Step mode does not consume this queue
        // automatically: the host must deliver the result in a later slice.
        context
            .message_sender
            .send_to_input_queue(
                InputMessage {
                    content: InputMessageContent::User(UserContent::ToolResult(ToolResult {
                        id: id.as_str().to_owned(),
                        call_id: call_id.map(|value| value.into_inner()),
                        content: vec![ToolResultContent::text("coral")],
                    })),
                    group_id: context.group_id.clone(),
                    metadata: None,
                    synthetic: None,
                    display_as: None,
                    subscription: false,
                },
                id.as_str(),
            )
            .await?;
        Ok(())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Error> {
    let conversation = InMemoryConversationStore::new();
    let state = InMemoryStateStore::new();
    let (tx, mut queued) = mpsc::unbounded_channel();
    let sender = Queue(tx);
    let thread_id = ThreadId::from("learning-thread");
    let observer = EventCollector::new();

    // Each build creates an independent step-mode host over the same stores.
    let build = || {
        AgentSystemBuilder::new(conversation.clone(), state.clone(), model(), sender.clone())
            .without_builtin_tools()
            .tool(Box::new(Lookup {
                store: conversation.clone(),
            }))
            .build()
    };

    let mut system = build();
    system
        .step(
            vec![(
                InputMessage::user_text(thread_id.clone(), "Look it up"),
                "user-1".to_owned(),
            )],
            &observer,
            &mut NoDeferral,
        )
        .await?;
    let before_result = conversation
        .load_history_up_to(&thread_id, None, None)
        .await?;
    assert_eq!(before_result.len(), 2); // User text and unanswered tool call.
    println!("slice 1 ended: 2 stored messages, result still in host queue");
    drop(system);

    let callback = queued.try_recv()?;
    let mut reloaded = build();
    reloaded
        .step(vec![callback.clone()], &observer, &mut NoDeferral)
        .await?;
    let after_result = conversation
        .load_history_up_to(&thread_id, None, None)
        .await?;
    assert_eq!(after_result.len(), 4); // Plus tool result and assistant reply.
    assert!(observer.take().iter().any(|(_, event)| matches!(
        event, AgentEvent::TextChunk { text } if text == "Lookup returned coral."
    )));
    println!("slice 2 reloaded history: 4 stored messages, reply emitted");

    let replay = reloaded
        .step(vec![callback], &observer, &mut NoDeferral)
        .await?;
    assert!(matches!(replay.as_slice(), [(_, StepOutcome::Skipped)]));
    assert_eq!(
        conversation
            .load_history_up_to(&thread_id, None, None)
            .await?
            .len(),
        4
    );
    println!("duplicate result: skipped, still 4 stored messages");
    println!("PASS: commit-before-dispatch, reload, and duplicate-result handling");
    Ok(())
}
