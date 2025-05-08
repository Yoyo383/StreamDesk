use std::{net::TcpStream, sync::mpsc::Receiver};

use eframe::egui::{self, Align, Color32, FontId, Layout, RichText, TextEdit};
use remote_desktop::{
    protocol::{Message, MessageType},
    AppData, Scene, SceneChange,
};

use crate::menu_scene::MenuScene;

pub struct LoginScene {
    username: String,
    password: String,
    socket_receiver: Option<Receiver<Option<TcpStream>>>,
    connected_to_server: bool,
    failed_to_connect: bool,
    error_message: String,
}

impl LoginScene {
    pub fn new(
        socket_receiver: Option<Receiver<Option<TcpStream>>>,
        connected_to_server: bool,
    ) -> Self {
        Self {
            username: String::new(),
            password: String::new(),
            socket_receiver,
            connected_to_server,
            failed_to_connect: false,
            error_message: String::new(),
        }
    }

    fn login(&mut self, socket: &mut TcpStream) -> Option<SceneChange> {
        let password = format!("{:x}", md5::compute(self.password.clone()));

        let message = Message::new_login(&self.username, &password);
        message.send(socket).unwrap();

        let message = Message::receive(socket).unwrap();

        if message.message_type == MessageType::Login {
            return Some(SceneChange::To(Box::new(MenuScene::new(
                self.username.clone(),
            ))));
        }

        if message.message_type == MessageType::None {
            self.error_message = "Username or password are incorrect.".to_owned();
        }

        None
    }

    fn register(&mut self, socket: &mut TcpStream) -> Option<SceneChange> {
        let password = format!("{:x}", md5::compute(self.password.clone()));

        let message = Message::new_register(&self.username, &password);
        message.send(socket).unwrap();

        let message = Message::receive(socket).unwrap();

        if message.message_type == MessageType::Register {
            return Some(SceneChange::To(Box::new(MenuScene::new(
                self.username.clone(),
            ))));
        }

        if message.message_type == MessageType::None {
            self.error_message = "Username already taken.".to_owned();
        }

        None
    }
}

impl Scene for LoginScene {
    fn update(&mut self, ctx: &egui::Context, app_data: &mut AppData) -> Option<SceneChange> {
        let mut result: Option<SceneChange> = None;

        if !self.connected_to_server {
            match self.socket_receiver.as_ref().unwrap().try_recv() {
                Ok(Some(socket)) => {
                    app_data.socket = Some(socket);
                    self.connected_to_server = true;
                }
                Ok(None) => self.failed_to_connect = true,
                _ => (),
            }
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
                    ui.add_space(10.0);

                    if self.failed_to_connect {
                        ui.label(
                            RichText::new("Failed to connect to server.")
                                .color(Color32::RED)
                                .size(20.0),
                        );
                    } else if self.connected_to_server {
                        ui.label(
                            RichText::new("Connected to server!")
                                .color(Color32::GREEN)
                                .size(20.0),
                        );
                    } else {
                        ui.label(RichText::new("Connecting to server...").size(20.0));
                    }

                    ui.add_space(10.0);
                })
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            if !self.connected_to_server {
                ui.disable();
            }
            ui.add(
                TextEdit::singleline(&mut self.username)
                    .hint_text("Username")
                    .font(FontId::proportional(30.0)),
            );

            ui.add(
                TextEdit::singleline(&mut self.password)
                    .hint_text("Password")
                    .font(FontId::proportional(30.0))
                    .password(true),
            );

            if ui.button("Login").clicked() {
                result = self.login(app_data.socket.as_mut().unwrap());
            }

            if ui.button("Register").clicked() {
                result = self.register(app_data.socket.as_mut().unwrap());
            }

            ui.add_space(10.0);
            ui.label(
                RichText::new(&self.error_message)
                    .size(20.0)
                    .color(Color32::RED),
            );
        });

        result
    }

    fn on_exit(&mut self, app_data: &mut AppData) {
        let socket = app_data.socket.as_mut().unwrap();

        let message = Message::new_shutdown();
        message.send(socket).unwrap();

        socket
            .shutdown(std::net::Shutdown::Both)
            .expect("Could not close socket.");
    }
}
