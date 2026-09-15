//! Runtime primitives for provider-specific live sessions.
//!
//! A single actor owns transport I/O and lifecycle transitions so sends,
//! shutdown, and incoming events have deterministic ordering.

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::{
    sync::{broadcast, mpsc, oneshot, watch},
    time::{timeout, Duration},
};

pub(crate) const LIVE_EVENT_CHANNEL_CAPACITY: usize = 256;
const TRANSPORT_SEND_TIMEOUT: Duration = Duration::from_secs(5);
const TRANSPORT_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_ACKNOWLEDGEMENT_TIMEOUT: Duration = Duration::from_secs(12);

#[async_trait]
pub trait LiveTransport: Send + Sync {
    async fn send(&self, message: Value) -> Result<()>;
    async fn receive(&self) -> Result<Option<Value>>;
    async fn close(&self) -> Result<()>;
}

pub trait LiveProtocol: Send + Sync + 'static {
    type Command: Send + 'static;
    type Event: Clone + Send + 'static;

    fn encode(&self, command: Self::Command) -> Result<Value>;
    fn decode(&self, message: Value) -> Result<Self::Event>;
    fn close_command(&self) -> Option<Self::Command>;
    fn is_close_acknowledgement(&self, event: &Self::Event) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveSessionEndReason {
    Closed,
    CloseTimedOut,
    TransportClosed,
    TransportFailed,
    ProtocolFailed,
    DecodeFailed,
}

#[derive(Debug, Clone)]
struct LiveSessionOutcome {
    reason: LiveSessionEndReason,
    error: Option<Arc<anyhow::Error>>,
}

impl LiveSessionOutcome {
    fn result(&self) -> Result<()> {
        match &self.error {
            Some(error) => Err(anyhow!(error.to_string())),
            None if self.reason == LiveSessionEndReason::Closed => Ok(()),
            None => Err(anyhow!("live session ended ({:?})", self.reason)),
        }
    }
}

#[derive(Debug, Clone)]
pub enum LiveSessionEvent<E> {
    Message(E),
    Ended {
        reason: LiveSessionEndReason,
        error: Option<Arc<anyhow::Error>>,
    },
}

impl<E> LiveSessionEvent<E> {
    fn ended(reason: LiveSessionEndReason, error: Option<anyhow::Error>) -> Self {
        Self::Ended {
            reason,
            error: error.map(Arc::new),
        }
    }
}

enum ActorCommand<C> {
    Send {
        command: C,
        result: oneshot::Sender<Result<()>>,
    },
    Close {
        result: oneshot::Sender<Result<()>>,
    },
}

pub struct LiveSession<P: LiveProtocol> {
    commands: mpsc::Sender<ActorCommand<P::Command>>,
    events: broadcast::Sender<LiveSessionEvent<P::Event>>,
    ended: watch::Receiver<Option<LiveSessionOutcome>>,
}

impl<P: LiveProtocol> Clone for LiveSession<P> {
    fn clone(&self) -> Self {
        Self {
            commands: self.commands.clone(),
            events: self.events.clone(),
            ended: self.ended.clone(),
        }
    }
}

impl<P: LiveProtocol> LiveSession<P> {
    pub fn connect(
        protocol: Arc<P>,
        transport: Arc<dyn LiveTransport>,
    ) -> (Self, broadcast::Receiver<LiveSessionEvent<P::Event>>) {
        let (commands, command_rx) = mpsc::channel(256);
        let (events, initial_events) = broadcast::channel(LIVE_EVENT_CHANNEL_CAPACITY);
        let (ended_tx, ended) = watch::channel(None);
        tokio::spawn(run_actor(
            protocol,
            transport,
            command_rx,
            events.clone(),
            ended_tx,
        ));
        (
            Self {
                commands,
                events,
                ended,
            },
            initial_events,
        )
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LiveSessionEvent<P::Event>> {
        self.events.subscribe()
    }

    pub fn end_reason(&self) -> Option<LiveSessionEndReason> {
        self.ended.borrow().as_ref().map(|outcome| outcome.reason)
    }

    pub async fn send(&self, command: P::Command) -> Result<()> {
        if let Some(reason) = self.end_reason() {
            bail!("live session ended ({reason:?})");
        }
        let (result_tx, result_rx) = oneshot::channel();
        self.commands
            .send(ActorCommand::Send {
                command,
                result: result_tx,
            })
            .await
            .map_err(|_| anyhow!("live session ended"))?;
        result_rx.await.map_err(|_| anyhow!("live session ended"))?
    }

    pub async fn close(&self) -> Result<()> {
        let outcome = self.ended.borrow().clone();
        if let Some(outcome) = outcome {
            return outcome.result();
        }
        let (result_tx, result_rx) = oneshot::channel();
        if self
            .commands
            .send(ActorCommand::Close { result: result_tx })
            .await
            .is_err()
        {
            return self.wait_for_end().await;
        }
        match result_rx.await {
            Ok(result) => result,
            Err(_) => self.wait_for_end().await,
        }
    }

    async fn wait_for_end(&self) -> Result<()> {
        let mut ended = self.ended.clone();
        loop {
            let outcome = ended.borrow().clone();
            if let Some(outcome) = outcome {
                return outcome.result();
            }
            ended
                .changed()
                .await
                .map_err(|_| anyhow!("live session ended without an outcome"))?;
        }
    }
}

async fn run_actor<P: LiveProtocol>(
    protocol: Arc<P>,
    transport: Arc<dyn LiveTransport>,
    mut commands: mpsc::Receiver<ActorCommand<P::Command>>,
    events: broadcast::Sender<LiveSessionEvent<P::Event>>,
    ended: watch::Sender<Option<LiveSessionOutcome>>,
) {
    let mut close_waiters = Vec::new();
    let mut closing = false;
    let close_timeout = tokio::time::sleep(CLOSE_ACKNOWLEDGEMENT_TIMEOUT);
    tokio::pin!(close_timeout);

    let (reason, error) = loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(ActorCommand::Send { command, result }) if !closing => {
                    let message = match protocol.encode(command) {
                        Ok(message) => message,
                        Err(error) => {
                            let _ = result.send(Err(error));
                            continue;
                        }
                    };
                    let send_result = send_transport(&transport, message).await;
                    let failed = send_result.is_err();
                    let error = send_result.as_ref().err().map(|error| anyhow!(error.to_string()));
                    let _ = result.send(send_result);
                    if failed {
                        break (LiveSessionEndReason::TransportFailed, error);
                    }
                }
                Some(ActorCommand::Send { result, .. }) => {
                    let _ = result.send(Err(anyhow!("live session is closing")));
                }
                Some(ActorCommand::Close { result }) => {
                    close_waiters.push(result);
                    if !closing {
                        closing = true;
                        if let Some(command) = protocol.close_command() {
                            let close_result = match protocol.encode(command) {
                                Ok(message) => send_transport(&transport, message).await,
                                Err(error) => {
                                    break (LiveSessionEndReason::ProtocolFailed, Some(error));
                                }
                            };
                            if let Err(error) = close_result {
                                break (LiveSessionEndReason::TransportFailed, Some(error));
                            }
                            close_timeout.as_mut().reset(
                                tokio::time::Instant::now() + CLOSE_ACKNOWLEDGEMENT_TIMEOUT,
                            );
                        } else {
                            break (LiveSessionEndReason::Closed, None);
                        }
                    }
                }
                None => break (LiveSessionEndReason::Closed, None),
            },
            incoming = transport.receive() => match incoming {
                Ok(Some(message)) => match protocol.decode(message) {
                    Ok(event) => {
                        let acknowledged = protocol.is_close_acknowledgement(&event);
                        let _ = events.send(LiveSessionEvent::Message(event));
                        if acknowledged {
                            break (LiveSessionEndReason::Closed, None);
                        }
                    }
                    Err(error) => break (LiveSessionEndReason::DecodeFailed, Some(error)),
                },
                Ok(None) => break (LiveSessionEndReason::TransportClosed, None),
                Err(error) => break (LiveSessionEndReason::TransportFailed, Some(error)),
            },
            _ = &mut close_timeout, if closing => {
                break (
                    LiveSessionEndReason::CloseTimedOut,
                    Some(anyhow!("live session close acknowledgement timed out")),
                );
            },
        }
    };

    let close_result = timeout(TRANSPORT_CLOSE_TIMEOUT, transport.close()).await;
    let close_error = match close_result {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(_) => Some(anyhow!("live transport close timed out")),
    };
    let final_error = error.or(close_error);
    let outcome = LiveSessionOutcome {
        reason,
        error: final_error
            .as_ref()
            .map(|error| Arc::new(anyhow!(error.to_string()))),
    };
    ended.send_replace(Some(outcome.clone()));
    let _ = events.send(LiveSessionEvent::ended(
        reason,
        final_error.as_ref().map(|error| anyhow!(error.to_string())),
    ));
    for waiter in close_waiters {
        let _ = waiter.send(outcome.result());
    }
}

async fn send_transport(transport: &Arc<dyn LiveTransport>, message: Value) -> Result<()> {
    timeout(TRANSPORT_SEND_TIMEOUT, transport.send(message))
        .await
        .map_err(|_| anyhow!("live transport send timed out"))?
}
