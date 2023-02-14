use std::{fmt::Debug, future::Future, io::Stdout, pin::Pin};

use crossterm::event::EventStream;
use futures::{stream::FuturesUnordered, StreamExt};
use tokio::{
    runtime::Handle,
    sync::mpsc,
    task::{JoinError, JoinHandle},
};
use tui::{backend::CrosstermBackend, Terminal};

pub enum Command<M> {
    Async(Pin<Box<dyn Future<Output = M> + Send + 'static>>),
    Blocking(Box<dyn FnOnce() -> M + Send + 'static>),
}

impl<M> Command<M> {
    pub fn new_async<F: Future<Output = M> + Send + 'static>(f: F) -> Self {
        Self::Async(Box::pin(async move { f.await }))
    }

    pub fn new_blocking(f: impl FnOnce() -> M + Send + 'static) -> Self {
        Self::Blocking(Box::new(f))
    }
}

pub enum Message<T: Debug + Send + 'static> {
    Batch(Vec<Command<Message<T>>>),
    Sequence(Vec<Command<Message<T>>>),
    TermEvent(crossterm::event::Event),
    Quit,
    Custom(T),
}

impl<T: Debug + Send + 'static> Debug for Message<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Batch(_) => f.debug_tuple("Batch").field(&"[Commands]").finish(),
            Self::Sequence(_) => f.debug_tuple("Sequence").field(&"[Commands]").finish(),
            Self::TermEvent(arg0) => f.debug_tuple("TermEvent").field(arg0).finish(),
            Self::Quit => write!(f, "Quit"),
            Self::Custom(arg0) => f.debug_tuple("Custom").field(arg0).finish(),
        }
    }
}

pub trait Model: 'static {
    type CustomMessage: Debug + Send + 'static;

    fn init(&self) -> Option<Command<Message<Self::CustomMessage>>>;

    fn update(
        &mut self,
        msg: Message<Self::CustomMessage>,
    ) -> Option<Command<Message<Self::CustomMessage>>>;

    fn view(&self, terminal: &mut Terminal<CrosstermBackend<Stdout>>);
}

pub async fn run<M: Model>(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mut model: M,
) -> Result<(), MessageError> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command<Message<M::CustomMessage>>>(32);
    let cmd_tx_ = cmd_tx.clone();
    let (msg_tx, mut msg_rx) = mpsc::channel::<Message<M::CustomMessage>>(32);
    spawn_event_reader::<M>(msg_tx.clone());
    spawn_message_handler::<M>(msg_tx, cmd_tx, cmd_rx);

    if let Some(cmd) = model.init() {
        cmd_tx_
            .send(cmd)
            .await
            .map_err(|e| MessageError::SendFailure(e.to_string()))?;
    }

    while let Some(msg) = msg_rx.recv().await {
        if let QuitBehavior::Quit = handle_update(&mut model, msg, &cmd_tx_).await? {
            return Ok(());
        }
        while let Ok(msg) = msg_rx.try_recv() {
            if let QuitBehavior::Quit = handle_update(&mut model, msg, &cmd_tx_).await? {
                return Ok(());
            }
        }
        model.view(terminal);
    }

    Ok(())
}

fn spawn_event_reader<M: Model>(
    msg_tx: mpsc::Sender<Message<<M as Model>::CustomMessage>>,
) -> tokio::task::JoinHandle<Result<(), MessageError>> {
    tokio::task::spawn(async move {
        let mut event_reader = EventStream::new().fuse();
        while let Some(event) = event_reader.next().await {
            if let Ok(event) = event {
                msg_tx
                    .send(Message::TermEvent(event))
                    .await
                    .map_err(|e| MessageError::SendFailure(e.to_string()))?;
            }
        }
        Ok(())
    })
}

enum QuitBehavior {
    Quit,
    Continue,
}

#[derive(thiserror::Error, Debug)]
pub enum MessageError {
    #[error("{0}")]
    SendFailure(String),
    #[error("{0}")]
    JoinFailure(#[source] JoinError),
}

async fn handle_update<M: Model>(
    model: &mut M,
    msg: Message<M::CustomMessage>,
    cmd_tx: &mpsc::Sender<Command<Message<M::CustomMessage>>>,
) -> Result<QuitBehavior, MessageError> {
    if let Message::Quit = msg {
        return Ok(QuitBehavior::Quit);
    }
    if let Some(cmd) = model.update(msg) {
        cmd_tx
            .send(cmd)
            .await
            .map_err(|e| MessageError::SendFailure(e.to_string()))?;
    }
    Ok(QuitBehavior::Continue)
}

fn spawn_message_handler<M: Model>(
    msg_tx: mpsc::Sender<Message<M::CustomMessage>>,
    cmd_tx: mpsc::Sender<Command<Message<M::CustomMessage>>>,
    mut cmd_rx: mpsc::Receiver<Command<Message<M::CustomMessage>>>,
) -> JoinHandle<Result<(), MessageError>> {
    tokio::task::spawn(async move {
        let mut futs = FuturesUnordered::<JoinHandle<Result<(), MessageError>>>::default();
        loop {
            tokio::select! {
                Some(cmd) = cmd_rx.recv() => {
                    handle_cmd::<M>(cmd, msg_tx.clone(), cmd_tx.clone(), &mut futs)?;
                },
                Some(fut) = futs.next() => {

                    fut.map_err(MessageError::JoinFailure)??;
                }
                else => break
            }
        }

        Ok(())
    })
}

fn handle_cmd<M: Model>(
    cmd: Command<Message<M::CustomMessage>>,
    msg_tx: mpsc::Sender<Message<M::CustomMessage>>,
    cmd_tx: mpsc::Sender<Command<Message<M::CustomMessage>>>,
    futs: &mut FuturesUnordered<JoinHandle<Result<(), MessageError>>>,
) -> Result<(), MessageError> {
    match cmd {
        Command::Async(cmd) => {
            futs.push(tokio::task::spawn(async move {
                let msg = cmd.await;
                handle_msg::<M>(msg, msg_tx, cmd_tx).await
            }));
        }
        Command::Blocking(cmd) => {
            futs.push(tokio::task::spawn_blocking(move || {
                let msg = cmd();

                let handle: JoinHandle<Result<(), MessageError>> = tokio::task::spawn(async move {
                    handle_msg::<M>(msg, msg_tx, cmd_tx).await?;
                    Ok(())
                });

                Handle::current()
                    .block_on(handle)
                    .map_err(MessageError::JoinFailure)??;
                Ok(())
            }));
        }
    }
    Ok(())
}

async fn handle_msg<M: Model>(
    msg: Message<M::CustomMessage>,
    msg_tx: mpsc::Sender<Message<M::CustomMessage>>,
    cmd_tx: mpsc::Sender<Command<Message<M::CustomMessage>>>,
) -> Result<(), MessageError> {
    let mut futs = FuturesUnordered::<JoinHandle<Result<(), MessageError>>>::default();

    match msg {
        Message::Batch(cmds) => {
            for cmd in cmds {
                cmd_tx
                    .send(cmd)
                    .await
                    .map_err(|e| MessageError::SendFailure(e.to_string()))?;
            }
        }
        Message::Sequence(cmds) => {
            let msg_tx = msg_tx.clone();
            futs.push(tokio::task::spawn(async move {
                handle_sequence_cmd::<M>(cmds, msg_tx).await
            }));
        }
        msg => {
            msg_tx
                .send(msg)
                .await
                .map_err(|e| MessageError::SendFailure(e.to_string()))?;
        }
    }

    while let Some(fut) = futs.next().await {
        fut.map_err(MessageError::JoinFailure)??
    }

    Ok(())
}

async fn handle_sequence_cmd<M: Model>(
    cmds: Vec<Command<Message<<M as Model>::CustomMessage>>>,
    msg_tx: mpsc::Sender<Message<M::CustomMessage>>,
) -> Result<(), MessageError> {
    for command in cmds {
        match command {
            Command::Async(cmd) => {
                msg_tx
                    .send(cmd.await)
                    .await
                    .map_err(|e| MessageError::SendFailure(e.to_string()))?;
            }
            Command::Blocking(cmd) => {
                let msg_tx = msg_tx.clone();
                let handle: JoinHandle<Result<(), MessageError>> =
                    tokio::task::spawn_blocking(move || {
                        msg_tx
                            .blocking_send(cmd())
                            .map_err(|e| MessageError::SendFailure(e.to_string()))?;

                        Ok(())
                    });
                handle.await.map_err(MessageError::JoinFailure)??;
            }
        }
    }
    Ok(())
}
