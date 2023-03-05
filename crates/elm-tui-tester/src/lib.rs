use elm_ui::{Command, Message, Model, Program, ProgramError, QuitBehavior};
use std::{
    fmt::Write,
    sync::{Arc, RwLock},
    thread,
    time::Duration,
};
use tokio::sync::mpsc;
use tui::{backend::TestBackend, buffer::Buffer, layout::Rect, Terminal};

pub struct TuiTester<M: Model<Writer = Terminal<TestBackend>> + Send + 'static> {
    cmd_tx: mpsc::Sender<Command>,
    term_view: Arc<RwLock<String>>,
    handle: thread::JoinHandle<Result<M, ProgramError<M>>>,
}

impl<M: Model<Writer = Terminal<TestBackend>> + Send + 'static> TuiTester<M> {
    pub fn new(model: M, mut terminal: M::Writer) -> Self
    where
        M: Model<Writer = Terminal<TestBackend>> + Send + 'static,
        <M as elm_ui::Model>::Error: std::marker::Send,
    {
        let mut program = Program::new(model);
        let cmd_tx = program.cmd_tx();
        let term_view = Arc::new(RwLock::new(String::new()));
        let term_view_ = term_view.clone();
        let handle: thread::JoinHandle<Result<M, ProgramError<M>>> = thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async move {
                    program.initialize().await?;
                    program
                        .view(&mut terminal)
                        .map_err(ProgramError::<M>::ApplicationFailure)?;
                    while let Some(msg) = program.recv_msg().await {
                        let quit_behavior = program.update(msg).await?;
                        program
                            .view(&mut terminal)
                            .map_err(ProgramError::<M>::ApplicationFailure)?;
                        (*term_view_.write().unwrap()) = terminal_view(terminal.backend().buffer());

                        if quit_behavior == QuitBehavior::Quit {
                            return Ok(program.shutdown().await);
                        }
                    }

                    Ok(program.into_model())
                })
        });
        Self {
            cmd_tx,
            handle,
            term_view,
        }
    }

    pub async fn send_cmd(&self, cmd: Command) {
        self.cmd_tx.send(cmd).await.unwrap();
    }

    pub async fn send_msg(&self, msg: Message) {
        self.cmd_tx.send(Command::simple(msg)).await.unwrap();
    }

    pub async fn wait_for(&self, mut f: impl FnMut(&str) -> bool) -> Result<(), String> {
        for _ in 0..100 {
            if f(&self.term_view.read().unwrap()) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        return Err(self.term_view.read().unwrap().to_string());
    }

    pub fn wait_for_completion(self) -> Result<(M, String), ProgramError<M>> {
        let model = self.handle.join().unwrap()?;
        Ok((model, self.term_view.read().unwrap().to_string()))
    }
}

fn terminal_view(buffer: &Buffer) -> String {
    let Rect { width, height, .. } = buffer.area();
    let mut string_buf = String::with_capacity((width * height) as usize);
    for row in 0..*height {
        for col in 0..*width {
            write!(&mut string_buf, "{}", buffer.get(col, row).symbol).unwrap();
        }
        writeln!(&mut string_buf).unwrap();
    }
    string_buf
}
