use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::TcpStream,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::Instant,
};

use eframe::egui::{self, pos2, Color32, ImageSource, Rect, Sense, Stroke, Ui, Vec2};
use remote_desktop::{protocol::Packet, AppData, Scene, SceneChange};

use crate::menu_scene::MenuScene;

const PLAY_IMAGE: ImageSource = egui::include_image!("../images/play.svg");
const PAUSE_IMAGE: ImageSource = egui::include_image!("../images/pause.svg");

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

fn thread_receive_socket(mut socket: TcpStream, stdin: ChildStdin) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut stdin = Some(stdin);

        loop {
            let packet = Packet::receive(&mut socket).unwrap_or_default();

            match packet {
                Packet::Screen { bytes } => {
                    if let Some(ref mut stdin) = stdin {
                        let _ = stdin.write_all(&bytes);
                    }
                }

                Packet::None => {
                    stdin = None;
                }

                Packet::SessionExit => break,

                _ => (),
            }
        }
    })
}

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

pub struct WatchScene {
    now: Instant,
    elapsed_time: f32,

    username: String,
    duration: u32,
    current_frame_number: u32,

    stop_flag: Arc<AtomicBool>,
    is_paused: bool,

    frame_queue: Arc<(Mutex<VecDeque<Vec<u8>>>, Condvar)>,
    current_frame: Vec<u8>,

    thread_receive_socket: Option<JoinHandle<()>>,
    thread_read_decoded: Option<JoinHandle<()>>,
    ffmpeg_command: Child,
}

impl WatchScene {
    pub fn new(username: String, duration: u32, socket: &mut TcpStream) -> Self {
        let mut ffmpeg = start_ffmpeg();
        let stdin = ffmpeg.stdin.take().unwrap();
        let stdout = ffmpeg.stdout.take().unwrap();

        let frame_queue = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));

        let stop_flag = Arc::new(AtomicBool::new(false));

        let thread_receive_socket = thread_receive_socket(socket.try_clone().unwrap(), stdin);
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

    fn exit(&mut self, socket: &mut TcpStream) -> SceneChange {
        let session_exit = Packet::SessionExit;
        session_exit.send(socket).unwrap();

        self.stop_flag.store(true, Ordering::Relaxed);

        {
            let (_, condvar) = &*self.frame_queue;
            condvar.notify_all(); // wake up decoder thread in case it's waiting
        }

        let _ = self.thread_receive_socket.take().unwrap().join();
        let _ = self.thread_read_decoded.take().unwrap().join();
        let _ = self.ffmpeg_command.kill();

        SceneChange::To(Box::new(MenuScene::new(self.username.clone(), socket)))
    }
}

impl Scene for WatchScene {
    fn update(&mut self, ctx: &egui::Context, app_data: &mut AppData) -> SceneChange {
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

                    ui.label(time_string);
                    let progress_bar = egui::ProgressBar::new(progress);
                    ui.add(progress_bar);
                });

                ui.vertical_centered(|ui| {
                    if ui.button("Exit").clicked() {
                        result = self.exit(app_data.socket.as_mut().unwrap());
                    }
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.central_panel_ui(ui, ctx);
        });

        result
    }

    fn on_exit(&mut self, app_data: &mut AppData) {
        let socket = app_data.socket.as_mut().unwrap();

        self.exit(socket);

        let signout_packet = Packet::SignOut;
        signout_packet.send(socket).unwrap();

        let shutdown_packet = Packet::Shutdown;
        shutdown_packet.send(socket).unwrap();

        socket
            .shutdown(std::net::Shutdown::Both)
            .expect("Could not close socket.");
    }
}
