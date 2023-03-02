use elm_ui::{Command, Message, Model, OptionalCommand, Program};
use std::{
    error::Error,
    io::{self, Stdout},
    sync::Arc,
};
use termion::{
    event::{Event, Key},
    input::MouseTerminal,
    raw::{IntoRawMode, RawTerminal},
    screen::AlternateScreen,
};
use tui::{
    backend::TermionBackend,
    style::{Color, Style},
    widgets::{List, ListItem, ListState},
    Frame, Terminal,
};

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn Error>> {
    let stdout = io::stdout().into_raw_mode()?;
    let stdout = MouseTerminal::from(stdout);
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let program = Program::new(App::default());
    program.run(&mut terminal).await?;
    Ok(())
}

#[derive(Debug)]
pub enum AppMessage {
    SetListItems(Vec<String>),
}

#[derive(Default, Debug)]
pub struct App {
    list_items: Vec<String>,
    list_index: Option<usize>,
    list_state: ListState,
}

impl Model for App {
    type Writer = Terminal<TermionBackend<AlternateScreen<MouseTerminal<RawTerminal<Stdout>>>>>;
    type Error = io::Error;

    fn init(&mut self) -> Result<OptionalCommand, Self::Error> {
        Ok(Some(Command::simple(Message::Custom(Box::new(
            AppMessage::SetListItems(vec!["first item".to_owned(), "second_item".to_owned()]),
        )))))
    }

    fn update(&mut self, msg: Arc<Message>) -> Result<OptionalCommand, Self::Error> {
        match msg.as_ref() {
            Message::Custom(msg) => {
                if let Some(AppMessage::SetListItems(items)) = msg.downcast_ref::<AppMessage>() {
                    self.list_items = items.clone();
                    if self.list_items.is_empty() {
                        self.list_index = None;
                    } else {
                        self.list_index = Some(0);
                        self.list_state.select(self.list_index);
                    }
                }
            }
            Message::TermEvent(Event::Key(Key::Char('q' | 'Q'))) => {
                return Ok(Some(Command::simple(Message::Quit)));
            }
            Message::TermEvent(Event::Key(Key::Up)) => {
                if let Some(list_index) = self.list_index.as_mut() {
                    if *list_index > 0 {
                        *list_index -= 1;
                        self.list_state.select(Some(*list_index));
                    }
                }
            }
            Message::TermEvent(Event::Key(Key::Down)) => {
                if let Some(list_index) = self.list_index.as_mut() {
                    if *list_index < self.list_items.len() - 1 {
                        *list_index += 1;
                        self.list_state.select(Some(*list_index));
                    }
                }
            }
            _ => {}
        }
        Ok(None)
    }

    fn view(&self, terminal: &mut Self::Writer) -> Result<(), Self::Error> {
        terminal.draw(|f| ui(f, self))?;
        Ok(())
    }
}

fn ui(
    f: &mut Frame<TermionBackend<AlternateScreen<MouseTerminal<RawTerminal<Stdout>>>>>,
    app: &App,
) {
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
