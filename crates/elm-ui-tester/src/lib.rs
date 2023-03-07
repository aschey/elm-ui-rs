use elm_ui::{
    future_ext::{CancelledByShutdown, FutureExt},
    Command, Message, Model, Program, ProgramError, QuitBehavior,
};
use std::{
    sync::{Arc, RwLock},
    thread,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
#[cfg(feature = "tui")]
use tui::{backend::TestBackend, buffer::Buffer, layout::Rect, Terminal};

pub struct UiTester<M: Model + Send + 'static, O: Clone + Send + Sync + 'static>
where
    M::Writer: Send + 'static,
{
    cmd_tx: mpsc::Sender<Command>,
    term_view: Arc<RwLock<O>>,
    handle: thread::JoinHandle<Result<Result<M, ProgramError<M>>, CancelledByShutdown>>,
    cancellation_token: CancellationToken,
}

#[cfg(feature = "tui")]
impl<M: Model + Send + 'static + Send + 'static> UiTester<M, Buffer>
where
    M: Model<Writer = Terminal<TestBackend>> + Send + 'static,
{
    pub fn new_tui(model: M, writer: M::Writer) -> Self
    where
        <M as elm_ui::Model>::Error: std::marker::Send,
    {
        Self::new(model, writer, |o| o.backend().buffer().clone())
    }
}

impl<M: Model + Send + 'static, O: Clone + Send + Sync + 'static> UiTester<M, O>
where
    M::Writer: Send + 'static,
{
    pub fn new(
        model: M,
        mut writer: M::Writer,
        mut get_output: impl FnMut(&mut M::Writer) -> O + Send + 'static,
    ) -> Self
    where
        <M as elm_ui::Model>::Error: std::marker::Send,
    {
        let mut program = Program::new(model).with_spawn_event_handler(false);
        let cmd_tx = program.cmd_tx();
        let term_view = Arc::new(RwLock::new(get_output(&mut writer)));
        let term_view_ = term_view.clone();
        let cancellation_token = CancellationToken::new();
        let cancellation_token_ = cancellation_token.clone();
        let handle: thread::JoinHandle<Result<Result<M, ProgramError<M>>, CancelledByShutdown>> =
            thread::spawn(move || {
                tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(async move {
                        let local = tokio::task::LocalSet::new();
                        local
                            .run_until(async move {
                                program.initialize().await?;
                                program
                                    .view(&mut writer)
                                    .map_err(ProgramError::<M>::ApplicationFailure)?;
                                while let Some(msg) = program.recv_msg().await {
                                    let quit_behavior = program.update(msg).await?;
                                    program
                                        .view(&mut writer)
                                        .map_err(ProgramError::<M>::ApplicationFailure)?;
                                    (*term_view_.write().unwrap()) = get_output(&mut writer);

                                    if quit_behavior == QuitBehavior::Quit {
                                        return Ok(program.shutdown().await);
                                    }
                                }

                                Ok(program.into_model())
                            })
                            .cancel_on_shutdown(&cancellation_token_)
                            .await
                    })
            });
        Self {
            cmd_tx,
            handle,
            term_view,
            cancellation_token,
        }
    }

    pub async fn send_cmd(&self, cmd: Command) {
        self.cmd_tx.send(cmd).await.unwrap();
    }

    pub async fn send_msg(&self, msg: Message) {
        self.cmd_tx.send(Command::simple(msg)).await.unwrap();
    }

    #[cfg(feature = "crossterm")]
    pub async fn send_key(&self, key_event: crossterm::event::KeyEvent) {
        self.send_msg(Message::TermEvent(crossterm::event::Event::Key(key_event)))
            .await;
    }

    pub async fn wait_for(&self, f: impl FnMut(&O) -> bool) -> Result<(), O> {
        self.wait_for_timeout(f, Duration::from_secs(5)).await
    }

    pub async fn wait_for_timeout(
        &self,
        mut f: impl FnMut(&O) -> bool,
        timeout: Duration,
    ) -> Result<(), O> {
        let start = Instant::now();
        loop {
            if f(&self.term_view.read().unwrap()) {
                return Ok(());
            }
            if Instant::now().duration_since(start) > timeout {
                return Err(self.term_view.read().unwrap().clone());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub fn wait_for_completion(self) -> Result<(M, O), ProgramError<M>> {
        let cancel_task = tokio::task::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            self.cancellation_token.cancel();
        });
        let model = self.handle.join().unwrap().expect("Failed to shut down")?;
        cancel_task.abort();
        Ok((model, self.term_view.read().unwrap().clone()))
    }
}

#[cfg(feature = "tui")]
pub trait TerminalView {
    fn terminal_view(&self) -> String;
}

#[cfg(feature = "tui")]
impl TerminalView for Buffer {
    fn terminal_view(&self) -> String {
        use std::fmt::Write;

        let Rect { width, height, .. } = self.area();
        let mut string_buf = String::with_capacity((width * height) as usize);
        for row in 0..*height {
            for col in 0..*width {
                let cell = self.get(col, row);
                write!(&mut string_buf, "{}", cell.symbol).unwrap();
            }
            writeln!(&mut string_buf).unwrap();
        }
        string_buf
    }
}
