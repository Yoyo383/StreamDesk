use eframe::egui::{self, load::SizedTexture, Ui};
use remote_desktop::{
    egui_key_to_vk, normalize_mouse_position,
    protocol::{Message, MessageType},
    AppData, Scene, SceneChange,
};

use crate::modifiers_state::ModifiersState;
use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::TcpStream,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Instant,
};

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

fn thread_receive_encoded(
    mut socket: TcpStream,
    mut stdin: ChildStdin,
    stop_flag: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        while !stop_flag.load(Ordering::Relaxed) {
            if let Some(message) = Message::receive(&mut socket) {
                if message.message_type == MessageType::Screen {
                    stdin.write_all(&message.screen_data).unwrap();
                }
            }
        }
    });
}

fn thread_read_decoded(
    frame_queue: Arc<Mutex<VecDeque<Vec<u8>>>>,
    mut stdout: ChildStdout,
    stop_flag: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let mut rgba_buffer = vec![0u8; 1920 * 1080 * 4];
        while !stop_flag.load(Ordering::Relaxed) {
            stdout.read_exact(&mut rgba_buffer).unwrap();

            let mut queue = frame_queue.lock().unwrap();

            if queue.len() > 1 {
                queue.pop_front();
            }
            queue.push_back(rgba_buffer.clone());
        }
    });
}

pub struct MainScene {
    now: Instant,
    elapsed_time: f32,
    frame_queue: Arc<Mutex<VecDeque<Vec<u8>>>>,
    current_frame: Vec<u8>,
    modifiers_state: ModifiersState,
    stop_flag: Arc<AtomicBool>,
}

impl MainScene {
    pub fn new(app_data: &mut AppData) -> Self {
        let mut ffmpeg = start_ffmpeg();
        let stdin = ffmpeg.stdin.take().unwrap();
        let stdout = ffmpeg.stdout.take().unwrap();

        let frame_queue = Arc::new(Mutex::new(VecDeque::new()));
        let frame_queue_clone = frame_queue.clone();

        let stop_flag = Arc::new(AtomicBool::new(false));

        thread_receive_encoded(
            app_data.socket.as_mut().unwrap().try_clone().unwrap(),
            stdin,
            stop_flag.clone(),
        );
        thread_read_decoded(frame_queue_clone, stdout, stop_flag.clone());

        Self {
            now: Instant::now(),
            elapsed_time: 0.,
            frame_queue,
            current_frame: vec![0u8; 1920 * 1080 * 4],
            modifiers_state: ModifiersState::new(),
            stop_flag,
        }
    }

    fn handle_input(&mut self, input: &egui::InputState, app_data: &mut AppData) {
        self.modifiers_state.update(input);

        let socket = app_data.socket.as_mut().unwrap();

        for key in &self.modifiers_state.keys {
            let message = Message::new_keyboard(key.key, key.pressed);
            message.send(socket).unwrap();
        }

        for event in &input.events {
            match event {
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    ..
                } => {
                    let mouse_position =
                        normalize_mouse_position(*pos, app_data.width, app_data.height);
                    let message = Message::new_mouse_click(*button, mouse_position, *pressed);
                    message.send(socket).unwrap();
                }

                egui::Event::Key {
                    physical_key,
                    pressed,
                    ..
                } => {
                    if let Some(key) = physical_key {
                        let vk = egui_key_to_vk(key).unwrap();
                        let message = Message::new_keyboard(vk, *pressed);
                        message.send(socket).unwrap();
                    }
                }

                egui::Event::PointerMoved(new_pos) => {
                    let mouse_position =
                        normalize_mouse_position(*new_pos, app_data.width, app_data.height);
                    let message = Message::new_mouse_move(mouse_position);
                    message.send(socket).unwrap();
                }

                egui::Event::MouseWheel { delta, .. } => {
                    let message = Message::new_scroll(delta.y.signum());
                    message.send(socket).unwrap();
                }

                _ => (),
            }
        }
    }

    fn render(&mut self, ui: &mut Ui, ctx: &egui::Context, app_data: &mut AppData) {
        if self.elapsed_time > 1. / 30. {
            self.elapsed_time = 0.;
            if let Some(image) = self.frame_queue.lock().unwrap().pop_front() {
                self.current_frame = image;
            }
        }

        let texture = egui::ColorImage::from_rgba_unmultiplied([1920, 1080], &self.current_frame);
        let handle = ctx.load_texture("screen", texture, egui::TextureOptions::default());
        let sized_texture = SizedTexture::new(&handle, ui.available_size());
        ui.image(sized_texture);

        ui.input(|input| self.handle_input(input, app_data));

        ctx.request_repaint();
    }
}

impl Scene for MainScene {
    fn update(&mut self, ctx: &egui::Context, app_data: &mut AppData) -> Option<SceneChange> {
        let now = Instant::now();
        let dt = now.duration_since(self.now).as_secs_f32();
        self.now = now;
        self.elapsed_time += dt;

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                self.render(ui, ctx, app_data);
            });

        None
    }

    fn on_exit(&mut self, app_data: &mut AppData) {
        self.stop_flag.store(true, Ordering::Relaxed);
        let socket = app_data.socket.as_mut().unwrap();

        let message = Message::new_shutdown();
        message.send(socket).unwrap();

        socket
            .shutdown(std::net::Shutdown::Both)
            .expect("Could not close socket.");
    }
}
