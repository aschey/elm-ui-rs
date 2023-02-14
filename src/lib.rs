use futures::{stream::FuturesUnordered, StreamExt};
use std::{fmt::Debug, future::Future, pin::Pin};
use tokio::{
    runtime::Handle,
    sync::mpsc,
    task::{JoinError, JoinHandle},
};

pub enum Command<M> {
    Async(Pin<Box<dyn Future<Output = Option<M>> + Send + 'static>>),
    Blocking(Box<dyn FnOnce() -> Option<M> + Send + 'static>),
}

impl<M> Command<M> {
    pub fn new_async<F: Future<Output = Option<M>> + Send + 'static>(f: F) -> Self {
        Self::Async(Box::pin(async move { f.await }))
    }

    pub fn new_blocking(f: impl FnOnce() -> Option<M> + Send + 'static) -> Self {
        Self::Blocking(Box::new(f))
    }
}

pub enum Message<T: Debug + Send + 'static> {
    Batch(Vec<Command<Message<T>>>),
    Sequence(Vec<Command<Message<T>>>),
    #[cfg(all(not(feature = "termion"), feature = "crossterm"))]
    TermEvent(crossterm::event::Event),
    #[cfg(feature = "termion")]
    TermEvent(termion::event::Event),
    Quit,
    Custom(T),
}

impl<T: Debug + Send + 'static> Debug for Message<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Batch(_) => f.debug_tuple("Batch").field(&"[Commands]").finish(),
            Self::Sequence(_) => f.debug_tuple("Sequence").field(&"[Commands]").finish(),
            #[cfg(any(feature = "termion", feature = "crossterm"))]
            Self::TermEvent(arg0) => f.debug_tuple("TermEvent").field(arg0).finish(),
            Self::Quit => write!(f, "Quit"),
            Self::Custom(arg0) => f.debug_tuple("Custom").field(arg0).finish(),
        }
    }
}

pub type OptionalCommand<M> = Option<Command<Message<M>>>;

pub trait Model: Debug + 'static {
    type CustomMessage: Debug + Send + 'static;
    type Writer;
    type Error: std::error::Error + ToString;

    fn init(&self) -> Result<OptionalCommand<Self::CustomMessage>, Self::Error>;

    fn update(
        &mut self,
        msg: Message<Self::CustomMessage>,
    ) -> Result<OptionalCommand<Self::CustomMessage>, Self::Error>;

    fn view(&self, writer: &mut Self::Writer) -> Result<(), Self::Error>;
}

pub struct Program<M: Model> {
    model: M,
    cmd_tx: mpsc::Sender<Command<Message<<M as Model>::CustomMessage>>>,
    cmd_rx: Option<mpsc::Receiver<Command<Message<<M as Model>::CustomMessage>>>>,
    msg_tx: mpsc::Sender<Message<M::CustomMessage>>,
    msg_rx: mpsc::Receiver<Message<M::CustomMessage>>,
}

impl<M: Model> Program<M> {
    pub fn new(model: M) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command<Message<M::CustomMessage>>>(32);
        let (msg_tx, msg_rx) = mpsc::channel::<Message<M::CustomMessage>>(32);
        Self {
            model,
            cmd_tx,
            cmd_rx: Some(cmd_rx),
            msg_tx,
            msg_rx,
        }
    }

    pub async fn run(mut self, writer: &mut M::Writer) -> Result<(), ProgramError<M>> {
        self.initialize().await?;

        while let Some(msg) = self.recv_msg().await {
            let quit_behavior = self.update(msg).await?;
            self.view(writer)
                .map_err(ProgramError::ApplicationFailure)?;
            if quit_behavior == QuitBehavior::Quit {
                return Ok(());
            }
        }

        Ok(())
    }

    pub async fn recv_msg(&mut self) -> Option<Message<M::CustomMessage>> {
        self.msg_rx.recv().await
    }

    pub fn view(&self, writer: &mut M::Writer) -> Result<(), M::Error> {
        self.model.view(writer)
    }

    pub async fn initialize(&mut self) -> Result<(), ProgramError<M>> {
        #[cfg(any(feature = "termion", feature = "crossterm"))]
        self.spawn_event_reader();
        self.spawn_message_handler();

        if let Some(cmd) = self
            .model
            .init()
            .map_err(ProgramError::ApplicationFailure)?
        {
            self.cmd_tx.send(cmd).await.map_err(|e| {
                ProgramError::MessageFailure(MessageError::SendFailure(e.to_string()))
            })?;
        }

        Ok(())
    }

    pub async fn update(
        &mut self,
        msg: Message<<M>::CustomMessage>,
    ) -> Result<QuitBehavior, ProgramError<M>> {
        if QuitBehavior::Quit == self.handle_update(msg).await? {
            return Ok(QuitBehavior::Quit);
        }
        while let Ok(msg) = self.msg_rx.try_recv() {
            if QuitBehavior::Quit == self.handle_update(msg).await? {
                return Ok(QuitBehavior::Quit);
            }
        }

        Ok(QuitBehavior::Continue)
    }

    #[cfg(all(not(feature = "termion"), feature = "crossterm"))]
    fn spawn_event_reader(&self) -> tokio::task::JoinHandle<Result<(), MessageError>> {
        let msg_tx = self.msg_tx.clone();
        tokio::task::spawn(async move {
            let mut event_reader = crossterm::event::EventStream::new().fuse();

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

    #[cfg(feature = "termion")]
    fn spawn_event_reader(&self) -> tokio::task::JoinHandle<Result<(), MessageError>> {
        use termion::input::TermRead;

        let msg_tx = self.msg_tx.clone();
        tokio::task::spawn_blocking(move || {
            let stdin = std::io::stdin();
            for event in stdin.events().flatten() {
                msg_tx
                    .blocking_send(Message::TermEvent(event))
                    .map_err(|e| MessageError::SendFailure(e.to_string()))?;
            }

            Ok(())
        })
    }

    fn spawn_message_handler(&mut self) -> Option<JoinHandle<Result<(), MessageError>>> {
        if let Some(mut cmd_rx) = self.cmd_rx.take() {
            let msg_tx = self.msg_tx.clone();
            let cmd_tx = self.cmd_tx.clone();
            Some(tokio::task::spawn(async move {
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
            }))
        } else {
            None
        }
    }

    async fn handle_update(
        &mut self,
        msg: Message<M::CustomMessage>,
    ) -> Result<QuitBehavior, ProgramError<M>> {
        if let Message::Quit = msg {
            return Ok(QuitBehavior::Quit);
        }
        if let Some(cmd) = self
            .model
            .update(msg)
            .map_err(ProgramError::ApplicationFailure)?
        {
            self.cmd_tx.send(cmd).await.map_err(|e| {
                ProgramError::MessageFailure(MessageError::SendFailure(e.to_string()))
            })?;
        }
        Ok(QuitBehavior::Continue)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuitBehavior {
    Quit,
    Continue,
}

#[derive(thiserror::Error, Debug)]
pub enum MessageError {
    #[error("{0}")]
    SendFailure(String),
    #[error("{0}")]
    JoinFailure(JoinError),
}

#[derive(thiserror::Error, Debug)]
pub enum ProgramError<M: Model> {
    #[error("{0}")]
    MessageFailure(MessageError),
    #[error("{0}")]
    ApplicationFailure(M::Error),
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
    msg: Option<Message<M::CustomMessage>>,
    msg_tx: mpsc::Sender<Message<M::CustomMessage>>,
    cmd_tx: mpsc::Sender<Command<Message<M::CustomMessage>>>,
) -> Result<(), MessageError> {
    let mut futs = FuturesUnordered::<JoinHandle<Result<(), MessageError>>>::default();

    match msg {
        Some(Message::Batch(cmds)) => {
            for cmd in cmds {
                cmd_tx
                    .send(cmd)
                    .await
                    .map_err(|e| MessageError::SendFailure(e.to_string()))?;
            }
        }
        Some(Message::Sequence(cmds)) => {
            let msg_tx = msg_tx.clone();
            futs.push(tokio::task::spawn(async move {
                handle_sequence_cmd::<M>(cmds, msg_tx).await
            }));
        }
        Some(msg) => {
            msg_tx
                .send(msg)
                .await
                .map_err(|e| MessageError::SendFailure(e.to_string()))?;
        }
        None => {}
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
                if let Some(msg) = cmd.await {
                    msg_tx
                        .send(msg)
                        .await
                        .map_err(|e| MessageError::SendFailure(e.to_string()))?;
                }
            }
            Command::Blocking(cmd) => {
                let msg_tx = msg_tx.clone();
                let handle: JoinHandle<Result<(), MessageError>> =
                    tokio::task::spawn_blocking(move || {
                        if let Some(msg) = cmd() {
                            msg_tx
                                .blocking_send(msg)
                                .map_err(|e| MessageError::SendFailure(e.to_string()))?;
                        }

                        Ok(())
                    });
                handle.await.map_err(MessageError::JoinFailure)??;
            }
        }
    }
    Ok(())
}
