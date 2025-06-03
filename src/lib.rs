use core::f32;
use std::{collections::HashMap, path::Path, sync::MutexGuard};

use eframe::egui::{
    self,
    text::{LayoutJob, TextWrapping},
    Color32, FontId, Key, Pos2, Rect, RichText, ScrollArea, TextFormat, Ui,
};
use ftail::Ftail;
use protocol::Packet;
use secure_channel::SecureChannel;

pub mod protocol;
pub mod secure_channel;

pub const LOG_TARGET: &'static str = "stream-desk";
pub const LOG_DIR: &'static str = "logs";
pub const SERVER_LOG_FILE: &'static str = "server.log";
pub const CLIENT_LOG_FILE: &'static str = "client.log";

/// Initializes the logger.
///
/// # Arguments
///
/// * `log_file` - The file to log to.
pub fn initialize_logger(log_file: &str) {
    let _ = Ftail::new()
        .console(log::LevelFilter::Info)
        .single_file(
            &Path::new(LOG_DIR).join(log_file),
            true,
            log::LevelFilter::Info,
        )
        .filter_targets(vec![LOG_TARGET])
        .init();
}

/// Maps an egui `Key` to a Windows virtual key code.
///
/// This function is primarily used for converting egui keyboard input
/// into a format compatible with Windows API functions that expect
/// virtual key codes (e.g., for simulating key presses).
///
/// # Arguments
///
/// * `key` - A reference to an `egui::Key` enum variant.
///
/// # Returns
///
/// An `Option<u16>` which is:
/// - `Some(u16)` containing the corresponding Windows virtual key code if a mapping exists.
/// - `None` if the provided `egui::Key` does not have a direct virtual key code equivalent
///   or is not supported by this mapping.
pub fn egui_key_to_vk(key: &Key) -> Option<u16> {
    use winapi::um::winuser::*;
    use Key::*;
    Some(match key {
        ArrowDown => VK_DOWN as u16,
        ArrowLeft => VK_LEFT as u16,
        ArrowRight => VK_RIGHT as u16,
        ArrowUp => VK_UP as u16,
        Escape => VK_ESCAPE as u16,
        Tab => VK_TAB as u16,
        Backspace => VK_BACK as u16,
        Enter => VK_RETURN as u16,
        Space => VK_SPACE as u16,
        Insert => VK_INSERT as u16,
        Delete => VK_DELETE as u16,
        Home => VK_HOME as u16,
        End => VK_END as u16,
        PageUp => VK_PRIOR as u16,
        PageDown => VK_NEXT as u16,
        A => 0x41,
        B => 0x42,
        C => 0x43,
        D => 0x44,
        E => 0x45,
        F => 0x46,
        G => 0x47,
        H => 0x48,
        I => 0x49,
        J => 0x4A,
        K => 0x4B,
        L => 0x4C,
        M => 0x4D,
        N => 0x4E,
        O => 0x4F,
        P => 0x50,
        Q => 0x51,
        R => 0x52,
        S => 0x53,
        T => 0x54,
        U => 0x55,
        V => 0x56,
        W => 0x57,
        X => 0x58,
        Y => 0x59,
        Z => 0x5A,
        Num0 => 0x30,
        Num1 => 0x31,
        Num2 => 0x32,
        Num3 => 0x33,
        Num4 => 0x34,
        Num5 => 0x35,
        Num6 => 0x36,
        Num7 => 0x37,
        Num8 => 0x38,
        Num9 => 0x39,
        F1 => VK_F1 as u16,
        F2 => VK_F2 as u16,
        F3 => VK_F3 as u16,
        F4 => VK_F4 as u16,
        F5 => VK_F5 as u16,
        F6 => VK_F6 as u16,
        F7 => VK_F7 as u16,
        F8 => VK_F8 as u16,
        F9 => VK_F9 as u16,
        F10 => VK_F10 as u16,
        F11 => VK_F11 as u16,
        F12 => VK_F12 as u16,
        F13 => VK_F13 as u16,
        F14 => VK_F14 as u16,
        F15 => VK_F15 as u16,
        F16 => VK_F16 as u16,
        F17 => VK_F17 as u16,
        F18 => VK_F18 as u16,
        F19 => VK_F19 as u16,
        F20 => VK_F20 as u16,
        Minus => VK_OEM_MINUS as u16,
        Plus => VK_OEM_PLUS as u16,
        Equals => VK_OEM_PLUS as u16, // Same as Plus in Windows
        Comma => VK_OEM_COMMA as u16,
        Period => VK_OEM_PERIOD as u16,
        Slash => VK_OEM_2 as u16, // Forward slash
        Backslash => VK_OEM_5 as u16,
        Colon => VK_OEM_1 as u16, // Actually semicolon on US layout
        Semicolon => VK_OEM_1 as u16,
        Quote => VK_OEM_7 as u16,
        OpenBracket => VK_OEM_4 as u16,
        CloseBracket => VK_OEM_6 as u16,
        Backtick => VK_OEM_3 as u16,
        _ => return None,
    })
}

/// Normalizes the mouse position from egui coordinates to a range suitable
/// for remote control (0 to 65,535).
///
/// This is typically used to translate screen coordinates into a format
/// that can be used by an operating system's input simulation APIs, which
/// often expect normalized values regardless of screen resolution.
///
/// # Arguments
///
/// * `mouse_position` - The current mouse cursor position in egui's screen coordinates.
/// * `image_rect` - The `Rect` representing the area where the image is displayed.
///                  This is used to correctly scale the mouse position relative to the image.
///
/// # Returns
///
/// A tuple `(u32, u32)` where:
/// - The first `u32` is the normalized X coordinate (0 to 65,535).
/// - The second `u32` is the normalized Y coordinate (0 to 65,535).
pub fn normalize_mouse_position(mouse_position: Pos2, image_rect: Rect) -> (u32, u32) {
    let x = (mouse_position.x - image_rect.left()) * 65535.0 / image_rect.width();
    let y = (mouse_position.y - image_rect.top()) * 65535.0 / image_rect.height();
    (x as u32, y as u32)
}

/// Displays the list of connected users and their roles (Host, Controller, Participant).
///
/// If the current user is the **host**, a "Revoke Control" button will appear next to
/// the active controller. Pressing this button will revoke control from that user.
///
/// # Arguments
///
/// * `ui` - A mutable reference to the `egui::Ui` where the list will be drawn.
/// * `usernames` - A `MutexGuard` containing a `HashMap` mapping usernames to their `UserType`.
/// * `username` - The current user's own username, used to display "(You)" next to their name.
/// * `is_host` - A boolean indicating whether the current user is the host of the session.
///
/// # Returns
///
/// An `Option<String>`:
/// - `Some(controller_username)` if the "Revoke Control" button for a specific controller was pressed.
/// - `None` if no "Revoke Control" button was pressed or if the current user is not the host.
pub fn users_list(
    ui: &mut Ui,
    usernames: MutexGuard<HashMap<String, UserType>>,
    username: String,
    is_host: bool,
) -> Option<String> {
    let mut result: Option<String> = None;

    let mut hosts = Vec::new();
    let mut controllers = Vec::new();
    let mut participants = Vec::new();

    // Categorize users by their type
    for (username, user_type) in usernames.iter() {
        match user_type {
            UserType::Host => hosts.push(username.clone()),
            UserType::Controller => controllers.push(username.clone()),
            UserType::Participant => participants.push(username.clone()),
            UserType::Leaving => (), // 'Leaving' users are not displayed in the active list
        }
    }

    // Display Hosts
    ui.heading("Host");
    for host in hosts.iter() {
        if *host == username {
            ui.label(format!("{} (You)", host));
        } else {
            ui.label(host);
        }
    }

    // Display Controllers, with "Revoke Control" button for the host
    if !controllers.is_empty() {
        ui.add_space(10.0);

        ui.heading("Controller");
        for controller in controllers.iter() {
            ui.horizontal(|ui| {
                if *controller == username {
                    ui.label(format!("{} (You)", controller));
                } else {
                    ui.label(controller);
                }

                if is_host {
                    if ui.button("Revoke Control (Ctrl+Shift+R)").clicked() {
                        result = Some(controller.clone());
                    }
                }
            });
        }
    }

    // Display Participants
    if !participants.is_empty() {
        ui.add_space(10.0);

        ui.heading("Participants");
        for participant in participants.iter() {
            if *participant == username {
                ui.label(format!("{} (You)", participant));
            } else {
                ui.label(participant);
            }
        }
    }

    result
}

/// Displays the chat user interface, including the chat log and an input field
/// for sending new messages.
///
/// When the "Send" button is clicked and the message is not empty, a `Chat` packet
/// is constructed and sent over the provided `SecureChannel`.
///
/// Messages in the chat log can optionally include special formatting directives:
/// - `#r` for red text
/// - `#g` for green text
/// - `#b` for blue text
///
/// Messages are displayed with the username in a distinct color if they follow the "Username: Message" format.
///
/// # Arguments
///
/// * `ui` - A mutable reference to the `egui::Ui` where the chat UI will be drawn.
/// * `chat_log` - A `MutexGuard` containing a `Vec<String>` representing the chat history.
/// * `message` - A mutable reference to the `String` holding the current message being typed in the input field.
/// * `channel` - A mutable reference to the `SecureChannel` used for sending chat packets.
///
/// # Panics
///
/// Panics if the `channel.send()` operation fails, as `unwrap()` is used.
pub fn chat_ui(
    ui: &mut Ui,
    chat_log: MutexGuard<Vec<String>>,
    message: &mut String,
    channel: &mut SecureChannel,
) {
    ui.heading("Chat");
    ui.separator();

    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                // Send button logic
                if ui.button("Send").clicked() && !message.is_empty() {
                    let chat_packet = Packet::Chat {
                        message: message.to_string(),
                    };
                    // Send the chat packet over the secure channel. Panics on error.
                    channel.send(chat_packet).unwrap();

                    message.clear(); // Clear the input field after sending
                }

                // Text input field for typing messages
                ui.add(egui::TextEdit::singleline(message).desired_width(f32::INFINITY));
            });
        });

        ui.add_space(10.0);

        // Scrollable area for displaying chat messages
        ScrollArea::vertical()
            .stick_to_bottom(true) // Keeps the scrollbar at the bottom
            .show(ui, |ui: &mut Ui| {
                // Iterate through messages in reverse to show newest at the bottom
                for message in chat_log.iter().rev() {
                    let mut message = message.clone();
                    let mut text_color = Color32::WHITE;

                    // Check for special color directives (e.g., #r, #g, #b)
                    if let Some(first_char) = message.chars().nth(0) {
                        if first_char == '#' && message.len() > 1 {
                            let color_char = message.chars().nth(1).unwrap();

                            // Set color based on the directive
                            text_color = match color_char {
                                'r' => Color32::RED,
                                'g' => Color32::GREEN,
                                'b' => Color32::BLUE,
                                _ => Color32::WHITE, // Default if unknown directive
                            };

                            // Remove the color directive from the message
                            message.drain(..2);
                        }
                    }

                    // Attempt to parse message as "Username: Message" format
                    if let Some((username, content)) = message.split_once(": ") {
                        let mut job = LayoutJob::default();

                        // Style for the username (dynamic color)
                        let username_format = TextFormat {
                            font_id: FontId::proportional(14.0),
                            color: text_color,
                            ..Default::default()
                        };
                        job.append(&format!("{}: ", username), 0.0, username_format);

                        // Style for the message content (default color)
                        let message_format = TextFormat {
                            font_id: FontId::proportional(14.0),
                            color: Color32::WHITE, // Message content is always white
                            ..Default::default()
                        };
                        job.append(content, 0.0, message_format);

                        // Enable wrapping for long messages
                        job.wrap = TextWrapping {
                            max_width: ui.available_width(),
                            ..Default::default()
                        };

                        ui.label(job);
                    } else {
                        // Fallback for messages without "Username: Message" format
                        ui.add(
                            egui::Label::new(RichText::new(message.clone()).color(text_color))
                                .wrap(),
                        );
                    }
                }
            });
    });
}

/// Represents a requested scene change within the application.
///
/// This enum is used by the `Scene` trait to indicate whether the current scene
/// should be replaced by another, or if it should remain active.
pub enum SceneChange {
    /// Indicates that the current scene should not change.
    None,
    /// Indicates that the scene should transition to a new one.
    /// The `Box<dyn Scene>` holds a trait object of the new scene.
    To(Box<dyn Scene>),
}

/// Base scene trait. All application scenes must implement this trait.
///
/// Scenes are responsible for their own logic (`update`) and for handling
/// cleanup when they are exited (`on_exit`).
pub trait Scene {
    /// Updates the scene's logic and renders its UI components for a single frame.
    ///
    /// This function is called repeatedly by the application's main loop.
    /// It handles user input, updates internal state, and draws elements to the screen.
    ///
    /// # Arguments
    ///
    /// * `ctx` - The `egui::Context` which provides access to rendering and input functionalities.
    /// * `channel` - A mutable reference to the `SecureChannel`, used for network communication
    ///               within the scene (e.g., sending/receiving packets).
    ///
    /// # Returns
    ///
    /// A `SceneChange` enum variant indicating whether to stay in the current scene (`SceneChange::None`)
    /// or transition to a new one (`SceneChange::To(new_scene)`).
    fn update(&mut self, ctx: &egui::Context, channel: &mut SecureChannel) -> SceneChange;

    /// Called when the application is exiting or when the scene is being transitioned away from.
    ///
    /// This function should perform any necessary cleanup, such as closing network connections,
    /// saving state, or releasing resources specific to this scene.
    ///
    /// # Arguments
    ///
    /// * `channel` - A mutable reference to the `SecureChannel`, allowing the scene to close
    ///               network connections if necessary.
    fn on_exit(&mut self, channel: &mut SecureChannel);
}

/// Represents the role or type of a user within the application.
///
/// This enum is used to categorize users based on their privileges and status
/// within a collaborative session (e.g., remote desktop).
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum UserType {
    /// The user is currently in the process of leaving the session.
    Leaving,
    /// The user is the host of the session, typically having full control.
    Host,
    /// The user has control over the remote desktop.
    Controller,
    /// The user is a participant viewing the session but without control.
    Participant,
}

impl std::fmt::Display for UserType {
    /// Implements the `Display` trait for `UserType`, allowing it to be
    /// formatted into a string.
    ///
    /// # Arguments
    ///
    /// * `f` - The formatter to which the string representation is written.
    ///
    /// # Returns
    ///
    /// A `std::fmt::Result` indicating success or failure of the formatting.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            UserType::Host => "Host",
            UserType::Controller => "Controller",
            UserType::Participant => "Participant",
            UserType::Leaving => "Leaving",
        };

        write!(f, "{}", str)
    }
}
