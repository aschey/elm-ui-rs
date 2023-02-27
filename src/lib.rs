pub mod future_ext;

use async_recursion::async_recursion;
use futures::{stream::FuturesUnordered, Stream, StreamExt};
use std::{
    any::Any,
    collections::HashMap,
    fmt::Debug,
    future::{self, Future},
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::{
    runtime::Handle,
    sync::mpsc,
    task::{self, JoinError, JoinHandle},
};
use tokio_util::sync::CancellationToken;

pub type AsyncCommand = dyn FnOnce(
        mpsc::Sender<Command>,
        CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Option<Message>> + Send>>
    + Send;
pub type BlockingCommand =
    dyn FnOnce(mpsc::Sender<Command>, CancellationToken) -> Option<Message> + Send;

pub enum CommandFn {
    Async(Box<AsyncCommand>),
    Blocking(Box<BlockingCommand>),
}

impl Debug for CommandFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Async(_) => f.debug_tuple("Async").field(&"Fn").finish(),
            Self::Blocking(_) => f.debug_tuple("Blocking").field(&"Fn").finish(),
        }
    }
}

#[derive(Debug)]
pub struct Command {
    name: String,
    func: CommandFn,
}

impl Command {
    pub fn new_async<F: Future<Output = Option<Message>> + Send + 'static>(
        f: impl FnOnce(mpsc::Sender<Command>, CancellationToken) -> F + Send + 'static,
    ) -> Self {
        Self {
            name: "".to_owned(),
            func: CommandFn::Async(Box::new(|sender, cancellation_token| {
                Box::pin(async move { f(sender, cancellation_token).await })
            })),
        }
    }

    pub fn new_blocking(
        f: impl FnOnce(mpsc::Sender<Command>, CancellationToken) -> Option<Message> + Send + 'static,
    ) -> Self {
        Self {
            name: "".to_owned(),
            func: CommandFn::Blocking(Box::new(f)),
        }
    }

    pub fn simple(msg: Message) -> Self {
        Self::new_async(|_, _| future::ready(Some(msg)))
    }

    pub fn quit() -> Self {
        Self::simple(Message::Quit)
    }

    pub fn with_name(self, name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            func: self.func,
        }
    }
}

pub enum Message {
    Batch(Vec<Command>),
    Sequence(Vec<Command>),
    Stream(Pin<Box<dyn Stream<Item = Message> + Send>>),
    #[cfg(all(not(feature = "termion"), feature = "crossterm"))]
    TermEvent(crossterm::event::Event),
    #[cfg(feature = "termion")]
    TermEvent(termion::event::Event),
    Quit,
    CancelAll,
    Cancel(String),
    CancellationComplete(Option<String>),
    Custom(Box<dyn Any + Send>),
}

impl Debug for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Batch(arg0) => f.debug_tuple("Batch").field(arg0).finish(),
            Self::Sequence(arg0) => f.debug_tuple("Sequence").field(arg0).finish(),
            Self::Stream(_) => f.debug_tuple("Stream").field(&"<stream>").finish(),
            #[cfg(any(feature = "termion", feature = "crossterm"))]
            Self::TermEvent(arg0) => f.debug_tuple("TermEvent").field(arg0).finish(),
            Self::Quit => write!(f, "Quit"),
            Self::CancelAll => write!(f, "CancelAll"),
            Self::Cancel(arg0) => f.debug_tuple("Cancel").field(arg0).finish(),
            Self::CancellationComplete(arg0) => {
                f.debug_tuple("CancellationComplete").field(arg0).finish()
            }
            Self::Custom(arg0) => f.debug_tuple("Custom").field(arg0).finish(),
        }
    }
}

impl Message {
    pub fn custom(msg: impl Any + Send) -> Self {
        Self::Custom(Box::new(msg))
    }
}

pub type OptionalCommand = Option<Command>;

pub trait Model {
    type Writer;
    type Error: std::error::Error + ToString;

    fn init(&mut self) -> Result<OptionalCommand, Self::Error>;
    fn update(&mut self, msg: Arc<Message>) -> Result<OptionalCommand, Self::Error>;
    fn view(&self, writer: &mut Self::Writer) -> Result<(), Self::Error>;
}

pub struct Program<M: Model> {
    model: M,
    cmd_tx: mpsc::Sender<Command>,
    cmd_rx: Option<mpsc::Receiver<Command>>,
    msg_tx: mpsc::Sender<Message>,
    msg_rx: mpsc::Receiver<Message>,
    cancellation_tokens: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl<M: Model> Program<M> {
    pub fn new(model: M) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(32);
        let (msg_tx, msg_rx) = mpsc::channel::<Message>(32);
        Self {
            model,
            cmd_tx,
            cmd_rx: Some(cmd_rx),
            msg_tx,
            msg_rx,
            cancellation_tokens: Default::default(),
        }
    }

    pub async fn run(mut self, writer: &mut M::Writer) -> Result<(), ProgramError<M>> {
        self.initialize().await?;
        self.view(writer)
            .map_err(ProgramError::ApplicationFailure)?;
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

    pub fn cmd_tx(&self) -> mpsc::Sender<Command> {
        self.cmd_tx.clone()
    }

    pub async fn recv_msg(&mut self) -> Option<Message> {
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

    pub async fn update(&mut self, msg: Message) -> Result<QuitBehavior, ProgramError<M>> {
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
            let cancellation_tokens = self.cancellation_tokens.clone();
            Some(tokio::task::spawn(async move {
                let mut futs = FuturesUnordered::<JoinHandle<Result<(), MessageError>>>::default();
                loop {
                    tokio::select! {
                        Some(cmd) = cmd_rx.recv() => {
                            {
                                let mut cancellation_tokens = cancellation_tokens.lock().unwrap();
                                if !cancellation_tokens.contains_key(&cmd.name) {
                                    cancellation_tokens.insert(cmd.name.clone(), CancellationToken::new());
                                }
                            }
                            handle_cmd::<M>(
                                cmd,
                                msg_tx.clone(),
                                cmd_tx.clone(),
                                &mut futs,
                                cancellation_tokens.clone(),
                            )?;
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

    async fn handle_update(&mut self, msg: Message) -> Result<QuitBehavior, ProgramError<M>> {
        if let Message::Quit = msg {
            return Ok(QuitBehavior::Quit);
        }
        if let Some(cmd) = self
            .model
            .update(Arc::new(msg))
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
    cmd: Command,
    msg_tx: mpsc::Sender<Message>,
    cmd_tx: mpsc::Sender<Command>,
    futs: &mut FuturesUnordered<JoinHandle<Result<(), MessageError>>>,
    cancellation_tokens: Arc<Mutex<HashMap<String, CancellationToken>>>,
) -> Result<(), MessageError> {
    let cancellation_token = cancellation_tokens
        .lock()
        .unwrap()
        .get(&cmd.name)
        .unwrap()
        .clone();
    match cmd.func {
        CommandFn::Async(cmd) => {
            futs.push(tokio::task::spawn(async move {
                let msg = cmd(cmd_tx.clone(), cancellation_token).await;
                handle_msg::<M>(msg, msg_tx, cmd_tx, cancellation_tokens).await
            }));
        }
        CommandFn::Blocking(cmd) => {
            futs.push(tokio::task::spawn_blocking(move || {
                let msg = cmd(cmd_tx.clone(), cancellation_token);
                let handle: JoinHandle<Result<(), MessageError>> = tokio::task::spawn(async move {
                    handle_msg::<M>(msg, msg_tx, cmd_tx, cancellation_tokens).await?;
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

#[async_recursion]
async fn handle_msg<M: Model>(
    msg: Option<Message>,
    msg_tx: mpsc::Sender<Message>,
    cmd_tx: mpsc::Sender<Command>,
    cancellation_tokens: Arc<Mutex<HashMap<String, CancellationToken>>>,
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
            let cmd_tx = cmd_tx.clone();
            futs.push(tokio::task::spawn(async move {
                handle_sequence_cmd::<M>(cmds, cmd_tx, msg_tx, cancellation_tokens).await
            }));
        }
        Some(Message::Stream(mut rx)) => {
            let msg_tx = msg_tx.clone();
            let cmd_tx = cmd_tx.clone();
            futs.push(task::spawn(async move {
                while let Some(msg) = rx.next().await {
                    let res = handle_msg::<M>(
                        Some(msg),
                        msg_tx.clone(),
                        cmd_tx.clone(),
                        cancellation_tokens.clone(),
                    )
                    .await;
                    res?;
                }
                Ok(())
            }));
        }
        Some(Message::CancelAll) => {
            for token in cancellation_tokens.lock().unwrap().values() {
                token.cancel();
            }
            msg_tx
                .send(Message::CancellationComplete(None))
                .await
                .map_err(|e| MessageError::SendFailure(e.to_string()))?;
        }
        Some(Message::Cancel(name)) => {
            if let Some(token) = cancellation_tokens.lock().unwrap().get(&name) {
                token.cancel();
            }
            msg_tx
                .send(Message::CancellationComplete(Some(name)))
                .await
                .map_err(|e| MessageError::SendFailure(e.to_string()))?;
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
    cmds: Vec<Command>,
    cmd_tx: mpsc::Sender<Command>,
    msg_tx: mpsc::Sender<Message>,
    cancellation_tokens: Arc<Mutex<HashMap<String, CancellationToken>>>,
) -> Result<(), MessageError> {
    for command in cmds {
        let cancellation_token = cancellation_tokens
            .lock()
            .unwrap()
            .get(&command.name)
            .unwrap()
            .clone();
        match command.func {
            CommandFn::Async(cmd) => {
                if let Some(msg) = cmd(cmd_tx.clone(), cancellation_token).await {
                    msg_tx
                        .send(msg)
                        .await
                        .map_err(|e| MessageError::SendFailure(e.to_string()))?;
                }
            }
            CommandFn::Blocking(cmd) => {
                let msg_tx = msg_tx.clone();
                let cmd_tx = cmd_tx.clone();
                let handle: JoinHandle<Result<(), MessageError>> =
                    tokio::task::spawn_blocking(move || {
                        if let Some(msg) = cmd(cmd_tx, cancellation_token) {
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
