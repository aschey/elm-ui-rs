use std::{io, time::Duration};

use elm_ui::{Command, Message, Model, OptionalCommand, Program};
use fltk::{
    app,
    button::Button,
    enums::{Color, FrameType},
    frame::Frame,
    prelude::*,
    window::Window,
};

fn main() {
    let app = app::App::default();
    let mut wind = Window::default().with_size(400, 300);
    let mut frame = Frame::default()
        .with_size(100, 40)
        .center_of(&wind)
        .with_label("0");
    frame.set_label_size(20);

    let mut but_inc = Button::default()
        .size_of(&frame)
        .above_of(&frame, 0)
        .with_label("+5");
    let mut but_dec = Button::default()
        .size_of(&frame)
        .below_of(&frame, 0)
        .with_label("-5");

    wind.end();
    wind.show();

    let program = Program::new(App { val: 0 });
    let cmd_tx = program.cmd_tx();
    but_inc.set_callback({
        let cmd_tx = cmd_tx.clone();
        move |_| {
            cmd_tx
                .try_send(Command::simple(Message::custom(AppMessage::Increment)))
                .unwrap();
        }
    });

    but_dec.set_callback(move |_| {
        cmd_tx
            .try_send(Command::simple(Message::custom(AppMessage::Decrement)))
            .unwrap();
    });
    but_inc.set_color(Color::from_u32(0x304FFE));
    but_inc.set_selection_color(Color::Green);
    but_inc.set_label_size(20);
    but_inc.set_frame(FrameType::FlatBox);
    but_inc.set_label_color(Color::White);
    but_dec.set_color(Color::from_u32(0x2962FF));
    but_dec.set_selection_color(Color::Red);
    but_dec.set_frame(FrameType::FlatBox);
    but_dec.set_label_size(20);
    but_dec.set_label_color(Color::White);

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            program.run(&mut frame).await.unwrap();
        });
    });

    app.run().unwrap();
}

#[derive(Debug, Copy, Clone)]
pub enum AppMessage {
    AutoIncrement,
    Increment,
    Decrement,
}

#[derive(Debug)]
pub struct App {
    val: i32,
}

impl Model for App {
    type Writer = Frame;
    type Error = io::Error;

    fn init(&mut self) -> Result<OptionalCommand, Self::Error> {
        Ok(Some(Command::new_async(|_, _| async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Some(Message::custom(AppMessage::AutoIncrement))
        })))
    }

    fn update(&mut self, msg: std::sync::Arc<Message>) -> Result<OptionalCommand, Self::Error> {
        if let Message::Custom(custom_msg) = msg.as_ref() {
            if let Some(msg) = custom_msg.downcast_ref::<AppMessage>() {
                match msg {
                    AppMessage::AutoIncrement => {
                        self.val += 1;
                        return Ok(Some(Command::new_async(move |_, _| async move {
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            Some(Message::custom(AppMessage::AutoIncrement))
                        })));
                    }
                    AppMessage::Increment => {
                        self.val += 5;
                    }
                    AppMessage::Decrement => {
                        self.val -= 5;
                    }
                }
            }
        }
        Ok(None)
    }

    fn view(&self, frame: &mut Self::Writer) -> Result<(), Self::Error> {
        frame.set_label(&self.val.to_string());
        app::awake();
        Ok(())
    }
}
