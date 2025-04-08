use eframe::egui::load::SizedTexture;
use eframe::egui::{Context, Event, Key, Modifiers, PointerButton, Pos2, Ui};
use eframe::{egui, NativeOptions};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;
use winapi::um::winuser::{
    self, SendInput, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, MOUSEINPUT,
    VK_CONTROL, VK_HOME, VK_SHIFT,
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

pub fn egui_key_to_vk(key: &Key) -> Option<u16> {
    use winapi::um::winuser::*;
    use Key::*;

    Some(match key {
        ArrowDown => VK_DOWN as u16,
        ArrowLeft => VK_LEFT as u16,
        ArrowRight => VK_RIGHT as u16,
        ArrowUp => VK_UP as u16,
        Escape => VK_ESCAPE as u16,
        Tab => VK_TAB as u16,
        Backspace => VK_BACK as u16,
        Enter => VK_RETURN as u16,
        Space => VK_SPACE as u16,
        Insert => VK_INSERT as u16,
        Delete => VK_DELETE as u16,
        Home => VK_HOME as u16,
        End => VK_END as u16,
        PageUp => VK_PRIOR as u16,
        PageDown => VK_NEXT as u16,
        A => 0x41,
        B => 0x42,
        C => 0x43,
        D => 0x44,
        E => 0x45,
        F => 0x46,
        G => 0x47,
        H => 0x48,
        I => 0x49,
        J => 0x4A,
        K => 0x4B,
        L => 0x4C,
        M => 0x4D,
        N => 0x4E,
        O => 0x4F,
        P => 0x50,
        Q => 0x51,
        R => 0x52,
        S => 0x53,
        T => 0x54,
        U => 0x55,
        V => 0x56,
        W => 0x57,
        X => 0x58,
        Y => 0x59,
        Z => 0x5A,
        Num0 => 0x30,
        Num1 => 0x31,
        Num2 => 0x32,
        Num3 => 0x33,
        Num4 => 0x34,
        Num5 => 0x35,
        Num6 => 0x36,
        Num7 => 0x37,
        Num8 => 0x38,
        Num9 => 0x39,
        F1 => VK_F1 as u16,
        F2 => VK_F2 as u16,
        F3 => VK_F3 as u16,
        F4 => VK_F4 as u16,
        F5 => VK_F5 as u16,
        F6 => VK_F6 as u16,
        F7 => VK_F7 as u16,
        F8 => VK_F8 as u16,
        F9 => VK_F9 as u16,
        F10 => VK_F10 as u16,
        F11 => VK_F11 as u16,
        F12 => VK_F12 as u16,
        _ => return None,
    })
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

    fn send_virtual_key(&self, vk: u16, pressed: bool) {
        unsafe {
            let mut input: INPUT = std::mem::zeroed();
            input.type_ = INPUT_KEYBOARD;
            *input.u.ki_mut() = KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if pressed { 0 } else { KEYEVENTF_KEYUP },
                time: 0,
                dwExtraInfo: 0,
            };

            let mut inputs = [input];
            SendInput(
                inputs.len() as u32,
                inputs.as_mut_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            );
        }
    }

    fn send_key_input_pressed(&self, vk: u16, modifiers: &Modifiers) {
        if modifiers.ctrl {
            self.send_virtual_key(VK_CONTROL as u16, true);
        }
        if modifiers.alt {
            self.send_virtual_key(VK_HOME as u16, true);
        }
        if modifiers.shift {
            self.send_virtual_key(VK_SHIFT as u16, true);
        }

        self.send_virtual_key(vk, true);
    }

    fn send_key_input_released(&self, vk: u16, modifiers: &Modifiers) {
        self.send_virtual_key(vk, false);

        if modifiers.ctrl {
            self.send_virtual_key(VK_CONTROL as u16, false);
        }
        if modifiers.alt {
            self.send_virtual_key(VK_HOME as u16, false);
        }
        if modifiers.shift {
            self.send_virtual_key(VK_SHIFT as u16, false);
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

            for event in &input.raw.events {
                match event {
                    Event::Key {
                        key,
                        pressed,
                        modifiers,
                        ..
                    } => {
                        if let Some(vk) = egui_key_to_vk(key) {
                            if *pressed {
                                self.send_key_input_pressed(vk, modifiers);
                            } else {
                                self.send_key_input_released(vk, modifiers);
                            }
                        }
                    }
                    _ => (),
                }
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
