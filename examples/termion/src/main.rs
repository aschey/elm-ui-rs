use std::{
    error::Error,
    io::{self, Stdout},
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
use tui_elm::{Command, Message, Model, OptionalCommand, Program};

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
    type CustomMessage = AppMessage;
    type Writer = Terminal<TermionBackend<AlternateScreen<MouseTerminal<RawTerminal<Stdout>>>>>;
    type Error = io::Error;

    fn init(&self) -> Result<OptionalCommand<Self::CustomMessage>, Self::Error> {
        Ok(Some(Command::new_async(async move {
            Message::Custom(AppMessage::SetListItems(vec![
                "first item".to_owned(),
                "second_item".to_owned(),
            ]))
        })))
    }

    fn update(
        &mut self,
        msg: tui_elm::Message<Self::CustomMessage>,
    ) -> Result<OptionalCommand<Self::CustomMessage>, Self::Error> {
        match msg {
            Message::Custom(AppMessage::SetListItems(items)) => {
                self.list_items = items;
                if self.list_items.is_empty() {
                    self.list_index = None;
                } else {
                    self.list_index = Some(0);
                }
            }
            Message::TermEvent(Event::Key(Key::Char('q' | 'Q'))) => {
                return Ok(Some(Command::new_async(async move { Message::Quit })));
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
