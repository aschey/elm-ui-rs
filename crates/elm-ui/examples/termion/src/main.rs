use elm_ui::{Command, Message, Model, OptionalCommand, Program};
use std::{
    error::Error,
    io::{self, Stdout},
    sync::Arc,
};
use termion::{
    event::Key,
    input::TermRead,
    raw::{IntoRawMode, RawTerminal},
    screen::AlternateScreen,
};
use tokio::{sync::mpsc, task};
use tui::{
    backend::{Backend, TermionBackend},
    style::{Color, Style},
    widgets::{List, ListItem, ListState},
    Frame, Terminal,
};

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn Error>> {
    let stdout = io::stdout().into_raw_mode()?;
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let program = Program::new(App::default());
    let cmd_tx = program.cmd_tx();
    spawn_event_reader(cmd_tx);
    program.run(&mut terminal).await?;

    terminal.backend_mut().flush()?;
    terminal.flush()?;
    drop(terminal);

    Ok(())
}

#[derive(Debug)]
pub enum AppMessage {
    SetListItems(Vec<String>),
    KeyEvent(Key),
}

#[derive(Default, Debug)]
pub struct App {
    list_items: Vec<String>,
    list_index: Option<usize>,
    list_state: ListState,
}

fn spawn_event_reader(cmd_tx: mpsc::Sender<Command>) {
    task::spawn_blocking(move || {
        let stdin = std::io::stdin();
        for event in stdin.keys().flatten() {
            cmd_tx
                .blocking_send(Command::simple(Message::custom(AppMessage::KeyEvent(
                    event,
                ))))
                .unwrap();
            if matches!(event, Key::Char('q')) {
                // Need to ensure we drop the stdin reference cleanly here
                return;
            }
        }
    });
}

impl Model for App {
    type Writer = Terminal<TermionBackend<AlternateScreen<RawTerminal<Stdout>>>>;
    type Error = io::Error;

    fn init(&mut self) -> Result<OptionalCommand, Self::Error> {
        Ok(Some(Command::simple(Message::Custom(Box::new(
            AppMessage::SetListItems(vec!["first item".to_owned(), "second_item".to_owned()]),
        )))))
    }

    fn update(&mut self, msg: Arc<Message>) -> Result<OptionalCommand, Self::Error> {
        if let Message::Custom(msg) = msg.as_ref() {
            if let Some(msg) = msg.downcast_ref::<AppMessage>() {
                match msg {
                    AppMessage::SetListItems(items) => {
                        self.list_items = items.clone();
                        if self.list_items.is_empty() {
                            self.list_index = None;
                        } else {
                            self.list_index = Some(0);
                            self.list_state.select(self.list_index);
                        }
                    }
                    AppMessage::KeyEvent(Key::Char('q')) => {
                        return Ok(Some(Command::simple(Message::Quit)));
                    }
                    AppMessage::KeyEvent(Key::Up) => {
                        if let Some(list_index) = self.list_index.as_mut() {
                            if *list_index > 0 {
                                *list_index -= 1;
                                self.list_state.select(Some(*list_index));
                            }
                        }
                    }
                    AppMessage::KeyEvent(Key::Down) => {
                        if let Some(list_index) = self.list_index.as_mut() {
                            if *list_index < self.list_items.len() - 1 {
                                *list_index += 1;
                                self.list_state.select(Some(*list_index));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(None)
    }

    fn view(&self, terminal: &mut Self::Writer) -> Result<(), Self::Error> {
        terminal.draw(|f| ui(f, self))?;
        Ok(())
    }
}

fn ui(f: &mut Frame<TermionBackend<AlternateScreen<RawTerminal<Stdout>>>>, app: &App) {
    let items: Vec<ListItem> = app
        .list_items
        .iter()
        .map(|l| ListItem::new(l.clone()))
        .collect();
    f.render_stateful_widget(
        List::new(items).highlight_style(Style::default().fg(Color::Green).bg(Color::Black)),
        f.size(),
        &mut app.list_state.clone(),
    )
}
