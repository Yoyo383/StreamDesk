use eframe::egui::load::SizedTexture;
use eframe::egui::{Context, TextureHandle, Ui};
use eframe::{egui, NativeOptions};
use std::io::Read;
use std::net::TcpStream;
use std::time::Instant;

fn main() {
    let options = NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([600.0, 400.0]),
        hardware_acceleration: eframe::HardwareAcceleration::Preferred,
        ..Default::default()
    };

    let _ = eframe::run_native(
        "Screen Capture",
        options,
        Box::new(move |cc| Ok(Box::new(MyApp::new(cc)))),
    );
}

struct MyApp {
    now: Instant,
    dt: f32,
    socket: TcpStream,
}

impl MyApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let socket = TcpStream::connect("127.0.0.1:7643").expect("Couldn't connect to the server.");

        Self {
            now: Instant::now(),
            dt: 0.,
            socket,
        }
    }

    fn render(&mut self, ui: &mut Ui, ctx: &Context, handle: TextureHandle) {
        ui.style_mut().spacing.indent = 0.;
        let image = SizedTexture::new(&handle, ui.available_size());
        ui.image(image);

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

        let mut size_buffer = [0u8; 8];
        self.socket.read_exact(&mut size_buffer).unwrap();
        let size = u64::from_be_bytes(size_buffer);

        let mut pixels = vec![0u8; size as usize];
        self.socket.read_exact(&mut pixels).unwrap();

        let texture = egui::ColorImage::from_rgba_unmultiplied([1920, 1080], &pixels);
        let handle = ctx.load_texture("screen", texture, egui::TextureOptions::default());

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                self.render(ui, ctx, handle);
            });
    }
}
