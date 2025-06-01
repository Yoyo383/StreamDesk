use std::sync::mpsc::Receiver;

use eframe::egui::{self, Align, Color32, FontId, Layout, RichText, SelectableLabel, TextEdit};
use remote_desktop::{
    protocol::{Packet, ResultPacket},
    secure_channel::SecureChannel,
    Scene, SceneChange,
};

use crate::menu_scene::MenuScene;

pub struct LoginScene {
    login_username: String,
    login_password: String,
    register_username: String,
    register_password: String,
    register_confirm_password: String,

    socket_receiver: Option<Receiver<Option<SecureChannel>>>,
    connected_to_server: bool,
    failed_to_connect: bool,

    error_message_login: String,
    error_message_register: String,

    is_login: bool,
}

impl LoginScene {
    pub fn new(
        socket_receiver: Option<Receiver<Option<SecureChannel>>>,
        connected_to_server: bool,
    ) -> Self {
        Self {
            login_username: String::new(),
            login_password: String::new(),
            register_username: String::new(),
            register_password: String::new(),
            register_confirm_password: String::new(),

            socket_receiver,
            connected_to_server,
            failed_to_connect: false,

            error_message_login: String::new(),
            error_message_register: String::new(),

            is_login: true,
        }
    }

    fn login(&mut self, channel: &mut SecureChannel) -> SceneChange {
        if self.login_username.len() > 20 {
            self.error_message_login = "Username cannot be longer than 20 characters.".to_string();
            return SceneChange::None;
        }

        let password = format!("{:x}", md5::compute(self.login_password.clone()));

        let login_packet = Packet::Login {
            username: self.login_username.clone(),
            password,
        };
        channel.send(login_packet).unwrap();

        let result = channel.receive().unwrap();
        match result {
            ResultPacket::Failure(msg) => {
                self.error_message_login = msg;
                SceneChange::None
            }

            ResultPacket::Success(_) => SceneChange::To(Box::new(MenuScene::new(
                self.login_username.clone(),
                channel,
                "",
            ))),
        }
    }

    fn register(&mut self, channel: &mut SecureChannel) -> SceneChange {
        // make sure passwords match
        if self.register_password != self.register_confirm_password {
            self.error_message_register = "Passwords do not match.".to_string();
            return SceneChange::None;
        }

        // validate credentials
        if self.register_username.is_empty() {
            self.error_message_register = "Username cannot be empty.".to_string();
            return SceneChange::None;
        }

        if self
            .register_username
            .chars()
            .any(|c| !c.is_ascii_alphanumeric())
        {
            self.error_message_register =
                "Username can only contain English letters and numbers.".to_string();
            return SceneChange::None;
        }

        if self.register_username.len() > 20 {
            self.error_message_register =
                "Username cannot be longer than 20 characters.".to_string();
            return SceneChange::None;
        }

        if self.register_password.is_empty() {
            self.error_message_register = "Password cannot be empty.".to_string();
            return SceneChange::None;
        }

        // send to server
        let password = format!("{:x}", md5::compute(self.register_password.clone()));

        let register_packet = Packet::Register {
            username: self.register_username.clone(),
            password,
        };
        channel.send(register_packet).unwrap();

        let result = channel.receive().unwrap();
        match result {
            ResultPacket::Failure(msg) => {
                self.error_message_register = msg;
                SceneChange::None
            }

            ResultPacket::Success(_) => SceneChange::To(Box::new(MenuScene::new(
                self.register_username.clone(),
                channel,
                "",
            ))),
        }
    }
}

impl Scene for LoginScene {
    fn update(&mut self, ctx: &egui::Context, channel: &mut SecureChannel) -> SceneChange {
        let mut result: SceneChange = SceneChange::None;

        if !self.connected_to_server {
            match self.socket_receiver.as_ref().unwrap().try_recv() {
                Ok(Some(new_channel)) => {
                    *channel = new_channel;
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

            ui.horizontal(|ui| {
                let login_text = RichText::new("Login").size(20.0);
                let register_text = RichText::new("Register").size(20.0);

                let login_label = SelectableLabel::new(self.is_login, login_text);
                if ui.add(login_label).clicked() {
                    self.is_login = true;
                }

                let register_label = SelectableLabel::new(!self.is_login, register_text);
                if ui.add(register_label).clicked() {
                    self.is_login = false;
                }
            });

            ui.add_space(10.0);

            ui.vertical_centered(|ui| {
                if self.is_login {
                    ui.add(
                        TextEdit::singleline(&mut self.login_username)
                            .hint_text("Username")
                            .font(FontId::proportional(30.0)),
                    );

                    ui.add_space(5.0);

                    ui.add(
                        TextEdit::singleline(&mut self.login_password)
                            .hint_text("Password")
                            .font(FontId::proportional(30.0))
                            .password(true),
                    );

                    ui.add_space(5.0);

                    if ui
                        .add_sized(
                            [100.0, 40.0],
                            egui::Button::new(RichText::new("Login").size(20.0)),
                        )
                        .clicked()
                    {
                        result = self.login(channel);
                    }

                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(&self.error_message_login)
                            .size(20.0)
                            .color(Color32::RED),
                    );
                } else {
                    ui.add(
                        TextEdit::singleline(&mut self.register_username)
                            .hint_text("Username")
                            .font(FontId::proportional(30.0)),
                    );

                    ui.add_space(5.0);

                    ui.add(
                        TextEdit::singleline(&mut self.register_password)
                            .hint_text("Password")
                            .font(FontId::proportional(30.0))
                            .password(true),
                    );

                    ui.add_space(5.0);

                    ui.add(
                        TextEdit::singleline(&mut self.register_confirm_password)
                            .hint_text("Confirm assword")
                            .font(FontId::proportional(30.0))
                            .password(true),
                    );

                    ui.add_space(5.0);

                    if ui
                        .add_sized(
                            [100.0, 40.0],
                            egui::Button::new(RichText::new("Register").size(20.0)),
                        )
                        .clicked()
                    {
                        result = self.register(channel);
                    }

                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(&self.error_message_register)
                            .size(20.0)
                            .color(Color32::RED),
                    );
                }
            });
        });

        result
    }

    fn on_exit(&mut self, channel: &mut SecureChannel) {
        channel.send(Packet::Shutdown).unwrap();

        channel.close();
    }
}
