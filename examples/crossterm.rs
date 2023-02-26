use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::{
    error::Error,
    io::{self, Stdout},
    sync::Arc,
};
use tui::{
    backend::CrosstermBackend,
    style::{Color, Style},
    widgets::{List, ListItem, ListState},
    Frame, Terminal,
};
use tui_elm::{Command, Message, Model, OptionalCommand, Program};

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let program = Program::new(App::default());
    program.run(&mut terminal).await?;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
    )?;
    terminal.show_cursor()?;
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

impl App {
    fn ui(&self, f: &mut Frame<CrosstermBackend<Stdout>>) {
        let items: Vec<ListItem> = self
            .list_items
            .iter()
            .map(|l| ListItem::new(l.clone()))
            .collect();
        f.render_stateful_widget(
            List::new(items).highlight_style(Style::default().fg(Color::Green).bg(Color::Black)),
            f.size(),
            &mut self.list_state.clone(),
        )
    }
}

impl Model for App {
    type Writer = Terminal<CrosstermBackend<io::Stdout>>;
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
                    }
                }
            }
            Message::TermEvent(Event::Key(KeyEvent {
                code: KeyCode::Char('q' | 'Q'),
                ..
            })) => {
                return Ok(Some(Command::new_async(|_, _| async move {
                    Some(Message::Quit)
                })));
            }
            Message::TermEvent(Event::Key(KeyEvent {
                code: KeyCode::Up, ..
            })) => {
                if let Some(list_index) = self.list_index.as_mut() {
                    if *list_index > 0 {
                        *list_index -= 1;
                        self.list_state.select(Some(*list_index));
                    }
                }
            }
            Message::TermEvent(Event::Key(KeyEvent {
                code: KeyCode::Down,
                ..
            })) => {
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
        terminal.draw(|f| self.ui(f))?;
        Ok(())
    }
}
