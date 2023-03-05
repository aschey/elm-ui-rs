use elm_ui::{
    future_ext::{CancelledByShutdown, FutureExt},
    Command, Message, Model, Program, ProgramError, QuitBehavior,
};
use std::{
    fmt::Write,
    sync::{Arc, RwLock},
    thread,
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tui::{ backend::TestBackend, buffer::Buffer, layout::Rect, Terminal};

pub struct TuiTester<M: Model<Writer = Terminal<TestBackend>> + Send + 'static> {
    cmd_tx: mpsc::Sender<Command>,
    term_view: Arc<RwLock<Buffer>>,
    handle: thread::JoinHandle<Result<Result<M, ProgramError<M>>, CancelledByShutdown>>,
    cancellation_token: CancellationToken,
}

impl<M: Model<Writer = Terminal<TestBackend>> + Send + 'static> TuiTester<M> {
    pub fn new(model: M, mut terminal: M::Writer) -> Self
    where
        M: Model<Writer = Terminal<TestBackend>> + Send + 'static,
        <M as elm_ui::Model>::Error: std::marker::Send,
    {
        let mut program = Program::new(model);
        let cmd_tx = program.cmd_tx();
        let term_view = Arc::new(RwLock::new(terminal.backend().buffer().clone()));
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
                                    .view(&mut terminal)
                                    .map_err(ProgramError::<M>::ApplicationFailure)?;
                                while let Some(msg) = program.recv_msg().await {
                                    let quit_behavior = program.update(msg).await?;
                                    program
                                        .view(&mut terminal)
                                        .map_err(ProgramError::<M>::ApplicationFailure)?;
                                    (*term_view_.write().unwrap()) =
                                        terminal.backend().buffer().clone();

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

    pub async fn wait_for(&self, mut f: impl FnMut(&Buffer) -> bool) -> Result<(), String> {
        for _ in 0..100 {
            if f(&self.term_view.read().unwrap()) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        return Err(terminal_view(&self.term_view.read().unwrap()));
    }

    pub fn wait_for_completion(self) -> Result<(M, Buffer), ProgramError<M>> {
        let cancel_task = tokio::task::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            self.cancellation_token.cancel();
        });
        let model = self.handle.join().unwrap().expect("Failed to shut down")?;
        cancel_task.abort();
        Ok((model, self.term_view.read().unwrap().clone()))
    }
}

pub fn terminal_view(buffer: &Buffer) -> String {
    let Rect { width, height, .. } = buffer.area();
    let mut string_buf = String::with_capacity((width * height) as usize);
    for row in 0..*height {
        for col in 0..*width {
            let cell = buffer.get(col, row);
            write!(&mut string_buf, "{}", cell.symbol).unwrap();
        }
        writeln!(&mut string_buf).unwrap();
    }
    string_buf
}
