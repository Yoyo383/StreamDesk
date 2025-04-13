use eframe::egui::PointerButton;
use remote_desktop::protocol::{Message, MessageType};
use std::{
    io::Read,
    net::{TcpListener, TcpStream},
    process::{Child, ChildStdout, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
};
use winapi::um::winuser::{self, SendInput, INPUT, INPUT_MOUSE, MOUSEINPUT, WHEEL_DELTA};

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
                    let message = Message::new_screen(buffer[..n].to_vec());
                    message.send(&mut socket).unwrap();
                }
                Err(e) => {
                    eprintln!("ffmpeg read error: {}", e);
                    break;
                }
            }
        }
    });
}

fn send_mouse_move(mouse_position: (i32, i32)) {
    unsafe {
        let mut move_input: INPUT = std::mem::zeroed();
        move_input.type_ = INPUT_MOUSE;
        *move_input.u.mi_mut() = MOUSEINPUT {
            dx: mouse_position.0,
            dy: mouse_position.1,
            mouseData: 0,
            dwFlags: winuser::MOUSEEVENTF_ABSOLUTE | winuser::MOUSEEVENTF_MOVE,
            time: 0,
            dwExtraInfo: 0,
        };

        let mut inputs = [move_input];
        SendInput(
            inputs.len() as u32,
            inputs.as_mut_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

fn send_mouse_click(mouse_position: (i32, i32), button: PointerButton, pressed: bool) {
    unsafe {
        let mut flags: u32 = winuser::MOUSEEVENTF_ABSOLUTE | winuser::MOUSEEVENTF_MOVE;
        if button == PointerButton::Primary {
            if pressed {
                flags |= winuser::MOUSEEVENTF_LEFTDOWN;
            } else {
                flags |= winuser::MOUSEEVENTF_LEFTUP;
            }
        } else if button == PointerButton::Secondary {
            if pressed {
                flags |= winuser::MOUSEEVENTF_RIGHTDOWN;
            } else {
                flags |= winuser::MOUSEEVENTF_RIGHTUP;
            }
        } else if button == PointerButton::Middle {
            if pressed {
                flags |= winuser::MOUSEEVENTF_MIDDLEDOWN;
            } else {
                flags |= winuser::MOUSEEVENTF_MIDDLEUP;
            }
        }

        let mut click_up_input: INPUT = std::mem::zeroed();

        click_up_input.type_ = INPUT_MOUSE;
        *click_up_input.u.mi_mut() = MOUSEINPUT {
            dx: mouse_position.0,
            dy: mouse_position.1,
            mouseData: 0,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        };

        let mut inputs = [click_up_input];
        SendInput(
            inputs.len() as u32,
            inputs.as_mut_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

fn send_scroll(delta: i32) {
    unsafe {
        let mut scroll_input: INPUT = std::mem::zeroed();
        scroll_input.type_ = INPUT_MOUSE;
        *scroll_input.u.mi_mut() = MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: (delta * WHEEL_DELTA as i32) as u32,
            dwFlags: winuser::MOUSEEVENTF_WHEEL,
            time: 0,
            dwExtraInfo: 0,
        };

        let mut inputs = [scroll_input];
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

    thread_read_encoded(socket.try_clone().unwrap(), stdout);

    loop {
        let message = Message::receive(&mut socket).unwrap();

        // println!("{:?}", message);

        match message.message_type {
            MessageType::Shutdown => {
                STOP.store(true, Ordering::Relaxed);
                socket
                    .shutdown(std::net::Shutdown::Both)
                    .expect("Could not close socket.");
            }

            MessageType::MouseClick => {
                send_mouse_click(
                    message.mouse_position,
                    message.mouse_button,
                    message.pressed,
                );
            }

            MessageType::MouseMove => {
                send_mouse_move(message.mouse_position);
            }

            MessageType::Scroll => {
                send_scroll(message.mouse_position.1);
            }

            _ => (),
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
