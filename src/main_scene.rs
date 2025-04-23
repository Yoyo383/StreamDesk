use eframe::egui::{self, pos2, Color32, Rect, Sense, Stroke, Ui, Vec2};
use remote_desktop::{
    egui_key_to_vk, normalize_mouse_position,
    protocol::{Message, MessageType},
    AppData, Scene, SceneChange,
};

use crate::{menu_scene::MenuScene, modifiers_state::ModifiersState};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::TcpStream,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
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

fn thread_receive_socket(
    mut socket: TcpStream,
    mut stdin: ChildStdin,
    stop_flag: Arc<AtomicBool>,
    usernames: Arc<Mutex<Vec<String>>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while !stop_flag.load(Ordering::Relaxed) {
            let message = Message::receive(&mut socket).unwrap_or_default();

            match message.message_type {
                MessageType::Screen => {
                    stdin.write_all(&message.vector_data).unwrap();
                }

                MessageType::Joining => {
                    let mut usernames = usernames.lock().unwrap();
                    usernames.push(
                        String::from_utf8(message.vector_data).expect("bytes should be utf8"),
                    );
                }

                MessageType::SessionExit => {
                    // remove username from usernames
                    let username =
                        String::from_utf8(message.vector_data).expect("bytes should be utf8");

                    let mut usernames = usernames.lock().unwrap();
                    usernames.retain(|name| *name != username);
                }

                MessageType::SessionEnd => {
                    // signal all threads
                    stop_flag.store(true, Ordering::Relaxed);
                }

                _ => {}
            }
        }
    })
}

fn thread_read_decoded(
    frame_queue: Arc<Mutex<VecDeque<Vec<u8>>>>,
    mut stdout: ChildStdout,
    stop_flag: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut rgba_buffer = vec![0u8; 1920 * 1080 * 4];
        while !stop_flag.load(Ordering::Relaxed) {
            if let Ok(()) = stdout.read_exact(&mut rgba_buffer) {
                let mut queue = frame_queue.lock().unwrap();

                if queue.len() > 1 {
                    queue.pop_front();
                }
                queue.push_back(rgba_buffer.clone());
            }
        }
    })
}

pub struct MainScene {
    now: Instant,
    elapsed_time: f32,
    frame_queue: Arc<Mutex<VecDeque<Vec<u8>>>>,
    current_frame: Vec<u8>,
    modifiers_state: ModifiersState,
    stop_flag: Arc<AtomicBool>,
    image_rect: Rect,
    usernames: Arc<Mutex<Vec<String>>>,
    username: String,
    thread_receive_socket: Option<JoinHandle<()>>,
    thread_read_decoded: Option<JoinHandle<()>>,
    ffmpeg_command: Child,
}

impl MainScene {
    pub fn new(app_data: &mut AppData, usernames: Vec<String>, username: String) -> Self {
        let mut ffmpeg = start_ffmpeg();
        let stdin = ffmpeg.stdin.take().unwrap();
        let stdout = ffmpeg.stdout.take().unwrap();

        let frame_queue = Arc::new(Mutex::new(VecDeque::new()));
        let frame_queue_clone = frame_queue.clone();

        let stop_flag = Arc::new(AtomicBool::new(false));

        let usernames = Arc::new(Mutex::new(usernames));

        let thread_receive_socket = thread_receive_socket(
            app_data.socket.as_mut().unwrap().try_clone().unwrap(),
            stdin,
            stop_flag.clone(),
            usernames.clone(),
        );
        let thread_read_decoded = thread_read_decoded(frame_queue_clone, stdout, stop_flag.clone());

        Self {
            now: Instant::now(),
            elapsed_time: 0.,
            frame_queue,
            current_frame: vec![0u8; 1920 * 1080 * 4],
            modifiers_state: ModifiersState::new(),
            stop_flag,
            image_rect: Rect {
                min: pos2(0.0, 0.0),
                max: pos2(0.0, 0.0),
            },
            usernames,
            username,
            thread_receive_socket: Some(thread_receive_socket),
            thread_read_decoded: Some(thread_read_decoded),
            ffmpeg_command: ffmpeg,
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
                    let mouse_position = normalize_mouse_position(*pos, self.image_rect);

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
                    let mouse_position = normalize_mouse_position(*new_pos, self.image_rect);

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

    fn central_panel_ui(&mut self, ui: &mut Ui, ctx: &egui::Context, app_data: &mut AppData) {
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
        let response = ui.allocate_rect(centered_rect, Sense::hover());
        self.image_rect = centered_rect;

        ui.painter().image(
            handle.id(),
            self.image_rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );

        // put a border
        let stroke = Stroke::new(1.0, Color32::WHITE);
        ui.painter()
            .rect_stroke(centered_rect, 0.0, stroke, egui::StrokeKind::Outside);

        if response.hovered() {
            ui.ctx().memory_mut(|mem| mem.request_focus(egui::Id::NULL));
            ui.input(|input| self.handle_input(input, app_data));
        }

        ctx.request_repaint();
    }

    fn disconnect(&mut self, socket: &mut TcpStream) -> SceneChange {
        let message = Message::new_session_exit(&self.username);
        message.send(socket).unwrap();

        self.stop_flag.store(true, Ordering::Relaxed);

        let _ = self.thread_receive_socket.take().unwrap().join();
        let _ = self.thread_read_decoded.take().unwrap().join();
        let _ = self.ffmpeg_command.kill();

        SceneChange::To(Box::new(MenuScene::new(None, true)))
    }
}

impl Scene for MainScene {
    fn update(&mut self, ctx: &egui::Context, app_data: &mut AppData) -> Option<SceneChange> {
        let mut result: Option<SceneChange> = None;

        let now = Instant::now();
        let dt = now.duration_since(self.now).as_secs_f32();
        self.now = now;
        self.elapsed_time += dt;

        if self.elapsed_time > 1. / 30. {
            self.elapsed_time = 0.;
            if let Some(image) = self.frame_queue.lock().unwrap().pop_front() {
                self.current_frame = image;
            }
        }

        if self.stop_flag.load(Ordering::Relaxed) {
            let _ = self.ffmpeg_command.kill();
            let _ = self.thread_receive_socket.take().unwrap().join();
            let _ = self.thread_read_decoded.take().unwrap().join();

            return Some(SceneChange::To(Box::new(MenuScene::new(None, true))));
        }

        egui::SidePanel::right("participants").show(ctx, |ui| {
            ui.heading("Participants");
            ui.separator();

            let usernames = self.usernames.lock().unwrap();
            for username in usernames.iter() {
                ui.label(username);
            }
        });

        egui::TopBottomPanel::bottom("disconnect_panel")
            .resizable(false)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    if ui.button("Disconnect").clicked() {
                        result = Some(self.disconnect(app_data.socket.as_mut().unwrap()));
                    }
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.central_panel_ui(ui, ctx, app_data);
        });

        result
    }

    fn on_exit(&mut self, app_data: &mut AppData) {
        let socket = app_data.socket.as_mut().unwrap();

        self.disconnect(socket);

        let message = Message::new_shutdown();
        message.send(socket).unwrap();

        socket
            .shutdown(std::net::Shutdown::Both)
            .expect("Could not close socket.");
    }
}
