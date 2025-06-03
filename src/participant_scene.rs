use eframe::egui::{self, pos2, Color32, Rect, Sense, Stroke, Ui, Vec2};
use remote_desktop::protocol::{ControlPayload, Packet};
use remote_desktop::secure_channel::SecureChannel;
use remote_desktop::{
    chat_ui, egui_key_to_vk, normalize_mouse_position, users_list, Scene, SceneChange, UserType,
};

use crate::{menu_scene::MenuScene, modifiers_state::ModifiersState};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Instant,
};

// Constants for control request messages displayed on the UI.
const REQUEST_CONTROL_MSG: &'static str = "Request Control";
const WAITING_CONTROL_MSG: &'static str = "Waiting for response...";
const CONTROLLING_MSG: &'static str = "You're the controller!";

/// Enum to control which panel is currently visible on the right side of the UI.
#[derive(PartialEq, Eq)]
enum RightPanelType {
    UsersList,
    Chat,
}

/// Starts the `ffmpeg` command to decode incoming H.264 video streams.
///
/// This function sets up `ffmpeg` as a child process. It configures `ffmpeg`
/// to receive H.264 data from its standard input, discard corrupted frames,
/// and output raw RGBA pixel data to its standard output.
///
/// # Returns
///
/// A `Child` process handle to the spawned `ffmpeg` instance.
///
/// # Panics
///
/// Panics if `ffmpeg` fails to spawn.
fn start_ffmpeg() -> Child {
    let ffmpeg = Command::new("ffmpeg")
        .args([
            "-flags",
            "low_delay", // Prioritize low latency decoding
            "-fflags",
            "discardcorrupt", // Discard corrupted frames instead of stopping
            "-f",
            "h264", // Input format is H.264
            "-i",
            "-", // Read input from stdin
            "-f",
            "rawvideo", // Output raw video
            "-pix_fmt",
            "rgba", // Output pixel format is RGBA
            "-",    // Write output to stdout
        ])
        .stdin(Stdio::piped()) // Pipe for H.264 input
        .stdout(Stdio::piped()) // Pipe for RGBA output
        .stderr(Stdio::null()) // Suppress ffmpeg's stderr output
        .spawn()
        .expect("Failed to spawn ffmpeg");

    ffmpeg
}

/// Spawns a dedicated thread to continuously receive `Packet`s from the `SecureChannel`.
///
/// This thread processes different types of incoming packets:
/// - `Packet::Screen`: Writes the received H.264 bytes to `ffmpeg`'s stdin for decoding.
/// - `Packet::UserUpdate`: Updates the shared `usernames` map, adding or removing users
///   and pushing status messages to the `chat_log`.
/// - `Packet::RequestControl`: Updates the `control_msg` to indicate the client is now controlling.
/// - `Packet::DenyControl`: Resets the `control_msg` and adds a denial message to `chat_log`.
/// - `Packet::SessionExit` or `Packet::SessionEnd`: Sets a `stop_flag` to signal other threads
///   to terminate and then exits the loop, returning control to the main UI thread.
/// - `Packet::Chat`: Adds the received chat message to the `chat_log`.
///
/// # Arguments
///
/// * `channel` - A `SecureChannel` instance (cloned for thread ownership) to receive packets.
/// * `stdin` - The `ChildStdin` of the `ffmpeg` process, used to feed H.264 data.
/// * `stop_flag` - An `Arc<AtomicBool>` used to signal this thread to stop.
/// * `usernames` - An `Arc<Mutex<HashMap<String, UserType>>>` to share and update the list of session participants.
/// * `control_msg` - An `Arc<Mutex<String>>` to share and update the current control status message.
/// * `chat_log` - An `Arc<Mutex<Vec<String>>>` to share and append chat messages.
///
/// # Returns
///
/// A `JoinHandle` for the spawned thread, allowing the main thread to wait for its completion.
fn thread_receive_socket(
    mut channel: SecureChannel,
    mut stdin: ChildStdin,
    stop_flag: Arc<AtomicBool>,
    usernames: Arc<Mutex<HashMap<String, UserType>>>,
    control_msg: Arc<Mutex<String>>,
    chat_log: Arc<Mutex<Vec<String>>>,
) -> JoinHandle<()> {
    thread::spawn(move || loop {
        // Check stop flag early to react to shutdown signals
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }

        // Receive packet, defaulting if there's an error to prevent crashing the thread
        let packet = channel.receive().unwrap_or_default();

        match packet {
            Packet::Screen { bytes } => {
                let _ = stdin.write_all(&bytes);
            }

            Packet::UserUpdate {
                user_type,
                joined_before,
                username,
            } => {
                let mut usernames_guard = usernames.lock().unwrap();
                let mut chat_log_guard = chat_log.lock().unwrap();

                if user_type == UserType::Leaving {
                    usernames_guard.remove(&username);
                    chat_log_guard.push(format!("#r{} has disconnected.", username));
                } else {
                    // Only add a "joined" message if the user wasn't already in the list
                    // and hadn't joined before (i.e., truly new to the session).
                    if usernames_guard.contains_key(&username) {
                        chat_log_guard.push(format!("#b{} is now a {}.", username, user_type));
                    } else if !joined_before {
                        chat_log_guard.push(format!("#g{} has joined the session.", username));
                    }
                    usernames_guard.insert(username.clone(), user_type);
                }
            }

            Packet::RequestControl { .. } => {
                // This client has been granted control
                let mut control_msg_guard = control_msg.lock().unwrap();
                *control_msg_guard = CONTROLLING_MSG.to_string();
            }

            Packet::DenyControl { .. } => {
                // This client's control request was denied
                let mut control_msg_guard = control_msg.lock().unwrap();
                // Only update message if we weren't already controlling
                if *control_msg_guard != CONTROLLING_MSG {
                    let mut chat_log_guard = chat_log.lock().unwrap();
                    chat_log_guard
                        .push("#rYour control request was denied by the host.".to_string());
                }
                *control_msg_guard = REQUEST_CONTROL_MSG.to_string();
            }

            Packet::SessionExit => {
                stop_flag.store(true, Ordering::Relaxed);
                break;
            }

            Packet::SessionEnd => {
                stop_flag.store(true, Ordering::Relaxed);
                channel.send(packet).unwrap();
                break;
            }

            Packet::Chat { message } => {
                let mut chat_log_guard = chat_log.lock().unwrap();
                chat_log_guard.push(message);
            }

            _ => (),
        }
    })
}

/// Spawns a dedicated thread to read decoded RGBA frames from `ffmpeg`'s stdout.
///
/// These raw video frames are then pushed into a shared `frame_queue` for rendering
/// on the main UI thread. The thread ensures that the queue does not grow excessively
/// large by removing older frames if it exceeds a certain size (e.g., 3 frames).
///
/// # Arguments
///
/// * `frame_queue` - An `Arc<Mutex<VecDeque<Vec<u8>>>>` to store decoded RGBA frames.
/// * `stdout` - The `ChildStdout` of the `ffmpeg` process, used to read RGBA data.
/// * `stop_flag` - An `Arc<AtomicBool>` used to signal this thread to stop.
///
/// # Returns
///
/// A `JoinHandle` for the spawned thread.
fn thread_read_decoded(
    frame_queue: Arc<Mutex<VecDeque<Vec<u8>>>>,
    mut stdout: ChildStdout,
    stop_flag: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        // Buffer for a single 1920x1080 RGBA frame (1920 * 1080 * 4 bytes)
        let mut rgba_buffer = vec![0u8; 1920 * 1080 * 4];
        while !stop_flag.load(Ordering::Relaxed) {
            // Attempt to read exactly one frame's worth of data
            if let Ok(()) = stdout.read_exact(&mut rgba_buffer) {
                let mut queue = frame_queue.lock().unwrap();

                // Limit the queue size to prevent excessive memory usage or lag
                if queue.len() > 3 {
                    queue.pop_front(); // Discard the oldest frame
                }
                queue.push_back(rgba_buffer.clone()); // Add the new frame
            } else {
                // If reading fails (e.g., pipe closed, ffmpeg exits), exit the loop
                break;
            }
        }
    })
}

/// Represents the active remote desktop session scene for a participant.
///
/// This scene displays the remote screen, handles user input for control
/// (keyboard, mouse), manages the list of online users, and provides chat functionality.
/// It integrates with `ffmpeg` for video decoding and uses multiple threads
/// for efficient network and video processing.
pub struct ParticipantScene {
    /// Tracks time for frame rate limiting of screen updates.
    now: Instant,
    /// Accumulates elapsed time to control frame updates.
    elapsed_time: f32,

    /// A shared queue of decoded RGBA frames from `ffmpeg`.
    frame_queue: Arc<Mutex<VecDeque<Vec<u8>>>>,
    /// The currently displayed screen frame.
    current_frame: Vec<u8>,

    /// State manager for keyboard modifier keys (Ctrl, Alt, Shift).
    modifiers_state: ModifiersState,
    /// A flag to signal all background threads to stop.
    stop_flag: Arc<AtomicBool>,
    /// The bounding rectangle where the remote screen image is drawn.
    image_rect: Rect,
    /// Determines which side panel (Users List or Chat) is currently active.
    right_panel_type: RightPanelType,

    /// A shared map of usernames and their `UserType`s (e.g., Host, Participant).
    usernames: Arc<Mutex<HashMap<String, UserType>>>,
    /// The username of the current client.
    username: String,
    /// A shared string indicating the current control status (e.g., "Request Control", "Controlling").
    control_msg: Arc<Mutex<String>>,

    /// A shared log of chat messages displayed in the chat panel.
    chat_log: Arc<Mutex<Vec<String>>>,
    /// The current message being typed by the user in the chat input.
    chat_message: String,

    /// Handle for the thread receiving packets from the server.
    thread_receive_socket: Option<JoinHandle<()>>,
    /// Handle for the thread reading decoded frames from `ffmpeg`.
    thread_read_decoded: Option<JoinHandle<()>>,
    /// The `ffmpeg` child process.
    ffmpeg_command: Child,
}

impl ParticipantScene {
    /// Creates a new `ParticipantScene` and initializes all necessary components.
    ///
    /// This involves:
    /// 1. Spawning the `ffmpeg` process and taking ownership of its stdin/stdout.
    /// 2. Initializing shared data structures (`frame_queue`, `usernames`, `control_msg`, `chat_log`)
    ///    using `Arc<Mutex>` for thread-safe access.
    /// 3. Spawning the `thread_receive_socket` and `thread_read_decoded` background threads,
    ///    passing them the necessary shared data and the `stop_flag`.
    ///
    /// # Arguments
    ///
    /// * `channel` - A mutable reference to the `SecureChannel` to be used for communication.
    /// * `username` - The username of the client entering this session.
    ///
    /// # Returns
    ///
    /// A new `ParticipantScene` instance.
    pub fn new(channel: &mut SecureChannel, username: String) -> Self {
        let mut ffmpeg = start_ffmpeg();
        let stdin = ffmpeg.stdin.take().unwrap();
        let stdout = ffmpeg.stdout.take().unwrap();

        let frame_queue = Arc::new(Mutex::new(VecDeque::new()));
        let frame_queue_clone = frame_queue.clone(); // Clone for the decoding thread

        let stop_flag = Arc::new(AtomicBool::new(false)); // Flag to gracefully stop threads

        let usernames = Arc::new(Mutex::new(HashMap::new()));
        let control_msg = Arc::new(Mutex::new(REQUEST_CONTROL_MSG.to_owned())); // Initial control state

        let chat_log = Arc::new(Mutex::new(Vec::new()));

        let thread_receive_socket = thread_receive_socket(
            channel.clone(), // Clone channel for the thread
            stdin,
            stop_flag.clone(),
            usernames.clone(),
            control_msg.clone(),
            chat_log.clone(),
        );
        let thread_read_decoded = thread_read_decoded(frame_queue_clone, stdout, stop_flag.clone());

        Self {
            now: Instant::now(),
            elapsed_time: 0.,

            frame_queue,
            current_frame: vec![0u8; 1920 * 1080 * 4], // Initialize with a blank frame

            modifiers_state: ModifiersState::new(),
            stop_flag,
            image_rect: Rect {
                min: pos2(0.0, 0.0),
                max: pos2(0.0, 0.0),
            }, // Will be updated during rendering
            right_panel_type: RightPanelType::UsersList, // Default to users list

            usernames,
            username,
            control_msg,

            chat_log,
            chat_message: String::new(),

            thread_receive_socket: Some(thread_receive_socket), // Store thread handles
            thread_read_decoded: Some(thread_read_decoded),
            ffmpeg_command: ffmpeg, // Store the ffmpeg child process handle
        }
    }

    /// Handles user input (keyboard, mouse) and sends corresponding `Control` packets to the server.
    ///
    /// This method is called when the client has active control of the remote desktop.
    /// It processes `egui` input events, converts them into `ControlPayload` types,
    /// and sends them over the `SecureChannel`.
    ///
    /// # Arguments
    ///
    /// * `input` - The current `egui::InputState` containing all user input events.
    /// * `channel` - A mutable reference to the `SecureChannel` to send control packets.
    fn handle_input(&mut self, input: &egui::InputState, channel: &mut SecureChannel) {
        // Update the state of modifier keys (e.g., Ctrl, Alt, Shift)
        self.modifiers_state.update(input);

        // Send packets for individual key presses/releases
        for key_event in &self.modifiers_state.keys {
            let key_packet = Packet::Control {
                payload: ControlPayload::Keyboard {
                    pressed: key_event.pressed,
                    key: key_event.key,
                },
            };
            // Ignore errors for now, but in a production app, robust error handling is needed
            channel.send(key_packet).unwrap();
        }

        // Process other egui input events
        for event in &input.events {
            match event {
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    ..
                } => {
                    // Normalize mouse position to be relative to the screen dimensions
                    let (mouse_x, mouse_y) = normalize_mouse_position(*pos, self.image_rect);

                    let click_packet = Packet::Control {
                        payload: ControlPayload::MouseClick {
                            mouse_x,
                            mouse_y,
                            pressed: *pressed,
                            button: *button,
                        },
                    };
                    channel.send(click_packet).unwrap();
                }

                egui::Event::Key {
                    physical_key,
                    pressed,
                    ..
                } => {
                    // Only process physical keys that map to a virtual key code
                    if let Some(key) = physical_key {
                        // Convert egui key to a platform-agnostic virtual key code
                        if let Some(vk) = egui_key_to_vk(key) {
                            let key_packet = Packet::Control {
                                payload: ControlPayload::Keyboard {
                                    pressed: *pressed,
                                    key: vk,
                                },
                            };
                            channel.send(key_packet).unwrap();
                        }
                    }
                }

                egui::Event::PointerMoved(new_pos) => {
                    // Normalize mouse position for movements
                    let (mouse_x, mouse_y) = normalize_mouse_position(*new_pos, self.image_rect);

                    let mouse_move_packet = Packet::Control {
                        payload: ControlPayload::MouseMove { mouse_x, mouse_y },
                    };
                    channel.send(mouse_move_packet).unwrap();
                }

                egui::Event::MouseWheel { delta, .. } => {
                    // Send scroll wheel events
                    let scroll_packet = Packet::Control {
                        payload: ControlPayload::Scroll {
                            // Convert scroll delta to a signum (1 for up, -1 for down)
                            delta: delta.y.signum() as i32,
                        },
                    };
                    channel.send(scroll_packet).unwrap();
                }

                _ => { /* Ignore other egui events */ }
            }
        }
    }

    /// Renders the main central panel of the UI, displaying the remote screen.
    ///
    /// This method fetches the latest decoded frame, creates an `egui::Texture` from it,
    /// and draws it onto the UI. It also calculates the appropriate scaling and centering
    /// for the screen image to fit the available space. Crucially, it checks if the
    /// current client has control before enabling input handling for the screen area.
    ///
    /// # Arguments
    ///
    /// * `ui` - A mutable reference to the `egui::Ui` to draw on.
    /// * `ctx` - The `egui::Context` for loading textures.
    /// * `channel` - A mutable reference to the `SecureChannel` for sending control packets.
    fn central_panel_ui(&mut self, ui: &mut Ui, ctx: &egui::Context, channel: &mut SecureChannel) {
        // Create an egui texture from the raw RGBA frame data
        let texture = egui::ColorImage::from_rgba_unmultiplied([1920, 1080], &self.current_frame);
        let handle = ctx.load_texture("screen", texture, egui::TextureOptions::default());

        // Calculate available space and scaling factor to fit the image
        let available_size = ui.available_size();
        let scale = {
            let scale_x = available_size.x / 1920.0;
            let scale_y = available_size.y / 1080.0;
            scale_x.min(scale_y) // Use the smaller scale to ensure the whole image fits
        };

        let final_size = Vec2::new(1920.0, 1080.0) * scale; // Scaled dimensions

        // Calculate the centered position for the image
        let available_rect = ui.max_rect();
        let top_left = available_rect.center() - final_size * 0.5;
        let centered_rect = Rect::from_min_size(top_left, final_size);

        // Allocate the calculated space for the image, making it interactive for input
        let response = ui.allocate_rect(centered_rect, Sense::click_and_drag());
        self.image_rect = centered_rect; // Store the actual drawn rectangle for input normalization

        // Draw the remote screen image
        ui.painter().image(
            handle.id(),
            self.image_rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), // UV coordinates for the whole texture
            Color32::WHITE,                                     // Tint color (white means no tint)
        );

        // Draw a border around the displayed screen
        let stroke = Stroke::new(1.0, Color32::WHITE);
        ui.painter()
            .rect_stroke(centered_rect, 0.0, stroke, egui::StrokeKind::Outside);

        // If the mouse is hovering over the screen area and the client has control,
        // request focus and handle user input.
        if response.hovered() && self.control_msg.lock().unwrap().as_str() == CONTROLLING_MSG {
            // Request focus so keyboard events are directed to this widget
            ui.ctx().memory_mut(|mem| mem.request_focus(egui::Id::NULL));
            ui.input(|input| self.handle_input(input, channel));
        }
    }

    /// Handles disconnecting from the current remote session.
    ///
    /// This method sends a `Packet::SessionExit` to the server to signal departure,
    /// then gracefully shuts down all background threads and the `ffmpeg` process.
    /// Finally, it transitions the application back to the `MenuScene`.
    ///
    /// # Arguments
    ///
    /// * `channel` - A mutable reference to the `SecureChannel` to send the exit packet.
    ///
    /// # Returns
    ///
    /// A `SceneChange::To` variant, signaling a transition to the `MenuScene`.
    fn disconnect(&mut self, channel: &mut SecureChannel) -> SceneChange {
        // Send a signal to the server that this participant is leaving the session
        channel.send(Packet::SessionExit).unwrap();

        // Signal background threads to stop (though they might already be stopping via stop_flag)
        self.stop_flag.store(true, Ordering::Relaxed);

        // Join the background threads to ensure they have finished their cleanup
        if let Some(handle) = self.thread_receive_socket.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.thread_read_decoded.take() {
            let _ = handle.join();
        }

        // Kill the ffmpeg process to release its resources
        let _ = self.ffmpeg_command.kill();
        let _ = self.ffmpeg_command.wait(); // Wait for ffmpeg to actually terminate

        // Transition back to the MenuScene
        SceneChange::To(Box::new(MenuScene::new(self.username.clone(), channel, "")))
    }
}

impl Scene for ParticipantScene {
    /// Updates the `ParticipantScene` for each frame.
    ///
    /// This method manages the rendering of the remote screen by dequeuing
    /// the latest available frame, handles UI updates for side panels (Users List, Chat),
    /// and processes interactions with control buttons (Disconnect, Request Control).
    /// It also checks the `stop_flag` for signals to exit the session.
    ///
    /// # Arguments
    ///
    /// * `ctx` - The `egui::Context` providing access to egui's rendering and input state.
    /// * `channel` - A mutable reference to the `SecureChannel` for all network communications.
    ///
    /// # Returns
    ///
    /// A `SceneChange` enum variant, indicating whether the scene should transition
    /// to `MenuScene` (e.g., on disconnection or session end) or remain in the `ParticipantScene`.
    fn update(&mut self, ctx: &egui::Context, channel: &mut SecureChannel) -> SceneChange {
        let mut result: SceneChange = SceneChange::None;

        // --- Frame Rate Limiting and Screen Update ---
        let now = Instant::now();
        let dt = now.duration_since(self.now).as_secs_f32();
        self.now = now;
        self.elapsed_time += dt;

        // Only update the displayed frame at approximately 30 FPS
        if self.elapsed_time > 1. / 30. {
            self.elapsed_time = 0.;
            if let Some(image) = self.frame_queue.lock().unwrap().pop_front() {
                self.current_frame = image; // Display the latest frame from the queue
            }
        }

        // --- Check for Session End Signal ---
        if self.stop_flag.load(Ordering::Relaxed) {
            // Ensure all related processes and threads are cleaned up if the stop flag is set
            let _ = self.ffmpeg_command.kill();
            let _ = self.ffmpeg_command.wait(); // Make sure ffmpeg process truly terminates
            if let Some(handle) = self.thread_receive_socket.take() {
                let _ = handle.join();
            }
            if let Some(handle) = self.thread_read_decoded.take() {
                let _ = handle.join();
            }

            // Transition back to the MenuScene with an informative message
            return SceneChange::To(Box::new(MenuScene::new(
                self.username.clone(),
                channel,
                "The host ended the session.",
            )));
        }

        // --- Right Side Panel (Users List / Chat) ---
        egui::SidePanel::right("participants").show(ctx, |ui| {
            // Toggle between User List and Chat
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut self.right_panel_type,
                    RightPanelType::UsersList,
                    "User List",
                );
                ui.selectable_value(&mut self.right_panel_type, RightPanelType::Chat, "Chat");
            });

            match self.right_panel_type {
                RightPanelType::UsersList => {
                    ui.heading("Users");
                    ui.separator();
                    // Delegate rendering of the user list to a shared utility function
                    users_list(
                        ui,
                        self.usernames.lock().unwrap(), // Lock the mutex to access usernames
                        self.username.clone(),
                        false, // This client is not the host
                    );
                }
                RightPanelType::Chat => {
                    // Delegate rendering of the chat UI to a shared utility function
                    chat_ui(
                        ui,
                        self.chat_log.lock().unwrap(), // Lock the mutex to access chat log
                        &mut self.chat_message,
                        channel,
                    );
                }
            }
        });

        // --- Bottom Panel (Disconnect and Control Buttons) ---
        egui::TopBottomPanel::bottom("bottom_panel")
            .resizable(false)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    // Disconnect button
                    if ui.button("Disconnect").clicked() {
                        result = self.disconnect(channel);
                    }

                    // Control request button
                    let mut control_msg_guard = self.control_msg.lock().unwrap();
                    if ui
                        .add_enabled(
                            // Only enable the button if the current message is "Request Control"
                            control_msg_guard.as_str() == REQUEST_CONTROL_MSG,
                            |ui: &mut Ui| ui.button(control_msg_guard.as_str()),
                        )
                        .clicked()
                    {
                        // Change button text to "Waiting for response..."
                        *control_msg_guard = WAITING_CONTROL_MSG.to_string();

                        // Send a control request packet to the server
                        let request_control = Packet::RequestControl {
                            username: self.username.clone(),
                        };
                        channel.send(request_control).unwrap();
                    }
                });
            });

        // --- Central Panel (Remote Screen Display) ---
        egui::CentralPanel::default().show(ctx, |ui| {
            self.central_panel_ui(ui, ctx, channel);
        });

        result // Return any scene change requested
    }

    /// Called when the application is exiting or transitioning away from the `ParticipantScene`.
    ///
    /// This method ensures a clean shutdown of all background threads and processes,
    /// sends necessary `SignOut` and `Shutdown` packets to the server, and closes the `SecureChannel`.
    ///
    /// # Arguments
    ///
    /// * `channel` - A mutable reference to the `SecureChannel` to be closed.
    fn on_exit(&mut self, channel: &mut SecureChannel) {
        // Perform disconnection logic, including thread cleanup and ffmpeg termination
        self.disconnect(channel);

        // Send final sign-out and shutdown packets to the server
        channel.send(Packet::SignOut).unwrap();
        channel.send(Packet::Shutdown).unwrap();

        // Close the secure communication channel
        channel.close();
    }
}
