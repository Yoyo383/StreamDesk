use eframe::egui::PointerButton;
use remote_desktop::protocol::{Message, MessageType};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, ChildStdout, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Instant,
};
use winapi::um::winuser::{self, SendInput, INPUT, INPUT_MOUSE, MOUSEINPUT};

fn start_ffmpeg() -> Child {
    let ffmpeg = Command::new("ffmpeg")
        .args(&[
            "-f",
            "gdigrab",
            "-framerate",
            "30",
            "-draw_mouse",
            "0",
            "-i",
            "desktop",
            "-vcodec",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-x264opts",
            "no-scenecut",
            "-sc_threshold",
            "0",
            "-f",
            "h264",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start FFmpeg");

    ffmpeg
}

fn thread_read_encoded(mut socket: TcpStream, mut stdout: ChildStdout) {
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while !STOP.load(Ordering::Relaxed) {
            match stdout.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    socket.write_all(&(n as u64).to_be_bytes()).unwrap();
                    socket.write_all(&buffer[..n]).unwrap();
                }
                Err(e) => {
                    eprintln!("ffmpeg read error: {}", e);
                    break;
                }
            }
        }
    });
}

fn send_mouse_click(mouse_position: (i32, i32), button: PointerButton) {
    unsafe {
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
                mi.dx = mouse_position.0;
                mi.dy = mouse_position.1;
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

fn handle_connection(mut socket: TcpStream) {
    let mut command = start_ffmpeg();
    let stdout = command.stdout.take().unwrap();

    let mut now = Instant::now();

    thread_read_encoded(socket.try_clone().unwrap(), stdout);

    let mut buffer = vec![0u8; Message::size()];

    loop {
        let new_now = Instant::now();
        let dt = new_now.duration_since(now).as_secs_f32();
        now = new_now;

        socket.read_exact(&mut buffer).unwrap();

        let message = Message::from_bytes(&buffer).unwrap();
        println!("{:?}", message);

        if message.message_type == MessageType::Shutdown {
            STOP.store(true, Ordering::Relaxed);
            socket
                .shutdown(std::net::Shutdown::Both)
                .expect("Could not close socket.");
            break;
        }

        if message.message_type == MessageType::Mouse {
            if !message.pressed {
                send_mouse_click(message.mouse_position, message.mouse_button);
            }
        }
    }
}

static STOP: AtomicBool = AtomicBool::new(false);

fn main() {
    let listener = TcpListener::bind("0.0.0.0:7643").expect("Could not bind listener");
    match listener.accept() {
        Ok((socket, _addr)) => handle_connection(socket),
        Err(e) => println!("Couldn't accept client: {e:?}"),
    }
}
