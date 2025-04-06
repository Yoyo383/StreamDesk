use eframe::egui::load::SizedTexture;
use eframe::egui::{Context, Ui};
use eframe::{egui, NativeOptions};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

fn start_ffmpeg() -> Child {
    let ffmpeg = Command::new("ffmpeg")
        .args([
            "-f", "h264", "-i", "-", "-f", "rawvideo", "-pix_fmt", "rgba", "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn ffmpeg");

    ffmpeg
}

fn thread_read_decoded(latest_frame: Arc<Mutex<Option<Vec<u8>>>>, mut stdout: ChildStdout) {
    thread::spawn(move || {
        let mut rgba_buffer = vec![0u8; 1920 * 1080 * 4];
        loop {
            stdout.read_exact(&mut rgba_buffer).unwrap();

            *latest_frame.lock().unwrap() = Some(rgba_buffer.clone());
        }
    });
}

struct MyApp {
    now: Instant,
    dt: f32,
    socket: TcpStream,
    stdin: ChildStdin,
    latest_frame: Arc<Mutex<Option<Vec<u8>>>>,
}

impl MyApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let socket = TcpStream::connect("127.0.0.1:7643").expect("Couldn't connect to the server.");

        let mut ffmpeg = start_ffmpeg();
        let stdin = ffmpeg.stdin.take().unwrap();
        let stdout = ffmpeg.stdout.take().unwrap();

        let latest_frame = Arc::new(Mutex::new(None));
        let latest_frame_clone = latest_frame.clone();

        thread_read_decoded(latest_frame_clone, stdout);

        Self {
            now: Instant::now(),
            dt: 0.,
            socket,
            stdin,
            latest_frame,
        }
    }

    fn render(&mut self, ui: &mut Ui, ctx: &Context) {
        if let Some(image) = &*self.latest_frame.lock().unwrap() {
            let texture = egui::ColorImage::from_rgba_unmultiplied([1920, 1080], image);
            let handle = ctx.load_texture("screen", texture, egui::TextureOptions::default());
            let sized_texture = SizedTexture::new(&handle, ui.available_size());
            ui.image(sized_texture);
        }

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

        // println!("{}", 1. / self.dt);

        let mut len_buffer = [0u8; 8];
        self.socket.read_exact(&mut len_buffer).unwrap();
        let len = u64::from_be_bytes(len_buffer);

        let mut packet = vec![0u8; len as usize];
        self.socket.read_exact(&mut packet).unwrap();

        self.stdin.write_all(&packet).unwrap();

        // let pixels = png_to_rgba(png_data);

        // let texture = egui::ColorImage::from_rgba_unmultiplied([1920, 1080], &pixels);
        // let handle = ctx.load_texture("screen", texture, egui::TextureOptions::default());

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
