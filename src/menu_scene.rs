use std::{
    collections::HashMap,
    sync::mpsc::{self, Receiver},
    thread,
};

use chrono::{DateTime, Local};
use eframe::egui::{self, Align, Button, Color32, FontId, Layout, RichText, TextEdit, Ui};
use log::info;
use stream_desk::{
    protocol::{Packet, ResultPacket},
    secure_channel::SecureChannel,
    Scene, SceneChange, LOG_TARGET,
};

use crate::{
    host_scene::HostScene, login_scene::LoginScene, participant_scene::ParticipantScene,
    watch_scene::WatchScene,
};

/// Receives and processes a list of available recordings from the server.
///
/// This function continuously receives packets from the server until a `Packet::None`
/// is encountered, indicating the end of the recording list. It collects recording IDs
/// and names into a `HashMap` and then converts them into a sorted `Vec` of `(id, name)` tuples,
/// ordered by creation time in reverse (newest first).
///
/// # Arguments
///
/// * `channel` - A mutable reference to the `SecureChannel` for communication with the server.
///
/// # Returns
///
/// A `Vec` of `(i32, String)` tuples, where `i32` is the recording ID and `String` is the recording name (timestamp),
/// sorted by timestamp in descending order.
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

/// Represents the main menu scene of the Remote Desktop client.
///
/// In this scene, authenticated users can choose to:
/// - **Host** a new remote desktop session.
/// - **Join** an existing remote desktop session as a participant.
/// - **Watch** previously recorded sessions.
/// - **Sign out** and return to the login screen.
pub struct MenuScene {
    /// The input field for joining a session.
    session_code: String,
    /// A message displayed to the user, indicating status or errors.
    status_message: String,
    /// A flag indicating if the `status_message` represents an error.
    is_error: bool,

    /// The username of the currently logged-in user.
    username: String,
    /// A list of available recordings, with their IDs and names (timestamps).
    recordings: Vec<(i32, String)>,
    /// An optional receiver for handling asynchronous join session results.
    join_receiver: Option<Receiver<(bool, String)>>,
    /// A flag to disable UI elements when a background operation (like joining) is in progress.
    is_disabled: bool,
}

impl MenuScene {
    /// Creates a new `MenuScene` instance.
    ///
    /// Initializes the scene with the logged-in username and retrieves the list of
    /// available recordings from the server.
    ///
    /// # Arguments
    ///
    /// * `username` - The username of the logged-in client.
    /// * `channel` - A mutable reference to the `SecureChannel` for initial data retrieval (recordings).
    /// * `status_message` - An initial status message to display.
    ///
    /// # Returns
    ///
    /// A new `MenuScene` ready to be displayed.
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

    /// Handles the "Host Session" button click.
    ///
    /// Sends a `Packet::Host` request to the server. Upon a successful response
    /// (which should contain the session code), it transitions the application
    /// to the `HostScene`. Panics if the server response is not a `ResultPacket::Success`.
    ///
    /// # Arguments
    ///
    /// * `channel` - A mutable reference to the `SecureChannel` for communication with the server.
    ///
    /// # Returns
    ///
    /// A `SceneChange` variant indicating a transition to `HostScene`.
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

    /// Handles the "Join Session" button click.
    ///
    /// Sends a `Packet::Join` request to the server with the provided session code and username.
    /// It then sets up an asynchronous receiver to wait for the host's approval or rejection.
    /// The UI is disabled while waiting for the approval.
    ///
    /// # Arguments
    ///
    /// * `session_code` - The 6-digit code of the session to join.
    /// * `channel` - A mutable reference to the `SecureChannel` for communication with the server.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success (`Ok` with a waiting message) or failure (`Err` with an error message)
    /// based on the immediate server response to the join request. The final transition to `ParticipantScene`
    /// happens asynchronously via `join_receiver`.
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

                info!(target: LOG_TARGET, "User requested to join session {}.", session_code);

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
    /// Updates and renders the `MenuScene` UI for each frame.
    ///
    /// This method checks for asynchronous join results, draws the title bar,
    /// connection status, user details, recording list, and the main session
    /// joining and hosting controls. It handles user interactions and triggers
    /// scene changes as appropriate.
    ///
    /// # Arguments
    ///
    /// * `ctx` - The `egui::Context` providing access to egui's rendering and input state.
    /// * `channel` - A mutable reference to the `SecureChannel` for all server communications.
    ///
    /// # Returns
    ///
    /// A `SceneChange` enum variant, indicating whether the scene should transition
    /// to `HostScene`, `ParticipantScene`, `WatchScene`, or `LoginScene`, or remain
    /// in the `MenuScene`.
    fn update(&mut self, ctx: &egui::Context, channel: &mut SecureChannel) -> SceneChange {
        let mut result: SceneChange = SceneChange::None;

        if let Some(join_receiver) = &self.join_receiver {
            if let Ok((join_result, msg)) = join_receiver.try_recv() {
                self.is_disabled = false;

                match join_result {
                    true => {
                        info!(target: LOG_TARGET, "Joining session {}.", self.session_code);

                        result = SceneChange::To(Box::new(ParticipantScene::new(
                            channel,
                            self.username.clone(),
                        )))
                    }

                    false => {
                        info!(
                            target: LOG_TARGET,
                            "Host of session {} denied the join request.",
                            self.session_code
                        );

                        self.is_error = true;
                        self.status_message = msg
                    }
                }
            }
        }

        egui::TopBottomPanel::top("title_bar").show(ctx, |ui| {
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                ui.label(RichText::new("Stream Desk").size(40.0));
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

                        info!(target: LOG_TARGET, "User signed out.");

                        result = SceneChange::To(Box::new(LoginScene::new(None, true)));
                    }

                    ui.label(RichText::new(format!("Welcome, {}", self.username)).size(20.0));
                });

                ui.add_space(10.0);
            });

        egui::SidePanel::right("recordings_panel").show(ctx, |ui| {
            if self.is_disabled {
                ui.disable();
            }

            ui.heading("Watch past recordings");
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                for (id, recording) in &self.recordings {
                    let time: DateTime<Local> = recording.parse().unwrap();
                    let recording_display_name = time.format("%B %-d, %Y | %T").to_string();

                    if ui.button(&recording_display_name).clicked() {
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

                                info!(target: LOG_TARGET, "Watching recording {}.", &recording_display_name);

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
                ui.label(RichText::new("Join Session (6 digit code)").size(20.0));
                ui.add_space(10.0);

                ui.add(
                    TextEdit::singleline(&mut self.session_code)
                        .hint_text("Enter code")
                        .char_limit(6)
                        .font(FontId::monospace(30.)),
                );

                ui.add_space(10.0);

                let code = self.session_code.parse::<u32>();
                let can_join = match code {
                    Ok(code) => code > 100_000 && code < 1_000_000,
                    Err(_) => false,
                };

                let join_button = ui.add_enabled(can_join, |ui: &mut Ui| {
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

    /// Called when the application is exiting or transitioning away from the `MenuScene`.
    ///
    /// This method sends a `SignOut` packet, then a `Shutdown` packet to the server,
    /// and finally closes the `SecureChannel` to ensure a graceful disconnection.
    ///
    /// # Arguments
    ///
    /// * `channel` - A mutable reference to the `SecureChannel` to be closed.
    fn on_exit(&mut self, channel: &mut SecureChannel) {
        channel.send(Packet::SignOut).unwrap();
        channel.send(Packet::Shutdown).unwrap();

        channel.close();
    }
}
