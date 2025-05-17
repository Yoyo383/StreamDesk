use std::{
    collections::HashMap,
    net::TcpStream,
    sync::mpsc::{self, Receiver},
    thread,
};

use eframe::egui::{self, Align, Button, Color32, FontId, Layout, RichText, TextEdit, Ui};
use remote_desktop::{
    protocol::{Packet, ResultPacket},
    AppData, Scene, SceneChange,
};

use crate::{
    host_scene::HostScene, login_scene::LoginScene, main_scene::MainScene, watch_scene::WatchScene,
};

fn numeric_text_edit(ui: &mut Ui, value: &mut String) {
    let response = ui.add(
        TextEdit::singleline(value)
            .hint_text("Enter code")
            .char_limit(6)
            .font(FontId::monospace(30.)),
    );

    if response.changed() {
        *value = value.to_uppercase();
    }
}

fn receive_recordings(socket: &mut TcpStream) -> HashMap<i32, String> {
    let mut recordings = HashMap::new();

    loop {
        let packet = Packet::receive(socket).unwrap();

        match packet {
            Packet::None => break,

            Packet::RecordingName { id, name } => {
                recordings.insert(id, name);
            }

            _ => (),
        }
    }

    recordings
}

pub struct MenuScene {
    session_code: String,
    join_fail_message: String,
    username: String,
    recordings: HashMap<i32, String>,
    join_receiver: Option<Receiver<(bool, String)>>,
    is_disabled: bool,
}

impl MenuScene {
    pub fn new(username: String, socket: &mut TcpStream) -> Self {
        let recordings = receive_recordings(socket);

        Self {
            session_code: String::new(),
            join_fail_message: String::new(),
            username,
            recordings,
            join_receiver: None,
            is_disabled: false,
        }
    }

    fn host_button(&self, app_data: &mut AppData) -> SceneChange {
        let socket = app_data.socket.as_mut().unwrap();

        let host_packet = Packet::Host;
        host_packet.send(socket).unwrap();

        let result = ResultPacket::receive(socket).unwrap();
        let ResultPacket::Success(code) = result else {
            panic!("should be success");
        };

        SceneChange::To(Box::new(HostScene::new(
            code,
            app_data,
            self.username.to_string(),
        )))
    }

    fn join_button(&mut self, session_code: u32, app_data: &mut AppData) -> Result<String, String> {
        let socket = app_data.socket.as_mut().unwrap();

        let join_message = Packet::Join {
            code: session_code,
            username: self.username.clone(),
        };
        join_message.send(socket).unwrap();

        let result = ResultPacket::receive(socket).unwrap();
        match result {
            ResultPacket::Failure(msg) => Err(msg),
            ResultPacket::Success(_) => {
                let mut socket = socket.try_clone().unwrap();
                let (sender, receiver) = mpsc::channel();
                self.join_receiver = Some(receiver);
                self.is_disabled = true;

                thread::spawn(move || {
                    let result = ResultPacket::receive(&mut socket).unwrap();

                    match result {
                        ResultPacket::Failure(msg) => sender.send((false, msg)),
                        ResultPacket::Success(msg) => sender.send((true, msg)),
                    }
                });

                Ok("Waiting for the host to approve...".to_string())
            }
        }
    }
}

impl Scene for MenuScene {
    fn update(&mut self, ctx: &egui::Context, app_data: &mut AppData) -> SceneChange {
        let mut result: SceneChange = SceneChange::None;

        if let Some(join_receiver) = &self.join_receiver {
            if let Ok((join_result, msg)) = join_receiver.try_recv() {
                self.is_disabled = false;

                match join_result {
                    true => {
                        result = SceneChange::To(Box::new(MainScene::new(
                            app_data,
                            self.username.clone(),
                        )))
                    }

                    false => {
                        self.join_fail_message = msg;
                        self.recordings = receive_recordings(app_data.socket.as_mut().unwrap());
                    }
                }
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
                if self.is_disabled {
                    ui.disable();
                }

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("Sign out").size(20.0),
                        ))
                        .clicked()
                    {
                        let signout_packet = Packet::SignOut;
                        signout_packet
                            .send(app_data.socket.as_mut().unwrap())
                            .unwrap();

                        result = SceneChange::To(Box::new(LoginScene::new(None, true)));
                    }

                    ui.label(RichText::new(format!("Welcome, {}", self.username)).size(20.0));
                });

                ui.add_space(10.0);
            });

        egui::SidePanel::right("join_panel").show(ctx, |ui| {
            if self.is_disabled {
                ui.disable();
            }

            ui.heading("Watch past recordings");
            ui.separator();

            for (id, recording) in &self.recordings {
                if ui.button(recording).clicked() {
                    let packet = Packet::WatchRecording { id: *id };
                    packet.send(app_data.socket.as_mut().unwrap()).unwrap();

                    let result_packet =
                        ResultPacket::receive(app_data.socket.as_mut().unwrap()).unwrap();

                    match result_packet {
                        ResultPacket::Failure(msg) => {
                            self.join_fail_message = msg;
                        }

                        ResultPacket::Success(_) => {
                            result = SceneChange::To(Box::new(WatchScene::new(
                                self.username.clone(),
                                app_data.socket.as_mut().unwrap(),
                            )));
                        }
                    }
                }
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.is_disabled {
                ui.disable();
            }

            ui.vertical_centered(|ui| {
                ui.add_space(10.0);
                ui.label(RichText::new("Join Session").size(20.0));
                ui.add_space(10.0);

                numeric_text_edit(ui, &mut self.session_code);

                ui.add_space(10.0);

                let join_button = ui.add(|ui: &mut Ui| {
                    ui.add_sized([100.0, 40.0], Button::new(RichText::new("Join").size(20.0)))
                });

                if join_button.clicked() {
                    match self.session_code.parse::<u32>() {
                        Ok(session_code) => match self.join_button(session_code, app_data) {
                            Ok(msg) => self.join_fail_message = msg,
                            Err(msg) => {
                                self.join_fail_message = msg;
                                self.recordings =
                                    receive_recordings(app_data.socket.as_mut().unwrap());
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

            ui.separator();

            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Host Session").size(20.0));
                ui.add_space(10.0);

                let host_button = ui.add_enabled(!self.username.is_empty(), |ui: &mut Ui| {
                    ui.add_sized([100.0, 40.0], Button::new(RichText::new("Host").size(20.0)))
                });

                if host_button.clicked() {
                    result = self.host_button(app_data);
                }
            });
        });

        result
    }

    fn on_exit(&mut self, app_data: &mut AppData) {
        let socket = app_data.socket.as_mut().unwrap();

        let signout_packet = Packet::SignOut;
        signout_packet.send(socket).unwrap();

        let shutdown_packet = Packet::Shutdown;
        shutdown_packet.send(socket).unwrap();

        socket
            .shutdown(std::net::Shutdown::Both)
            .expect("Could not close socket.");
    }
}
