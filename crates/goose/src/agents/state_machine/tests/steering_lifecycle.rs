use anyhow::Result;
use tokio_util::sync::CancellationToken;

use super::pipeline::{self, test_pipeline, MessageKind::Agent};
use crate::conversation::message::Message;

const SUMMARIZE_HISTORY: &str = "distill the conversation so far";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steering_is_fifo_during_inference_and_survives_compaction() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    let first_response = api.on("paint it").hold_reply("starting");
    api.on("then make it matte").reply("followed both changes");

    let run = pipeline.run(["paint it"]);
    let steer = async {
        first_response.entered().await;
        pipeline
            .steer(Message::user().with_text("first use blue"))
            .await;
        pipeline
            .steer(Message::user().with_text("then make it matte"))
            .await;
        first_response.release();
    };
    let (result, ()) = tokio::join!(run, steer);
    let result = result?;

    result.assert_message(-1, Agent, "followed both changes");
    let messages = result.conversation().messages();
    let first = messages
        .iter()
        .position(|message| message.as_concat_text() == "first use blue")
        .expect("first steer was persisted");
    let second = messages
        .iter()
        .position(|message| message.as_concat_text() == "then make it matte")
        .expect("second steer was persisted");
    assert!(first < second);
    assert!(messages[first].metadata.steer);
    assert!(messages[second].metadata.steer);
    assert!(!pipeline.has_pending_steers().await);
    let calls = api.calls();
    assert!(calls[1].input_contains("first use blue"));
    assert!(calls[1].input_contains("then make it matte"));

    let large_response = "x"
        .repeat((pipeline.context_limit() as f64 * pipeline::COMPACTION_THRESHOLD * 1.01) as usize);
    let held_response = api.on("continue near the limit").hold_reply(large_response);
    api.on(SUMMARIZE_HISTORY).reply("steered work summarized");
    api.on("Your context was compacted")
        .reply("continued after steering compaction");

    let run = pipeline.run(["continue near the limit"]);
    let steer = async {
        held_response.entered().await;
        pipeline
            .steer(Message::user().with_text("redirect before compaction"))
            .await;
        held_response.release();
    };
    let (result, ()) = tokio::join!(run, steer);
    let result = result?;

    result.assert_message(-1, Agent, "continued after steering compaction");
    assert_eq!(result.history_replacements(), 1);
    assert!(!pipeline.has_pending_steers().await);
    let summarization = api
        .calls()
        .into_iter()
        .find(|call| call.input_contains(SUMMARIZE_HISTORY))
        .expect("compaction request");
    assert!(summarization.input_contains("redirect before compaction"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_preserves_queued_steering_for_resume() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    let held_response = api
        .on("cancel current work")
        .hold_reply("current work finished");
    api.on("use the queued direction")
        .reply("queued direction applied");
    let cancel = CancellationToken::new();

    let run = pipeline.run_with_cancel("cancel current work", cancel.clone());
    let steer = async {
        held_response.entered().await;
        pipeline
            .steer(Message::user().with_text("use the queued direction"))
            .await;
        cancel.cancel();
        held_response.release();
    };
    let (cancelled, ()) = tokio::join!(run, steer);
    let cancelled = cancelled?;
    assert!(pipeline.has_pending_steers().await);
    assert!(!cancelled
        .conversation()
        .messages()
        .iter()
        .any(|message| message.as_concat_text() == "use the queued direction"));

    let resumed = pipeline.resume().await?;
    resumed.assert_message(-1, Agent, "queued direction applied");
    assert!(!pipeline.has_pending_steers().await);
    let steers = resumed
        .conversation()
        .messages()
        .iter()
        .filter(|message| message.as_concat_text() == "use the queued direction")
        .collect::<Vec<_>>();
    assert_eq!(steers.len(), 1);
    assert!(steers[0].metadata.steer);
    assert!(api
        .calls()
        .last()
        .expect("resumed inference request")
        .input_contains("use the queued direction"));

    Ok(())
}
