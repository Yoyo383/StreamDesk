use std::{
    collections::VecDeque,
    io::{Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::Instant,
};

use eframe::egui::{self, pos2, Color32, ImageSource, Rect, Sense, Stroke, Ui, Vec2};
use remote_desktop::{protocol::Packet, secure_channel::SecureChannel, Scene, SceneChange};

use crate::menu_scene::MenuScene;

const PLAY_IMAGE: ImageSource = egui::include_image!("../images/play.svg");
const PAUSE_IMAGE: ImageSource = egui::include_image!("../images/pause.svg");
const FORWARD_IMAGE: ImageSource = egui::include_image!("../images/forward.svg");
const BACKWARD_IMAGE: ImageSource = egui::include_image!("../images/backward.svg");

/// Spawns an FFmpeg process configured for H.264 to RGBA conversion.
///
/// This function creates an FFmpeg child process with specific arguments for
/// low-latency H.264 decoding. The process reads H.264 encoded data from stdin
/// and outputs raw RGBA pixel data to stdout.
///
/// # Returns
///
/// A `Child` process handle representing the spawned FFmpeg instance.
/// - The process is configured with piped stdin and stdout for data communication.
/// - stderr is redirected to null to suppress FFmpeg diagnostic output.
///
/// # Panics
///
/// Panics if FFmpeg cannot be spawned, typically due to FFmpeg not being
/// installed or not found in the system PATH.
fn start_ffmpeg() -> Child {
    let ffmpeg = Command::new("ffmpeg")
        .args([
            "-flags",
            "low_delay",
            "-fflags",
            "discardcorrupt",
            "-f",
            "h264",
            "-i",
            "-",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn ffmpeg");

    ffmpeg
}

/// Creates a background thread to receive screen data from the secure channel.
///
/// This thread continuously receives packets from the secure channel and forwards
/// H.264 encoded screen data to the FFmpeg process for decoding. The thread handles
/// different packet types and manages the FFmpeg stdin stream lifecycle.
///
/// # Arguments
///
/// * `channel` - A `SecureChannel` for receiving packets from the remote source.
/// * `stdin` - A `ChildStdin` handle to the FFmpeg process for writing H.264 data.
///
/// # Returns
///
/// A `JoinHandle<()>` for the spawned thread that can be used to wait for
/// completion or join the thread when cleaning up resources.
///
/// # Behavior
///
/// - `Packet::Screen` packets: H.264 data is written to FFmpeg stdin
/// - `Packet::None` packets: Closes the FFmpeg stdin stream
/// - `Packet::SeekInit` or `Packet::SessionExit`: Terminates the thread
/// - Other packet types are ignored
fn thread_receive_socket(mut channel: SecureChannel, stdin: ChildStdin) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut stdin = Some(stdin);

        loop {
            let packet = channel.receive().unwrap();

            match packet {
                Packet::Screen { bytes } => {
                    if let Some(ref mut stdin) = stdin {
                        let _ = stdin.write_all(&bytes);
                    }
                }

                Packet::None => {
                    stdin = None;
                }

                Packet::SeekInit => break,

                Packet::SessionExit => break,

                _ => (),
            }
        }
    })
}

/// Creates a background thread to read decoded frames from FFmpeg output.
///
/// This thread continuously reads RGBA frame data from FFmpeg stdout and manages
/// a synchronized frame queue. It implements backpressure by waiting when the
/// queue is full and respects stop signals for graceful shutdown.
///
/// # Arguments
///
/// * `stdout` - A `ChildStdout` handle to read decoded RGBA data from FFmpeg.
/// * `frame_queue` - An `Arc<(Mutex<VecDeque<Vec<u8>>>, Condvar)>` for thread-safe
///                     frame storage with synchronization primitives.
/// * `stop_flag` - An `Arc<AtomicBool>` for signaling thread termination.
///
/// # Returns
///
/// A `JoinHandle<()>` for the spawned thread that handles frame reading and
/// queue management in the background.
///
/// # Behavior
///
/// - Reads exactly 1920×1080×4 bytes per frame (RGBA pixels)
/// - Implements queue size limit of 30 frames to prevent memory overflow
/// - Blocks when queue is full until space is available or stop signal is received
/// - Terminates gracefully when FFmpeg stdout closes or stop flag is set
fn thread_read_decoded(
    mut stdout: ChildStdout,
    frame_queue: Arc<(Mutex<VecDeque<Vec<u8>>>, Condvar)>,
    stop_flag: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let (queue_mutex, condvar) = &*frame_queue;
        let mut rgba_buf = vec![0u8; 1920 * 1080 * 4];

        loop {
            // read exactly one decoded frame
            let Ok(()) = stdout.read_exact(&mut rgba_buf) else {
                return;
            };

            let mut queue = queue_mutex.lock().unwrap();

            // wait until queue is less than 30 or stop flag is true
            queue = condvar
                .wait_while(queue, |q| {
                    q.len() >= 30 && !stop_flag.load(Ordering::Relaxed)
                })
                .unwrap();

            if stop_flag.load(Ordering::Relaxed) {
                break;
            }

            queue.push_back(rgba_buf.clone());
        }
    })
}

/// Represents the video watching scene with playback controls and frame rendering.
///
/// This struct manages the entire video playback experience including FFmpeg
/// integration, frame decoding, UI rendering, and playback controls. It coordinates
/// multiple background threads for efficient video streaming and decoding.
pub struct WatchScene {
    /// Current timestamp for frame timing calculations
    now: Instant,
    /// Accumulated time since last frame update
    elapsed_time: f32,

    /// Username of the user
    username: String,
    /// Total duration of the recording in frames
    duration: i32,
    /// Current playback position in frames
    current_frame_number: i32,

    /// Atomic boolean for coordinating thread shutdown
    stop_flag: Arc<AtomicBool>,
    /// Current pause state of the playback
    is_paused: bool,

    /// Thread-safe queue containing decoded RGBA frames
    frame_queue: Arc<(Mutex<VecDeque<Vec<u8>>>, Condvar)>,
    /// Currently displayed frame data
    current_frame: Vec<u8>,

    /// Handle to the network receiving thread
    thread_receive_socket: Option<JoinHandle<()>>,
    /// Handle to the frame decoding thread
    thread_read_decoded: Option<JoinHandle<()>>,
    /// FFmpeg child process handle
    ffmpeg_command: Child,
}

impl WatchScene {
    /// Creates a new `WatchScene` instance and initializes the video playback system.
    ///
    /// This constructor sets up the complete video playback pipeline including FFmpeg
    /// process spawning, background thread creation for network reception and frame
    /// decoding, and initializes all necessary synchronization primitives.
    ///
    /// # Arguments
    ///
    /// * `username` - A `String` containing the username for the current session.
    /// * `duration` - An `i32` representing the total recording duration in frames.
    /// * `channel` - A mutable reference to `SecureChannel` for network communication.
    ///
    /// # Returns
    ///
    /// A new `Self` instance with all components initialized and background
    /// threads started for immediate video playback capability.
    pub fn new(username: String, duration: i32, channel: &mut SecureChannel) -> Self {
        let mut ffmpeg = start_ffmpeg();
        let stdin = ffmpeg.stdin.take().unwrap();
        let stdout = ffmpeg.stdout.take().unwrap();

        let frame_queue = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));

        let stop_flag = Arc::new(AtomicBool::new(false));

        let thread_receive_socket = thread_receive_socket(channel.clone(), stdin);
        let thread_read_decoded =
            thread_read_decoded(stdout, frame_queue.clone(), stop_flag.clone());

        Self {
            now: Instant::now(),
            elapsed_time: 0.0,

            username,
            duration,
            current_frame_number: 0,

            stop_flag,
            is_paused: false,

            frame_queue,
            current_frame: vec![0u8; 1920 * 1080 * 4],

            thread_receive_socket: Some(thread_receive_socket),
            thread_read_decoded: Some(thread_read_decoded),
            ffmpeg_command: ffmpeg,
        }
    }

    /// Renders the main video display area in the central panel.
    ///
    /// This method handles the video frame rendering with proper scaling and centering
    /// within the available UI space. It creates a texture from the current frame data
    /// and displays it with appropriate scaling to maintain aspect ratio.
    ///
    /// # Arguments
    ///
    /// * `ui` - A mutable reference to `Ui` for rendering operations.
    /// * `ctx` - A reference to `egui::Context` for texture management.
    ///
    /// # Behavior
    ///
    /// - Converts current frame data (1920×1080 RGBA) to an egui texture
    /// - Calculates appropriate scaling to fit within available space
    /// - Centers the video display within the panel
    /// - Applies a white border around the video frame
    fn central_panel_ui(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let texture = egui::ColorImage::from_rgba_unmultiplied([1920, 1080], &self.current_frame);
        let handle = ctx.load_texture("screen", texture, egui::TextureOptions::default());

        let available_size = ui.available_size();

        let scale = {
            let scale_x = available_size.x / 1920.0;
            let scale_y = available_size.y / 1080.0;
            scale_x.min(scale_y)
        };

        let final_size = Vec2::new(1920.0, 1080.0) * scale;

        let available_rect = ui.max_rect();
        let top_left = available_rect.center() - final_size * 0.5;
        let centered_rect = Rect::from_min_size(top_left, final_size);

        // allocate the space exactly at the centered position
        ui.allocate_rect(centered_rect, Sense::click_and_drag());

        ui.painter().image(
            handle.id(),
            centered_rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );

        // put a border
        let stroke = Stroke::new(1.0, Color32::WHITE);
        ui.painter()
            .rect_stroke(centered_rect, 0.0, stroke, egui::StrokeKind::Outside);
    }

    /// Performs a seek operation to a different position in the recording.
    ///
    /// This method implements a complete seek operation by coordinating with the
    /// remote server, stopping all current processes, clearing buffers, and
    /// restarting the entire playback pipeline at the new position.
    ///
    /// # Arguments
    ///
    /// * `delta` - An `i32` representing the number of seconds to seek (positive
    ///               for forward, negative for backward).
    /// * `channel` - A mutable reference to `SecureChannel` for server communication.
    ///
    /// # Behavior
    ///
    /// - Sends `Packet::SeekInit` to notify the server of seek operation
    /// - Stops all background threads and clears frame queue
    /// - Terminates current FFmpeg process
    /// - Calculates new position with bounds checking (0 to duration/30)
    /// - Sends `Packet::SeekTo` with new timestamp
    /// - Restarts FFmpeg and background threads for new position
    fn seek_recording(&mut self, delta: i32, channel: &mut SecureChannel) {
        // send seek init
        channel.send(Packet::SeekInit).unwrap();

        // stop everything
        self.stop_flag.store(true, Ordering::Relaxed);

        {
            let (queue, condvar) = &*self.frame_queue;
            let mut queue = queue.lock().unwrap();
            queue.clear(); // clear old frames
            condvar.notify_all();
        }

        let _ = self.thread_receive_socket.take().unwrap().join();
        let _ = self.thread_read_decoded.take().unwrap().join();
        let _ = self.ffmpeg_command.kill();

        // send seek to
        let time_seconds = (self.current_frame_number / 30 + delta).clamp(0, self.duration / 30);
        self.current_frame_number = time_seconds * 30;

        channel.send(Packet::SeekTo { time_seconds }).unwrap();

        // start everything from scratch

        self.ffmpeg_command = start_ffmpeg();
        let stdin = self.ffmpeg_command.stdin.take().unwrap();
        let stdout = self.ffmpeg_command.stdout.take().unwrap();

        self.stop_flag.store(false, Ordering::Relaxed);

        self.thread_receive_socket = Some(thread_receive_socket(channel.clone(), stdin));
        self.thread_read_decoded = Some(thread_read_decoded(
            stdout,
            self.frame_queue.clone(),
            self.stop_flag.clone(),
        ));
    }

    /// Gracefully exits the watch scene and returns to the menu.
    ///
    /// This method performs a complete cleanup of all resources and communicates
    /// session termination to the remote server before transitioning back to
    /// the menu scene.
    ///
    /// # Arguments
    ///
    /// * *`channel`* - A mutable reference to *`SecureChannel`* for server communication.
    ///
    /// # Returns
    ///
    /// A *`SceneChange`* containing the transition to a new *`MenuScene`* instance
    /// initialized with the current username and an empty status message.
    ///
    /// # Behavior
    ///
    /// - Sends *`Packet::SessionExit`* to notify server of session termination
    /// - Stops all background threads and clears frame queue
    /// - Terminates FFmpeg process and releases all resources
    /// - Creates and returns transition to menu scene
    fn exit(&mut self, channel: &mut SecureChannel) -> SceneChange {
        channel.send(Packet::SessionExit).unwrap();

        self.stop_flag.store(true, Ordering::Relaxed);

        {
            let (_, condvar) = &*self.frame_queue;
            condvar.notify_all(); // wake up decoder thread in case it's waiting
        }

        let _ = self.thread_receive_socket.take().unwrap().join();
        let _ = self.thread_read_decoded.take().unwrap().join();
        let _ = self.ffmpeg_command.kill();

        SceneChange::To(Box::new(MenuScene::new(self.username.clone(), channel, "")))
    }
}

impl Scene for WatchScene {
    /// Updates the watch scene state and renders the user interface.
    ///
    /// This method is called every frame to update the video playback state,
    /// handle user interactions, and render the complete UI including video
    /// display and playback controls.
    ///
    /// # Arguments
    ///
    /// * *`ctx`* - A reference to *`egui::Context`* for UI rendering operations.
    /// * *`channel`* - A mutable reference to *`SecureChannel`* for server communication.
    ///
    /// # Returns
    ///
    /// A *`SceneChange`* which is:
    /// - *`SceneChange::None`* during normal operation
    /// - *`SceneChange::To(...)`* when transitioning to another scene (e.g., menu)
    ///
    /// # Behavior
    ///
    /// - Updates frame timing and advances playback when not paused
    /// - Renders bottom panel with playback controls (play/pause, seek, progress)
    /// - Renders central panel with scaled video display
    /// - Handles user interactions for playback control and scene navigation
    fn update(&mut self, ctx: &egui::Context, channel: &mut SecureChannel) -> SceneChange {
        let mut result = SceneChange::None;

        let now = Instant::now();
        let dt = now.duration_since(self.now).as_secs_f32();
        self.now = now;
        self.elapsed_time += dt;

        if self.elapsed_time > 1.0 / 30.0 && !self.is_paused {
            self.elapsed_time = 0.0;

            let (queue_mutex, condvar) = &*self.frame_queue;
            let mut queue = queue_mutex.lock().unwrap();

            if let Some(frame) = queue.pop_front() {
                self.current_frame = frame;

                self.current_frame_number += 1;
            }

            condvar.notify_all();
        }

        egui::TopBottomPanel::bottom("bottom_panel")
            .resizable(false)
            .show(ctx, |ui| {
                let current_time = format!(
                    "{:02}:{:02}",
                    self.current_frame_number / 1800,
                    (self.current_frame_number % 1800) / 30
                );

                let total_time = format!(
                    "{:02}:{:02}",
                    self.duration / 1800,
                    (self.duration % 1800) / 30
                );

                let time_string = format!("{} / {}", current_time, total_time);
                let progress = self.current_frame_number as f32 / self.duration as f32;

                ui.horizontal(|ui| {
                    let pause_play_button = egui::Button::image(if self.is_paused {
                        PLAY_IMAGE
                    } else {
                        PAUSE_IMAGE
                    })
                    .frame(false);

                    let response = ui.add_sized([30.0, 30.0], pause_play_button);

                    // change cursor to hand
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    // toggle pause
                    if response.clicked() {
                        self.is_paused = !self.is_paused;
                    }

                    let skip_backward_button = egui::Button::image(BACKWARD_IMAGE).frame(false);
                    let response = ui.add_sized([30.0, 30.0], skip_backward_button);

                    // change cursor to hand
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    // skip forward 5 seconds
                    if response.clicked() {
                        self.seek_recording(-5, channel);
                    }

                    let skip_forward_button = egui::Button::image(FORWARD_IMAGE).frame(false);
                    let response = ui.add_sized([30.0, 30.0], skip_forward_button);

                    // change cursor to hand
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    // skip forward 5 seconds
                    if response.clicked() {
                        self.seek_recording(5, channel);
                    }

                    ui.label(time_string);
                    let progress_bar = egui::ProgressBar::new(progress);
                    ui.add(progress_bar);
                });

                ui.vertical_centered(|ui| {
                    if ui.button("Exit").clicked() {
                        result = self.exit(channel);
                    }
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.central_panel_ui(ui, ctx);
        });

        result
    }

    /// Performs cleanup operations when the scene is being exited.
    ///
    /// This method is called when the scene is being destroyed or replaced,
    /// ensuring proper resource cleanup and graceful disconnection from the
    /// remote server.
    ///
    /// # Arguments
    ///
    /// * *`channel`* - A mutable reference to *`SecureChannel`* for server communication.
    ///
    /// # Behavior
    ///
    /// - Calls internal exit method to clean up video playback resources
    /// - Sends *`Packet::SignOut`* to log out from the current session
    /// - Sends *`Packet::Shutdown`* to request server shutdown
    /// - Closes the secure channel connection
    fn on_exit(&mut self, channel: &mut SecureChannel) {
        self.exit(channel);

        channel.send(Packet::SignOut).unwrap();
        channel.send(Packet::Shutdown).unwrap();

        channel.close();
    }
}
