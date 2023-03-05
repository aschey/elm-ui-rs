use std::{
    error::Error,
    io::{self, Write},
    time::Duration,
};

use elm_ui::{Command, Message, Model, OptionalCommand, Program};

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn Error>> {
    Program::new(App::default()).run(&mut io::stdout()).await?;
    Ok(())
}

struct TickMsg(usize);

#[derive(Default, Debug)]
pub struct App {
    seq_num: usize,
    quitting: bool,
}

impl Model for App {
    type Writer = io::Stdout;

    type Error = io::Error;

    fn init(&mut self) -> Result<OptionalCommand, Self::Error> {
        Ok(Some(Command::new_async(|_, _| async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Some(Message::custom(TickMsg(1)))
        })))
    }

    fn update(&mut self, msg: std::sync::Arc<Message>) -> Result<OptionalCommand, Self::Error> {
        if let Message::Custom(custom_msg) = msg.as_ref() {
            if let Some(TickMsg(seq_num)) = custom_msg.downcast_ref() {
                let seq_num = *seq_num;
                self.seq_num = seq_num;
                if seq_num > 5 {
                    self.quitting = true;
                    return Ok(Some(Command::quit()));
                }

                return Ok(Some(Command::new_async(move |_, _| async move {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    Some(Message::custom(TickMsg(seq_num + 1)))
                })));
            }
        }
        Ok(None)
    }

    fn view(&self, writer: &mut Self::Writer) -> Result<(), Self::Error> {
        if !self.quitting {
            writer.write_all(format!("hello {}\n", self.seq_num).as_bytes())?;
            writer.flush()?;
        }
        Ok(())
    }
}
