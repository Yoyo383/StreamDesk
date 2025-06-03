use std::sync::mpsc::Receiver;

use eframe::egui::{self, Align, Color32, FontId, Layout, RichText, SelectableLabel, TextEdit};
use log::info;
use stream_desk::{
    protocol::{Packet, ResultPacket},
    secure_channel::SecureChannel,
    Scene, SceneChange, LOG_TARGET,
};

use crate::menu_scene::MenuScene;

/// Represents the login and registration user interface scene.
///
/// This scene handles user input for both logging in and registering new accounts,
/// communicating with the server via a `SecureChannel`. It also manages the connection
/// status to the server and displays relevant feedback to the user.
pub struct LoginScene {
    // Fields for login form
    login_username: String,
    login_password: String,
    // Fields for registration form
    register_username: String,
    register_password: String,
    register_confirm_password: String,

    // Communication and connection status
    socket_receiver: Option<Receiver<Option<SecureChannel>>>,
    connected_to_server: bool,
    failed_to_connect: bool,

    // Error messages displayed to the user
    error_message_login: String,
    error_message_register: String,

    /// A boolean flag to switch between the login and registration forms.
    is_login: bool,
}

impl LoginScene {
    /// Creates a new `LoginScene` instance.
    ///
    /// Initializes all input fields, sets the initial connection status,
    /// and prepares for receiving the `SecureChannel` once connected.
    ///
    /// # Arguments
    ///
    /// * `socket_receiver` - An `Option<Receiver<Option<SecureChannel>>>` to receive the established
    ///                       secure channel from a background connection thread. `None` if connection
    ///                       is already handled or not asynchronous.
    /// * `connected_to_server` - A boolean indicating whether a connection to the server
    ///                           is already established or is pending.
    ///
    /// # Returns
    ///
    /// A new `LoginScene` ready to be displayed.
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

    /// Attempts to log in a user with the provided credentials.
    ///
    /// This method sends a `Login` packet to the server, hashes the password using MD5,
    /// and processes the server's `ResultPacket`. If successful, it transitions to the `MenuScene`.
    /// Otherwise, it displays an error message.
    ///
    /// # Arguments
    ///
    /// * `channel` - A mutable reference to the `SecureChannel` for communication with the server.
    ///
    /// # Returns
    ///
    /// A `SceneChange` enum variant indicating whether to stay in the current scene (`SceneChange::None`)
    /// or transition to the `MenuScene` upon successful login.
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

            ResultPacket::Success(_) => {
                info!(target: LOG_TARGET, "Logged in as {}.", self.login_username);

                SceneChange::To(Box::new(MenuScene::new(
                    self.login_username.clone(),
                    channel,
                    "",
                )))
            }
        }
    }

    /// Attempts to register a new user with the provided credentials.
    ///
    /// This method validates the input fields (password match, username validity, length),
    /// sends a `Register` packet to the server (with MD5 hashed password), and processes
    /// the server's `ResultPacket`. If successful, it transitions to the `MenuScene`.
    /// Otherwise, it displays an error message.
    ///
    /// # Arguments
    ///
    /// * `channel` - A mutable reference to the `SecureChannel` for communication with the server.
    ///
    /// # Returns
    ///
    /// A `SceneChange` enum variant indicating whether to stay in the current scene (`SceneChange::None`)
    /// or transition to the `MenuScene` upon successful registration.
    fn register(&mut self, channel: &mut SecureChannel) -> SceneChange {
        // Basic input validation
        if self.register_password != self.register_confirm_password {
            self.error_message_register = "Passwords do not match.".to_string();
            return SceneChange::None;
        }

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

            ResultPacket::Success(_) => {
                info!(target: LOG_TARGET, "Registered as {}.", self.register_username);

                SceneChange::To(Box::new(MenuScene::new(
                    self.register_username.clone(),
                    channel,
                    "",
                )))
            }
        }
    }
}

impl Scene for LoginScene {
    /// Updates the `LoginScene`'s UI and logic for each frame.
    ///
    /// This includes checking for incoming `SecureChannel` from the background thread,
    /// displaying connection status, and rendering the login/registration forms.
    /// It handles user interactions like button clicks and input changes,
    /// triggering `login` or `register` methods as needed.
    ///
    /// # Arguments
    ///
    /// * `ctx` - The `egui::Context` providing access to egui's rendering and input state.
    /// * `channel` - A mutable reference to the `SecureChannel` for potential updates
    ///               from the connection thread and for sending authentication packets.
    ///
    /// # Returns
    ///
    /// A `SceneChange` enum variant, indicating whether the scene should transition
    /// to `MenuScene` (on successful authentication) or remain in the `LoginScene`.
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
                ui.label(RichText::new("StreamDesk").size(40.0));
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
                            .hint_text("Confirm password")
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

    /// Called when the application is exiting or transitioning away from the `LoginScene`.
    ///
    /// This method sends a `Shutdown` packet to the server and closes the `SecureChannel`,
    /// ensuring a graceful disconnection.
    ///
    /// # Arguments
    ///
    /// * `channel` - A mutable reference to the `SecureChannel` to be closed.
    fn on_exit(&mut self, channel: &mut SecureChannel) {
        channel.send(Packet::Shutdown).unwrap();
        channel.close();
    }
}
