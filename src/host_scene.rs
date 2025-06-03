use eframe::egui;
use h264_reader::{
    annexb::AnnexBReader,
    nal::{Nal, RefNal},
    push::NalInterest,
};
use log::info;
use remote_desktop::{
    chat_ui, protocol::ControlPayload, secure_channel::SecureChannel, users_list, Scene,
    SceneChange, UserType, LOG_TARGET,
};

use eframe::egui::PointerButton;
use remote_desktop::protocol::Packet;
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    process::{Child, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};
use winapi::um::winuser::{
    self, SendInput, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, MOUSEINPUT, WHEEL_DELTA,
};

use crate::menu_scene::MenuScene;

/// Starts FFmpeg process to capture desktop screen as H.264 stream
///
/// This function launches FFmpeg with optimized settings for real-time screen sharing:
/// - Uses gdigrab for Windows desktop capture
/// - 30 FPS framerate for smooth playback
/// - Ultrafast preset with zero latency tuning for minimal delay
/// - H.264 encoding with no scene cut detection for consistent streaming
///
/// # Returns
///
/// A `Child` process handle for the running FFmpeg instance
///
/// # Panics
///
/// Panics if FFmpeg cannot be started (e.g., FFmpeg not installed or not in PATH)
fn start_ffmpeg() -> Child {
    let ffmpeg = Command::new("ffmpeg")
        .args(&[
            "-f",
            "gdigrab",
            "-framerate",
            "30",
            "-draw_mouse",
            "0",
            "-i",
            "desktop",
            "-vcodec",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-g",
            "60",
            "-x264opts",
            "no-scenecut",
            "-sc_threshold",
            "0",
            "-f",
            "h264",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start FFmpeg");

    ffmpeg
}

/// Background thread for reading FFmpeg output and sending screen data to clients
///
/// This function creates a thread that:
/// 1. Reads H.264 NAL units from FFmpeg stdout
/// 2. Processes each complete NAL unit using AnnexBReader
/// 3. Sends screen packets to connected clients via secure channel
/// 4. Continues until stop flag is set
///
/// # Arguments
///
/// * `channel` - Secure communication channel for sending packets
/// * `stdout` - FFmpeg process stdout handle for reading video data
/// * `stop_flag` - Atomic boolean to signal thread termination
///
/// # Returns
///
/// A `JoinHandle` for the spawned thread
fn thread_send_screen(
    mut channel: SecureChannel,
    mut stdout: ChildStdout,
    stop_flag: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = AnnexBReader::accumulate(|nal: RefNal<'_>| {
            if !nal.is_complete() {
                return NalInterest::Buffer; // not ready yet
            }

            if nal.header().is_err() {
                return NalInterest::Ignore;
            }

            // sending the NAL (with the start)
            let mut nal_bytes: Vec<u8> = vec![0x00, 0x00, 0x01];
            nal.reader()
                .read_to_end(&mut nal_bytes)
                .expect("should be able to read NAL");

            channel.send(Packet::Screen { bytes: nal_bytes }).unwrap();

            NalInterest::Ignore
        });

        let mut buffer = [0u8; 4096];

        while !stop_flag.load(Ordering::Relaxed) {
            match stdout.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    reader.push(&buffer[..n]);
                }
                Err(e) => {
                    eprintln!("ffmpeg read error: {}", e);
                    break;
                }
            }
        }
    })
}

/// Background thread for handling incoming network packets from clients
///
/// This function processes various packet types:
/// - Join requests from new users
/// - User status updates (joining/leaving/role changes)
/// - Control packets (mouse/keyboard input from controllers)
/// - Control requests from viewers
/// - Chat messages
/// - Session end signals
///
/// # Arguments
///
/// * `channel` - Secure communication channel for receiving packets
/// * `usernames` - Shared map of connected users and their roles
/// * `requesting_control` - Set of users requesting control permissions
/// * `requesting_join` - Set of users requesting to join the session
/// * `chat_log` - Shared chat message history
///
/// # Returns
///
/// A `JoinHandle` for the spawned thread
fn thread_read_socket(
    mut channel: SecureChannel,
    usernames: Arc<Mutex<HashMap<String, UserType>>>,
    requesting_control: Arc<Mutex<HashSet<String>>>,
    requesting_join: Arc<Mutex<HashSet<String>>>,
    chat_log: Arc<Mutex<Vec<String>>>,
) -> JoinHandle<()> {
    thread::spawn(move || loop {
        let packet = channel.receive().unwrap_or_default();

        match packet {
            Packet::Join { username, .. } => {
                let mut requesting_join = requesting_join.lock().unwrap();
                requesting_join.insert(username);
            }

            Packet::UserUpdate {
                user_type,
                username,
                ..
            } => {
                let mut usernames = usernames.lock().unwrap();
                if user_type == UserType::Leaving {
                    usernames.remove(&username);

                    let mut chat_log = chat_log.lock().unwrap();
                    chat_log.push(format!("#r{} has disconnected.", username));
                } else {
                    let mut chat_log = chat_log.lock().unwrap();

                    if usernames.contains_key(&username) {
                        chat_log.push(format!("#b{} is now a {}.", username, user_type));
                    } else {
                        chat_log.push(format!("#g{} has joined the session.", username));
                    }

                    usernames.insert(username.clone(), user_type);
                }
            }

            Packet::Control { payload } => match payload {
                ControlPayload::MouseMove { mouse_x, mouse_y } => send_mouse_move(mouse_x, mouse_y),

                ControlPayload::MouseClick {
                    mouse_x,
                    mouse_y,
                    pressed,
                    button,
                } => send_mouse_click(mouse_x, mouse_y, button, pressed),

                ControlPayload::Keyboard { pressed, key } => send_key(key, pressed),

                ControlPayload::Scroll { delta } => send_scroll(delta),
            },

            Packet::RequestControl { username } => {
                let mut requesting_control = requesting_control.lock().unwrap();
                requesting_control.insert(username);
            }

            Packet::Chat { message } => {
                let mut chat_log = chat_log.lock().unwrap();
                chat_log.push(message);
            }

            Packet::SessionEnd => break,

            _ => (),
        }
    })
}

/// Sends mouse movement input to the host system
///
/// Uses Windows API to simulate mouse cursor movement at absolute coordinates.
/// The coordinates are treated as absolute positions on the screen.
///
/// # Arguments
///
/// * `mouse_x` - Absolute X coordinate for mouse position
/// * `mouse_y` - Absolute Y coordinate for mouse position
///
/// # Safety
///
/// This function uses unsafe Windows API calls to inject input events
fn send_mouse_move(mouse_x: u32, mouse_y: u32) {
    unsafe {
        let mut move_input: INPUT = std::mem::zeroed();
        move_input.type_ = INPUT_MOUSE;
        *move_input.u.mi_mut() = MOUSEINPUT {
            dx: mouse_x as i32,
            dy: mouse_y as i32,
            mouseData: 0,
            dwFlags: winuser::MOUSEEVENTF_ABSOLUTE | winuser::MOUSEEVENTF_MOVE,
            time: 0,
            dwExtraInfo: 0,
        };

        let mut inputs = [move_input];
        SendInput(
            inputs.len() as u32,
            inputs.as_mut_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

/// Sends mouse click input to the host system
///
/// Simulates mouse button press/release events at specified coordinates.
/// Supports primary (left), secondary (right), and middle mouse buttons.
///
/// # Arguments
///
/// * `mouse_x` - Absolute X coordinate for click position
/// * `mouse_y` - Absolute Y coordinate for click position  
/// * `button` - Which mouse button to simulate (Primary/Secondary/Middle)
/// * `pressed` - Whether this is a button press (`true`) or release (`false`)
///
/// # Safety
///
/// This function uses unsafe Windows API calls to inject input events
fn send_mouse_click(mouse_x: u32, mouse_y: u32, button: PointerButton, pressed: bool) {
    unsafe {
        let mut flags: u32 = winuser::MOUSEEVENTF_ABSOLUTE | winuser::MOUSEEVENTF_MOVE;
        if button == PointerButton::Primary {
            if pressed {
                flags |= winuser::MOUSEEVENTF_LEFTDOWN;
            } else {
                flags |= winuser::MOUSEEVENTF_LEFTUP;
            }
        } else if button == PointerButton::Secondary {
            if pressed {
                flags |= winuser::MOUSEEVENTF_RIGHTDOWN;
            } else {
                flags |= winuser::MOUSEEVENTF_RIGHTUP;
            }
        } else if button == PointerButton::Middle {
            if pressed {
                flags |= winuser::MOUSEEVENTF_MIDDLEDOWN;
            } else {
                flags |= winuser::MOUSEEVENTF_MIDDLEUP;
            }
        }

        let mut click_up_input: INPUT = std::mem::zeroed();

        click_up_input.type_ = INPUT_MOUSE;
        *click_up_input.u.mi_mut() = MOUSEINPUT {
            dx: mouse_x as i32,
            dy: mouse_y as i32,
            mouseData: 0,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        };

        let mut inputs = [click_up_input];
        SendInput(
            inputs.len() as u32,
            inputs.as_mut_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

/// Sends mouse scroll wheel input to the host system
///
/// Simulates vertical scrolling with the specified delta value.
/// Positive delta scrolls up, negative delta scrolls down.
///
/// # Arguments
///
/// * `delta` - Scroll amount and direction (positive = up, negative = down)
///
/// # Safety
///
/// This function uses unsafe Windows API calls to inject input events
fn send_scroll(delta: i32) {
    unsafe {
        let mut scroll_input: INPUT = std::mem::zeroed();
        scroll_input.type_ = INPUT_MOUSE;
        *scroll_input.u.mi_mut() = MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: (delta * WHEEL_DELTA as i32) as u32,
            dwFlags: winuser::MOUSEEVENTF_WHEEL,
            time: 0,
            dwExtraInfo: 0,
        };

        let mut inputs = [scroll_input];
        SendInput(
            inputs.len() as u32,
            inputs.as_mut_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

/// Sends keyboard input to the host system
///
/// Simulates key press or release events using Windows virtual key codes.
///
/// # Arguments
///
/// * `key` - Windows virtual key code for the key to simulate
/// * `pressed` - Whether this is a key press (`true`) or release (`false`)
///
/// # Safety
///
/// This function uses unsafe Windows API calls to inject input events
fn send_key(key: u16, pressed: bool) {
    unsafe {
        let mut key_input: INPUT = std::mem::zeroed();
        key_input.type_ = INPUT_KEYBOARD;
        *key_input.u.ki_mut() = KEYBDINPUT {
            wVk: key,
            wScan: 0,
            dwFlags: if pressed { 0 } else { winuser::KEYEVENTF_KEYUP },
            time: 0,
            dwExtraInfo: 0,
        };

        let mut inputs = [key_input];
        SendInput(
            inputs.len() as u32,
            inputs.as_mut_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

/// Main scene struct for hosting a remote desktop session
///
/// The `HostScene` manages the entire hosting experience including:
/// - Screen capture and streaming via FFmpeg
/// - User management and permissions
/// - Control request handling
/// - Chat functionality
/// - Session lifecycle management
///
/// This scene runs background threads for screen capture and network communication
/// while presenting a GUI for session management.
pub struct HostScene {
    /// Unique session code that clients use to join
    session_code: String,
    /// Flag to signal background threads to stop
    stop_flag: Arc<AtomicBool>,

    /// Map of connected users and their roles (Host/Controller/Viewer)
    usernames: Arc<Mutex<HashMap<String, UserType>>>,
    /// Username of the session host
    username: String,
    /// Set of users requesting control permissions
    requesting_control: Arc<Mutex<HashSet<String>>>,
    /// Set of users requesting to join the session
    requesting_join: Arc<Mutex<HashSet<String>>>,

    /// Shared chat message history
    chat_log: Arc<Mutex<Vec<String>>>,
    /// Current chat message being composed
    chat_message: String,

    /// FFmpeg process handle for screen capture
    ffmpeg_command: Child,
    /// Background thread handle for screen streaming
    thread_send_screen: Option<JoinHandle<()>>,
    /// Background thread handle for network communication
    thread_read_socket: Option<JoinHandle<()>>,
}

impl HostScene {
    /// Creates a new host scene and starts screen sharing
    ///
    /// This constructor:
    /// 1. Starts FFmpeg for screen capture
    /// 2. Spawns background threads for streaming and network handling
    /// 3. Initializes user management structures
    /// 4. Sets up chat functionality
    ///
    /// # Arguments
    ///
    /// * `session_code` - Unique code for this session
    /// * `channel` - Secure communication channel to clients
    /// * `username` - Host's username
    ///
    /// # Returns
    ///
    /// A new `HostScene` instance ready for use
    pub fn new(session_code: String, channel: &mut SecureChannel, username: String) -> Self {
        let mut command = start_ffmpeg();
        let stdout = command.stdout.take().unwrap();

        let stop_flag = Arc::new(AtomicBool::new(false));

        let thread_send_screen = thread_send_screen(channel.clone(), stdout, stop_flag.clone());

        let mut usernames_types = HashMap::new();
        usernames_types.insert(username.clone(), UserType::Host);

        let usernames = Arc::new(Mutex::new(usernames_types));
        let requesting_control = Arc::new(Mutex::new(HashSet::new()));
        let requesting_join = Arc::new(Mutex::new(HashSet::new()));

        let chat_log = Arc::new(Mutex::new(Vec::new()));

        let thread_read_socket = thread_read_socket(
            channel.clone(),
            usernames.clone(),
            requesting_control.clone(),
            requesting_join.clone(),
            chat_log.clone(),
        );

        Self {
            session_code,
            stop_flag,

            usernames,
            username,
            requesting_control,
            requesting_join,

            chat_log,
            chat_message: String::new(),

            ffmpeg_command: command,
            thread_send_screen: Some(thread_send_screen),
            thread_read_socket: Some(thread_read_socket),
        }
    }

    /// Cleanly disconnects and shuts down the hosting session
    ///
    /// This method:
    /// 1. Signals background threads to stop
    /// 2. Waits for screen streaming thread to finish
    /// 3. Terminates FFmpeg process
    /// 4. Sends `SessionExit` message
    /// 5. Waits for network thread to finish
    /// 6. Returns to the main menu
    ///
    /// # Arguments
    ///
    /// * `channel` - Communication channel to send exit notifications
    ///
    /// # Returns
    ///
    /// A `SceneChange` to transition back to the menu scene
    fn disconnect(&mut self, channel: &mut SecureChannel) -> SceneChange {
        self.stop_flag.store(true, Ordering::Relaxed);

        let _ = self.thread_send_screen.take().unwrap().join();
        let _ = self.ffmpeg_command.kill();

        channel.send(Packet::SessionExit).unwrap();

        let _ = self.thread_read_socket.take().unwrap().join();

        SceneChange::To(Box::new(MenuScene::new(self.username.clone(), channel, "")))
    }
}

impl Scene for HostScene {
    /// Updates the host scene GUI and handles user interactions
    ///
    /// Renders three main panels:
    /// 1. **Right Panel**: Chat interface for communication
    /// 2. **Left Panel**: Control and join request management
    /// 3. **Central Panel**: Session info, user list, and session controls
    ///
    /// Also handles the keyboard shortcut Ctrl+Shift+R to revoke control.
    ///
    /// # Arguments
    ///
    /// * `ctx` - egui context for rendering GUI
    /// * `channel` - Communication channel for sending packets
    ///
    /// # Returns
    ///
    /// A `SceneChange` indicating any scene transitions needed
    fn update(&mut self, ctx: &egui::Context, channel: &mut SecureChannel) -> SceneChange {
        let mut result: SceneChange = SceneChange::None;

        egui::SidePanel::right("chat").show(ctx, |ui| {
            chat_ui(
                ui,
                self.chat_log.lock().unwrap(),
                &mut self.chat_message,
                channel,
            );
        });

        egui::SidePanel::left("requests").show(ctx, |ui| {
            let mut requesting_control = self.requesting_control.lock().unwrap();
            let mut user_handled = String::new();
            let mut was_allowed = false;

            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Control Requests");
                ui.separator();

                for user in requesting_control.iter() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} is requesting control.", user));

                        if ui.button("Allow").clicked() {
                            user_handled = user.to_string();
                            was_allowed = true;
                        }

                        if ui.button("Deny").clicked() {
                            user_handled = user.to_string();

                            let deny_packet = Packet::DenyControl {
                                username: user.to_string(),
                            };
                            channel.send(deny_packet).unwrap();
                        }
                    });
                }

                if !user_handled.is_empty() {
                    requesting_control.remove(&user_handled);
                }

                if was_allowed {
                    // find current controller
                    let usernames = self.usernames.lock().unwrap();
                    let controller = usernames
                        .iter()
                        .find(|(_, user_type)| **user_type == UserType::Controller);

                    // if found controller send Deny
                    if let Some((controller, _)) = controller {
                        let deny_packet = Packet::DenyControl {
                            username: controller.to_string(),
                        };
                        channel.send(deny_packet).unwrap();

                        info!(
                            target: LOG_TARGET,
                            "User {} is no longer the Controller of the session.",
                            controller
                        );
                    }

                    // send to allowed user
                    let allow_packet = Packet::RequestControl {
                        username: user_handled.to_string(),
                    };
                    channel.send(allow_packet).unwrap();

                    info!(
                        target: LOG_TARGET,
                        "User {} is now the Controller of the session.",
                        user_handled
                    );

                    // send Deny to all other users and clear
                    for user in requesting_control.iter() {
                        let deny_packet = Packet::DenyControl {
                            username: user.to_string(),
                        };
                        channel.send(deny_packet).unwrap();
                    }

                    // clear requesting users
                    requesting_control.clear();
                }

                // requesting join
                let mut requesting_join = self.requesting_join.lock().unwrap();
                let mut user_handled = String::new();

                ui.add_space(20.0);
                ui.heading("Join Requests");
                ui.separator();

                for user in requesting_join.iter() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} is requesting to join.", user));

                        if ui.button("Allow").clicked() {
                            user_handled = user.to_string();

                            let join_packet = Packet::Join {
                                code: 0,
                                username: user.to_string(),
                            };
                            channel.send(join_packet).unwrap();

                            info!(target: LOG_TARGET, "User {} has joined the session.", user);
                        }

                        if ui.button("Deny").clicked() {
                            user_handled = user.to_string();

                            let deny_packet = Packet::DenyJoin {
                                username: user.to_string(),
                            };
                            channel.send(deny_packet).unwrap();
                        }
                    });
                }

                if !user_handled.is_empty() {
                    requesting_join.remove(&user_handled);
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(format!("Hosting, code {}", self.session_code));
            ui.separator();

            if let Some(controller) = users_list(
                ui,
                self.usernames.lock().unwrap(),
                self.username.clone(),
                true,
            ) {
                let deny_packet = Packet::DenyControl {
                    username: controller,
                };
                channel.send(deny_packet).unwrap();
            }

            if ui.button("End Session").clicked() {
                result = self.disconnect(channel);
            }
        });

        let input = ctx.input(|i| i.clone());

        // Handle Ctrl+Shift+R hotkey to revoke control
        if input.key_pressed(egui::Key::R) && input.modifiers.command && input.modifiers.shift {
            let usernames = self.usernames.lock().unwrap();
            let controller = usernames
                .iter()
                .find(|(_, user_type)| **user_type == UserType::Controller);

            if let Some((controller, _)) = controller {
                let deny_packet = Packet::DenyControl {
                    username: controller.to_string(),
                };
                channel.send(deny_packet).unwrap();

                info!(
                    target: LOG_TARGET,
                    "User {} is no longer the Controller of the session.",
                    controller
                );
            }
        }

        result
    }

    /// Performs cleanup when exiting the host scene
    ///
    /// This ensures proper session shutdown by:
    /// 1. Disconnecting from the session
    /// 2. Sending `SignOut` message
    /// 3. Sending `Shutdown` command
    /// 4. Closing the communication channel
    ///
    /// # Arguments
    ///
    /// * `channel` - Communication channel for sending exit notifications
    fn on_exit(&mut self, channel: &mut SecureChannel) {
        self.disconnect(channel);

        channel.send(Packet::SignOut).unwrap();
        channel.send(Packet::Shutdown).unwrap();

        channel.close();
    }
}
