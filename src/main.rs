use eframe::egui::load::SizedTexture;
use eframe::egui::{Context, PointerButton, Pos2, Ui};
use eframe::{egui, NativeOptions};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;
use winapi::um::winuser::{self, SendInput, INPUT, INPUT_MOUSE, MOUSEINPUT};

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
    width: f32,
    height: f32,
}

impl MyApp {
    fn new(_cc: &eframe::CreationContext<'_>, width: f32, height: f32) -> Self {
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
            width,
            height,
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

    fn send_mouse_click(&self, mouse_position: Pos2, button: PointerButton) {
        unsafe {
            let (mouse_x, mouse_y) = self.normalize_mouse_position(mouse_position);
            // Currently also moves the cursor, will be changed in the future so that
            // the cursor always moved to the right location
            let mut flags: u32 = winuser::MOUSEEVENTF_ABSOLUTE | winuser::MOUSEEVENTF_MOVE;
            if button == PointerButton::Primary {
                flags |= winuser::MOUSEEVENTF_LEFTDOWN | winuser::MOUSEEVENTF_LEFTUP;
            } else if button == PointerButton::Secondary {
                flags |= winuser::MOUSEEVENTF_RIGHTDOWN | winuser::MOUSEEVENTF_RIGHTUP;
            } else if button == PointerButton::Middle {
                flags |= winuser::MOUSEEVENTF_MIDDLEDOWN | winuser::MOUSEEVENTF_MIDDLEUP;
            }

            let click_up_input = INPUT {
                type_: INPUT_MOUSE,
                u: {
                    let mut mi = std::mem::zeroed::<MOUSEINPUT>();
                    mi.dx = mouse_x;
                    mi.dy = mouse_y;
                    mi.mouseData = 0;
                    mi.dwFlags = flags;
                    mi.time = 0;
                    mi.dwExtraInfo = 0;
                    std::mem::transmute(mi)
                },
            };

            let mut inputs = [click_up_input];
            SendInput(
                inputs.len() as u32,
                inputs.as_mut_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            );
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
            if input.pointer.primary_released() {
                self.send_mouse_click(input.pointer.latest_pos().unwrap(), PointerButton::Primary);
            }
            if input.pointer.secondary_released() {
                self.send_mouse_click(
                    input.pointer.latest_pos().unwrap(),
                    PointerButton::Secondary,
                );
            }
            if input.pointer.button_released(PointerButton::Middle) {
                self.send_mouse_click(input.pointer.latest_pos().unwrap(), PointerButton::Middle);
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

        self.update_size(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                self.render(ui, ctx);
            });
    }
}

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
