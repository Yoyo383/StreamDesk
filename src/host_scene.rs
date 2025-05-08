use eframe::egui;
use h264_reader::{
    annexb::AnnexBReader,
    nal::{Nal, RefNal, UnitType},
    push::NalInterest,
};
use remote_desktop::{protocol::MessageType, AppData, Scene, SceneChange};

use eframe::egui::PointerButton;
use remote_desktop::protocol::Message;
use std::{
    io::Read,
    net::TcpStream,
    process::{Child, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use winapi::um::winuser::{
    self, SendInput, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, MOUSEINPUT, WHEEL_DELTA,
};

use crate::menu_scene::MenuScene;

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
            "-g",
            "60",
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

fn thread_send_screen(
    mut socket: TcpStream,
    mut stdout: ChildStdout,
    stop_flag: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = AnnexBReader::accumulate(|nal: RefNal<'_>| {
            if !nal.is_complete() {
                return NalInterest::Buffer; // not ready yet
            }

            // getting nal unit type
            let nal_header = match nal.header() {
                Ok(header) => header,
                Err(_) => return NalInterest::Ignore,
            };

            let nal_type = nal_header.nal_unit_type();

            // SPS means that a keyframe is on its way
            if nal_type == UnitType::SeqParameterSet {
                let message = Message::new_merge_unready();
                message.send(&mut socket).unwrap();
            }

            // sending the NAL (with the start)
            let mut nal_bytes: Vec<u8> = vec![0x00, 0x00, 0x01];
            nal.reader()
                .read_to_end(&mut nal_bytes)
                .expect("should be able to read NAL");

            let message = Message::new_screen(nal_bytes);
            message.send(&mut socket).unwrap();

            NalInterest::Ignore
        });

        let mut buffer = [0u8; 4096];

        while !stop_flag.load(Ordering::Relaxed) {
            match stdout.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    reader.push(&buffer[..n]);
                }
                Err(e) => {
                    eprintln!("ffmpeg read error: {}", e);
                    break;
                }
            }
        }
    })
}

fn thread_read_socket(
    mut socket: TcpStream,
    stop_flag: Arc<AtomicBool>,
    usernames: Arc<Mutex<Vec<String>>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();

        while !stop_flag.load(Ordering::Relaxed) {
            let message = Message::receive(&mut socket).unwrap_or_default();

            match message.message_type {
                MessageType::Joining => {
                    let mut usernames = usernames.lock().unwrap();
                    usernames.push(
                        String::from_utf8(message.vector_data).expect("bytes should be utf8"),
                    );
                }

                MessageType::SessionExit => {
                    // remove username from usernames
                    let mut usernames = usernames.lock().unwrap();
                    let username =
                        String::from_utf8(message.vector_data).expect("bytes should be utf8");

                    usernames.retain(|name| *name != username);
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
                    send_scroll(message.general_data);
                }

                MessageType::Keyboard => {
                    send_key(message.key, message.pressed);
                }

                _ => (),
            }
        }

        socket.set_read_timeout(None).unwrap();
    })
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

fn send_key(key: u16, pressed: bool) {
    unsafe {
        let mut key_input: INPUT = std::mem::zeroed();
        key_input.type_ = INPUT_KEYBOARD;
        *key_input.u.ki_mut() = KEYBDINPUT {
            wVk: key,
            wScan: 0,
            dwFlags: if pressed { 0 } else { winuser::KEYEVENTF_KEYUP },
            time: 0,
            dwExtraInfo: 0,
        };

        let mut inputs = [key_input];
        SendInput(
            inputs.len() as u32,
            inputs.as_mut_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

pub struct HostScene {
    session_code: i32,
    stop_flag: Arc<AtomicBool>,
    usernames: Arc<Mutex<Vec<String>>>,
    username: String,
    ffmpeg_command: Child,
    thread_send_screen: Option<JoinHandle<()>>,
    thread_read_socket: Option<JoinHandle<()>>,
}

impl HostScene {
    pub fn new(session_code: i32, app_data: &mut AppData, username: String) -> Self {
        let mut command = start_ffmpeg();
        let stdout = command.stdout.take().unwrap();

        let stop_flag = Arc::new(AtomicBool::new(false));

        let socket = app_data.socket.as_mut().unwrap();

        let thread_send_screen =
            thread_send_screen(socket.try_clone().unwrap(), stdout, stop_flag.clone());

        let usernames = Arc::new(Mutex::new(vec![username.clone()]));

        let thread_read_socket = thread_read_socket(
            socket.try_clone().unwrap(),
            stop_flag.clone(),
            usernames.clone(),
        );

        Self {
            session_code,
            stop_flag,
            usernames,
            username,
            ffmpeg_command: command,
            thread_send_screen: Some(thread_send_screen),
            thread_read_socket: Some(thread_read_socket),
        }
    }

    fn disconnect(&mut self, socket: &mut TcpStream) -> SceneChange {
        self.stop_flag.store(true, Ordering::Relaxed);

        let _ = self.thread_send_screen.take().unwrap().join();
        let _ = self.thread_read_socket.take().unwrap().join();
        let _ = self.ffmpeg_command.kill();

        let message = Message::new_session_exit(&self.username);
        message.send(socket).unwrap();

        SceneChange::To(Box::new(MenuScene::new(self.username.clone())))
    }
}

impl Scene for HostScene {
    fn update(&mut self, ctx: &egui::Context, app_data: &mut AppData) -> Option<SceneChange> {
        let mut result: Option<SceneChange> = None;

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(format!("Hosting, code {}", self.session_code));
            ui.separator();

            {
                let usernames = self.usernames.lock().unwrap();
                for username in usernames.iter() {
                    ui.label(username);
                }
            }

            if ui.button("End Session").clicked() {
                result = Some(self.disconnect(app_data.socket.as_mut().unwrap()));
            }
        });

        result
    }

    fn on_exit(&mut self, app_data: &mut remote_desktop::AppData) {
        let socket = app_data.socket.as_mut().unwrap();

        self.disconnect(socket);

        let message = Message::new_shutdown();
        message.send(socket).unwrap();

        socket
            .shutdown(std::net::Shutdown::Both)
            .expect("Could not close socket.");
    }
}
