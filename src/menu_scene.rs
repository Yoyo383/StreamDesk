use std::{
    net::TcpStream,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use eframe::egui::{self, Align, Button, Color32, FontId, Layout, RichText, TextEdit, Ui};
use remote_desktop::{protocol::Message, AppData, Scene, SceneChange};

use crate::{host_scene::HostScene, main_scene::MainScene};

fn connect_to_server(sender: Sender<Option<TcpStream>>) {
    thread::spawn(move || match TcpStream::connect("127.0.0.1:7643") {
        Ok(socket) => sender.send(Some(socket)),
        Err(_) => sender.send(None),
    });
}

fn numeric_text_edit(ui: &mut Ui, value: &mut String) {
    // Limit to 6 characters and allow only digits
    let response = ui.add(
        TextEdit::singleline(value)
            .hint_text("Enter code")
            .char_limit(6)
            .font(FontId::monospace(30.)),
    );

    // Sanitize input: keep only digits and truncate to 6 chars
    if response.changed() {
        *value = value.to_uppercase();
    }
}

pub struct MenuScene {
    session_code: String,
    socket_receiver: Receiver<Option<TcpStream>>,
    ui_enabled: bool,
    failed_to_connect: bool,
    join_fail_message: String,
}

impl MenuScene {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        connect_to_server(sender);

        Self {
            session_code: String::new(),
            socket_receiver: receiver,
            ui_enabled: false,
            failed_to_connect: false,
            join_fail_message: String::new(),
        }
    }

    fn host_button(&self, app_data: &mut AppData) -> SceneChange {
        let socket = app_data.socket.as_mut().unwrap();

        let host_message = Message::new_hosting();
        host_message.send(socket).unwrap();

        let join_message = Message::receive(socket).unwrap();
        // type is MessageType::Joining, to get the session code
        let session_code = join_message.general_data;

        SceneChange::To(Box::new(HostScene::new(session_code, app_data)))
    }

    fn join_button(&self, session_code: i32, app_data: &mut AppData) -> Option<SceneChange> {
        let socket = app_data.socket.as_mut().unwrap();

        let join_message = Message::new_joining(session_code);
        join_message.send(socket).unwrap();

        let message = Message::receive(socket).unwrap();
        if message.general_data == -1 {
            return None;
        } else {
            return Some(SceneChange::To(Box::new(MainScene::new(app_data))));
        }
    }
}

impl Scene for MenuScene {
    fn update(&mut self, ctx: &egui::Context, app_data: &mut AppData) -> Option<SceneChange> {
        let mut result: Option<SceneChange> = None;

        match self.socket_receiver.try_recv() {
            Ok(Some(socket)) => {
                app_data.socket = Some(socket);
                self.ui_enabled = true;
            }
            Ok(None) => self.failed_to_connect = true,
            _ => (),
        }

        egui::TopBottomPanel::top("title_bar").show(ctx, |ui| {
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                ui.label(RichText::new("Remote Desktop").size(40.0));
            })
        });

        egui::TopBottomPanel::bottom("connection_status")
            .resizable(false)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    if self.failed_to_connect {
                        ui.label(
                            RichText::new("Failed to connect to server.")
                                .color(Color32::RED)
                                .size(20.0),
                        );
                    } else if self.ui_enabled {
                        ui.label(
                            RichText::new("Connected to server!")
                                .color(Color32::GREEN)
                                .size(20.0),
                        );
                    } else {
                        ui.label(RichText::new("Connecting to server...").size(20.0));
                    }
                })
            });

        let available_width = ctx.available_rect().width();
        let panel_width = available_width / 2.0;

        egui::SidePanel::right("join_panel")
            .resizable(false)
            .exact_width(panel_width)
            .show(ctx, |ui| {
                if !self.ui_enabled {
                    ui.disable();
                }
                ui.vertical_centered(|ui| {
                    ui.add_space(10.0);
                    ui.label(RichText::new("Join Session").size(20.0));
                    ui.add_space(10.0);

                    numeric_text_edit(ui, &mut self.session_code);

                    ui.add_space(10.0);

                    if ui
                        .add_sized([100.0, 40.0], Button::new(RichText::new("Join").size(20.0)))
                        .clicked()
                    {
                        match self.session_code.parse::<i32>() {
                            Ok(session_code) => match self.join_button(session_code, app_data) {
                                Some(scene_change) => result = Some(scene_change),
                                None => {
                                    self.join_fail_message =
                                        format!("No session found with code {}", session_code)
                                }
                            },
                            Err(_) => {
                                self.join_fail_message =
                                    "Invalid session code. Please enter a 6 digit code.".to_string()
                            }
                        }
                    }

                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(&self.join_fail_message)
                            .size(20.0)
                            .color(Color32::RED),
                    );
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                if !self.ui_enabled {
                    ui.disable();
                }
                ui.label(RichText::new("Host Session").size(20.0));
                ui.add_space(10.0);

                if ui
                    .add_sized([100.0, 40.0], Button::new(RichText::new("Host").size(20.0)))
                    .clicked()
                {
                    result = Some(self.host_button(app_data));
                }
            });
        });

        result
    }

    fn on_exit(&mut self, _app_data: &mut AppData) {}
}
