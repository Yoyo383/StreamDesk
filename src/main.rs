use eframe::egui::load::SizedTexture;
use eframe::egui::{Context, Ui};
use eframe::{egui, NativeOptions};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

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
    thread::spawn(move || loop {
        let mut len_buffer = [0u8; 8];
        socket.read_exact(&mut len_buffer).unwrap();
        let len = u64::from_be_bytes(len_buffer);

        let mut packet = vec![0u8; len as usize];
        socket.read_exact(&mut packet).unwrap();

        stdin.write_all(&packet).unwrap();
    });
}

fn thread_read_decoded(frame_queue: Arc<Mutex<VecDeque<Vec<u8>>>>, mut stdout: ChildStdout) {
    thread::spawn(move || {
        let mut rgba_buffer = vec![0u8; 1920 * 1080 * 4];
        loop {
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
    current_frame: Vec<u8>,
}

impl MyApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let socket = TcpStream::connect("127.0.0.1:7643").expect("Couldn't connect to the server.");

        let mut ffmpeg = start_ffmpeg();
        let stdin = ffmpeg.stdin.take().unwrap();
        let stdout = ffmpeg.stdout.take().unwrap();

        let frame_queue = Arc::new(Mutex::new(VecDeque::new()));
        let frame_queue_clone = frame_queue.clone();

        thread_receive_encoded(socket, stdin);
        thread_read_decoded(frame_queue_clone, stdout);

        Self {
            now: Instant::now(),
            dt: 0.,
            elapsed_time: 0.,
            frame_queue,
            current_frame: vec![0u8; 1920 * 1080 * 4],
        }
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
            if input.pointer.button_pressed(egui::PointerButton::Primary) {
                println!("{:?}", input.pointer.latest_pos());
            }
        });

        ctx.request_repaint();
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        self.dt = now.duration_since(self.now).as_secs_f32();
        self.now = now;
        self.elapsed_time += self.dt;

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                self.render(ui, ctx);
            });
    }
}

fn main() {
    let options = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([600.0 * 1920.0 / 1080.0, 600.0]),
        hardware_acceleration: eframe::HardwareAcceleration::Preferred,
        ..Default::default()
    };

    let _ = eframe::run_native(
        "Screen Capture",
        options,
        Box::new(move |cc| Ok(Box::new(MyApp::new(cc)))),
    );
}
