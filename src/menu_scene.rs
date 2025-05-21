use std::{
    collections::HashMap,
    sync::mpsc::{self, Receiver},
    thread,
};

use chrono::{DateTime, Local};
use eframe::egui::{self, Align, Button, Color32, FontId, Layout, RichText, TextEdit, Ui};
use remote_desktop::{
    protocol::{Packet, ResultPacket},
    secure_channel::SecureChannel,
    Scene, SceneChange,
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

fn receive_recordings(channel: &mut SecureChannel) -> Vec<(i32, String)> {
    let mut recordings = HashMap::new();

    loop {
        let packet = channel.receive().unwrap();

        match packet {
            Packet::None => break,

            Packet::RecordingName { id, name } => {
                recordings.insert(id, name);
            }

            _ => (),
        }
    }

    let mut recordings: Vec<(i32, String)> = recordings.into_iter().collect();

    // sort by the time (in reverse, newest one first)
    recordings.sort_by(|a, b| b.1.cmp(&a.1));

    recordings
}

pub struct MenuScene {
    session_code: String,
    status_message: String,
    is_error: bool,

    username: String,
    recordings: Vec<(i32, String)>,
    join_receiver: Option<Receiver<(bool, String)>>,
    is_disabled: bool,
}

impl MenuScene {
    pub fn new(username: String, channel: &mut SecureChannel, status_message: &str) -> Self {
        let recordings = receive_recordings(channel);

        Self {
            session_code: String::new(),
            status_message: status_message.to_string(),
            is_error: false,

            username,
            recordings,
            join_receiver: None,
            is_disabled: false,
        }
    }

    fn host_button(&self, channel: &mut SecureChannel) -> SceneChange {
        channel.send(Packet::Host).unwrap();

        let result = channel.receive().unwrap();
        let ResultPacket::Success(code) = result else {
            panic!("should be success");
        };

        SceneChange::To(Box::new(HostScene::new(
            code,
            channel,
            self.username.to_string(),
        )))
    }

    fn join_button(
        &mut self,
        session_code: u32,
        channel: &mut SecureChannel,
    ) -> Result<String, String> {
        let join_message = Packet::Join {
            code: session_code,
            username: self.username.clone(),
        };
        channel.send(join_message).unwrap();

        let result = channel.receive().unwrap();
        match result {
            ResultPacket::Failure(msg) => Err(msg),
            ResultPacket::Success(_) => {
                let mut channel = channel.clone();
                let (sender, receiver) = mpsc::channel();
                self.join_receiver = Some(receiver);
                self.is_disabled = true;

                thread::spawn(move || {
                    let result = channel.receive().unwrap();

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
    fn update(&mut self, ctx: &egui::Context, channel: &mut Option<SecureChannel>) -> SceneChange {
        let channel = channel.as_mut().unwrap();
        let mut result: SceneChange = SceneChange::None;

        if let Some(join_receiver) = &self.join_receiver {
            if let Ok((join_result, msg)) = join_receiver.try_recv() {
                self.is_disabled = false;

                match join_result {
                    true => {
                        result = SceneChange::To(Box::new(MainScene::new(
                            channel,
                            self.username.clone(),
                        )))
                    }

                    false => {
                        self.is_error = true;
                        self.status_message = msg
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
                        channel.send(Packet::SignOut).unwrap();

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

            egui::ScrollArea::vertical().show(ui, |ui| {
                for (id, recording) in &self.recordings {
                    let time: DateTime<Local> = recording.parse().unwrap();
                    let recording_display_name = time.format("%B %-d, %Y | %T").to_string();

                    if ui.button(recording_display_name).clicked() {
                        channel.send(Packet::WatchRecording { id: *id }).unwrap();

                        let result_packet = channel.receive().unwrap();

                        match result_packet {
                            ResultPacket::Failure(msg) => {
                                self.is_error = true;
                                self.status_message = msg;
                            }

                            ResultPacket::Success(duration) => {
                                let duration: i32 =
                                    duration.parse().expect("duration should be i32");
                                result = SceneChange::To(Box::new(WatchScene::new(
                                    self.username.clone(),
                                    duration,
                                    channel,
                                )));
                            }
                        }
                    }
                }
            });
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
                        Ok(session_code) => match self.join_button(session_code, channel) {
                            Ok(msg) => {
                                self.is_error = false;
                                self.status_message = msg
                            }
                            Err(msg) => {
                                self.is_error = true;
                                self.status_message = msg
                            }
                        },
                        Err(_) => {
                            self.is_error = true;
                            self.status_message =
                                "Invalid session code. Please enter a 6 digit code.".to_string()
                        }
                    }
                }

                ui.add_space(10.0);
                ui.label(
                    RichText::new(&self.status_message)
                        .size(20.0)
                        .color(if self.is_error {
                            Color32::RED
                        } else {
                            Color32::BLUE
                        }),
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
                    result = self.host_button(channel);
                }
            });
        });

        result
    }

    fn on_exit(&mut self, channel: &mut Option<SecureChannel>) {
        let channel = channel.as_mut().unwrap();

        channel.send(Packet::SignOut).unwrap();
        channel.send(Packet::Shutdown).unwrap();

        channel.close();
    }
}
