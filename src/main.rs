use eframe::egui::load::SizedTexture;
use eframe::egui::{Context, Pos2, Ui};
use eframe::{egui, NativeOptions};
use modifiers_state::ModifiersState;
use remote_desktop::egui_key_to_vk;
use remote_desktop::protocol::Message;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;
mod modifiers_state;

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

fn thread_receive_encoded(mut socket: TcpStream, mut stdin: ChildStdin) {
    thread::spawn(move || {
        while !STOP.load(Ordering::Relaxed) {
            let message = Message::receive(&mut socket).unwrap();
            stdin.write_all(&message.screen_data).unwrap();
        }
    });
}

fn thread_read_decoded(frame_queue: Arc<Mutex<VecDeque<Vec<u8>>>>, mut stdout: ChildStdout) {
    thread::spawn(move || {
        let mut rgba_buffer = vec![0u8; 1920 * 1080 * 4];
        while !STOP.load(Ordering::Relaxed) {
            stdout.read_exact(&mut rgba_buffer).unwrap();

            let mut queue = frame_queue.lock().unwrap();

            if queue.len() > 1 {
                queue.pop_front();
            }
            queue.push_back(rgba_buffer.clone());
        }
    });
}

struct MyApp {
    now: Instant,
    dt: f32,
    elapsed_time: f32,
    frame_queue: Arc<Mutex<VecDeque<Vec<u8>>>>,
    socket: TcpStream,
    current_frame: Vec<u8>,
    width: f32,
    height: f32,
    modifiers_state: ModifiersState,
}

impl MyApp {
    fn new(_cc: &eframe::CreationContext<'_>, width: f32, height: f32) -> Self {
        let socket = TcpStream::connect("127.0.0.1:7643").expect("Couldn't connect to the server.");

        let mut ffmpeg = start_ffmpeg();
        let stdin = ffmpeg.stdin.take().unwrap();
        let stdout = ffmpeg.stdout.take().unwrap();

        let frame_queue = Arc::new(Mutex::new(VecDeque::new()));
        let frame_queue_clone = frame_queue.clone();

        thread_receive_encoded(socket.try_clone().unwrap(), stdin);
        thread_read_decoded(frame_queue_clone, stdout);

        Self {
            now: Instant::now(),
            dt: 0.,
            elapsed_time: 0.,
            frame_queue,
            socket,
            current_frame: vec![0u8; 1920 * 1080 * 4],
            width,
            height,
            modifiers_state: ModifiersState::new(),
        }
    }

    fn update_size(&mut self, ctx: &Context) {
        let size = ctx.screen_rect().size();
        if self.width != size.x {
            self.width = size.x;
        }
        if self.height != size.y {
            self.height = size.y;
        }
    }

    fn normalize_mouse_position(&self, mouse_position: Pos2) -> (i32, i32) {
        let x = (mouse_position.x / self.width) * 65535.0;
        let y = (mouse_position.y / self.height) * 65535.0;
        (x as i32, y as i32)
    }

    fn render(&mut self, ui: &mut Ui, ctx: &Context) {
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

        ui.input(|input| {
            self.modifiers_state.update(input);

            for key in &self.modifiers_state.keys {
                let message = Message::new_keyboard(key.key, key.pressed);
                message.send(&mut self.socket).unwrap();
            }

            for event in &input.events {
                match event {
                    egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        ..
                    } => {
                        let mouse_position = self.normalize_mouse_position(*pos);
                        let message = Message::new_mouse_click(*button, mouse_position, *pressed);
                        message.send(&mut self.socket).unwrap();
                    }

                    egui::Event::Key {
                        physical_key,
                        pressed,
                        ..
                    } => {
                        if let Some(key) = physical_key {
                            let vk = egui_key_to_vk(key).unwrap();
                            let message = Message::new_keyboard(vk, *pressed);
                            message.send(&mut self.socket).unwrap();
                        }
                    }

                    egui::Event::PointerMoved(new_pos) => {
                        let mouse_position = self.normalize_mouse_position(*new_pos);
                        let message = Message::new_mouse_move(mouse_position);
                        message.send(&mut self.socket).unwrap();
                    }

                    _ => (),
                }
            }
        });

        ctx.request_repaint();
    }
}

impl Drop for MyApp {
    fn drop(&mut self) {
        STOP.store(true, Ordering::Relaxed);
        let message = Message::new_shutdown();
        message.send(&mut self.socket).unwrap();

        self.socket
            .shutdown(std::net::Shutdown::Both)
            .expect("Could not close socket.");
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        self.dt = now.duration_since(self.now).as_secs_f32();
        self.now = now;
        self.elapsed_time += self.dt;

        self.update_size(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                self.render(ui, ctx);
            });
    }
}

static STOP: AtomicBool = AtomicBool::new(false);

fn main() {
    let (width, height): (f32, f32) = (600.0 * 1920.0 / 1080.0, 600.0);
    let options = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([600.0 * 1920.0 / 1080.0, 600.0]),
        hardware_acceleration: eframe::HardwareAcceleration::Preferred,
        ..Default::default()
    };

    let _ = eframe::run_native(
        "Screen Capture",
        options,
        Box::new(move |cc| Ok(Box::new(MyApp::new(cc, width, height)))),
    );
}
