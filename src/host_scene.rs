use eframe::egui;
use h264_reader::{
    annexb::AnnexBReader,
    nal::{Nal, RefNal, UnitType},
    push::NalInterest,
};
use remote_desktop::{chat_ui, protocol::ControlPayload, users_list, Scene, SceneChange, UserType};

use eframe::egui::PointerButton;
use remote_desktop::protocol::Packet;
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    net::TcpStream,
    process::{Child, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
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
                let merge_unready = Packet::MergeUnready;
                merge_unready.send(&mut socket).unwrap();
            }

            // sending the NAL (with the start)
            let mut nal_bytes: Vec<u8> = vec![0x00, 0x00, 0x01];
            nal.reader()
                .read_to_end(&mut nal_bytes)
                .expect("should be able to read NAL");

            let screen_packet = Packet::Screen { bytes: nal_bytes };
            screen_packet.send(&mut socket).unwrap();

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
    usernames: Arc<Mutex<HashMap<String, UserType>>>,
    requesting_control: Arc<Mutex<HashSet<String>>>,
    requesting_join: Arc<Mutex<HashSet<String>>>,
    chat_log: Arc<Mutex<Vec<String>>>,
) -> JoinHandle<()> {
    thread::spawn(move || loop {
        let packet = Packet::receive(&mut socket).unwrap_or_default();

        match packet {
            Packet::Join { username, .. } => {
                let mut requesting_join = requesting_join.lock().unwrap();
                requesting_join.insert(username);
            }

            Packet::UserUpdate {
                user_type,
                username,
                ..
            } => {
                let mut usernames = usernames.lock().unwrap();
                if user_type == UserType::Leaving {
                    usernames.remove(&username);

                    let mut chat_log = chat_log.lock().unwrap();
                    chat_log.push(format!("#r{} has disconnected.", username));
                } else {
                    let mut chat_log = chat_log.lock().unwrap();

                    if usernames.contains_key(&username) {
                        chat_log.push(format!("#b{} is now a {}.", username, user_type));
                    } else {
                        chat_log.push(format!("#g{} has joined the session.", username));
                    }

                    usernames.insert(username.clone(), user_type);
                }
            }

            Packet::Control { payload } => match payload {
                ControlPayload::MouseMove { mouse_x, mouse_y } => send_mouse_move(mouse_x, mouse_y),

                ControlPayload::MouseClick {
                    mouse_x,
                    mouse_y,
                    pressed,
                    button,
                } => send_mouse_click(mouse_x, mouse_y, button, pressed),

                ControlPayload::Keyboard { pressed, key } => send_key(key, pressed),

                ControlPayload::Scroll { delta } => send_scroll(delta),
            },

            Packet::RequestControl { username } => {
                let mut requesting_control = requesting_control.lock().unwrap();
                requesting_control.insert(username);
            }

            Packet::Chat { message } => {
                let mut chat_log = chat_log.lock().unwrap();
                chat_log.push(message);
            }

            Packet::SessionEnd => break,

            _ => (),
        }
    })
}

fn send_mouse_move(mouse_x: u32, mouse_y: u32) {
    unsafe {
        let mut move_input: INPUT = std::mem::zeroed();
        move_input.type_ = INPUT_MOUSE;
        *move_input.u.mi_mut() = MOUSEINPUT {
            dx: mouse_x as i32,
            dy: mouse_y as i32,
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

fn send_mouse_click(mouse_x: u32, mouse_y: u32, button: PointerButton, pressed: bool) {
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
            dx: mouse_x as i32,
            dy: mouse_y as i32,
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
    session_code: String,
    stop_flag: Arc<AtomicBool>,

    usernames: Arc<Mutex<HashMap<String, UserType>>>,
    username: String,
    requesting_control: Arc<Mutex<HashSet<String>>>,
    requesting_join: Arc<Mutex<HashSet<String>>>,

    chat_log: Arc<Mutex<Vec<String>>>,
    chat_message: String,

    ffmpeg_command: Child,
    thread_send_screen: Option<JoinHandle<()>>,
    thread_read_socket: Option<JoinHandle<()>>,
}

impl HostScene {
    pub fn new(session_code: String, socket: &mut TcpStream, username: String) -> Self {
        let mut command = start_ffmpeg();
        let stdout = command.stdout.take().unwrap();

        let stop_flag = Arc::new(AtomicBool::new(false));

        let thread_send_screen =
            thread_send_screen(socket.try_clone().unwrap(), stdout, stop_flag.clone());

        let mut usernames_types = HashMap::new();
        usernames_types.insert(username.clone(), UserType::Host);

        let usernames = Arc::new(Mutex::new(usernames_types));
        let requesting_control = Arc::new(Mutex::new(HashSet::new()));
        let requesting_join = Arc::new(Mutex::new(HashSet::new()));

        let chat_log = Arc::new(Mutex::new(Vec::new()));

        let thread_read_socket = thread_read_socket(
            socket.try_clone().unwrap(),
            usernames.clone(),
            requesting_control.clone(),
            requesting_join.clone(),
            chat_log.clone(),
        );

        Self {
            session_code,
            stop_flag,

            usernames,
            username,
            requesting_control,
            requesting_join,

            chat_log,
            chat_message: String::new(),

            ffmpeg_command: command,
            thread_send_screen: Some(thread_send_screen),
            thread_read_socket: Some(thread_read_socket),
        }
    }

    fn disconnect(&mut self, socket: &mut TcpStream) -> SceneChange {
        self.stop_flag.store(true, Ordering::Relaxed);

        let _ = self.thread_send_screen.take().unwrap().join();
        let _ = self.ffmpeg_command.kill();

        let session_exit = Packet::SessionExit;
        session_exit.send(socket).unwrap();

        let _ = self.thread_read_socket.take().unwrap().join();

        SceneChange::To(Box::new(MenuScene::new(self.username.clone(), socket)))
    }
}

impl Scene for HostScene {
    fn update(&mut self, ctx: &egui::Context, socket: &mut Option<TcpStream>) -> SceneChange {
        let socket = socket.as_mut().unwrap();
        let mut result: SceneChange = SceneChange::None;

        egui::SidePanel::right("chat").show(ctx, |ui| {
            chat_ui(
                ui,
                self.chat_log.lock().unwrap(),
                &mut self.chat_message,
                socket,
            );
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(format!("Hosting, code {}", self.session_code));
            ui.separator();

            if let Some(controller) = users_list(
                ui,
                self.usernames.lock().unwrap(),
                self.username.clone(),
                true,
            ) {
                let deny_packet = Packet::DenyControl {
                    username: controller,
                };
                deny_packet.send(socket).unwrap();
            }

            {
                let mut requesting_control = self.requesting_control.lock().unwrap();
                let mut user_handled = String::new();
                let mut was_allowed = false;

                for user in requesting_control.iter() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} is requesting control.", user));

                        if ui.button("Allow").clicked() {
                            user_handled = user.to_string();
                            was_allowed = true;
                        }

                        if ui.button("Deny").clicked() {
                            user_handled = user.to_string();

                            let deny_packet = Packet::DenyControl {
                                username: user.to_string(),
                            };
                            deny_packet.send(socket).unwrap();
                        }
                    });
                }

                if !user_handled.is_empty() {
                    requesting_control.remove(&user_handled);
                }

                if was_allowed {
                    // find current controller
                    let usernames = self.usernames.lock().unwrap();
                    let controller = usernames
                        .iter()
                        .find(|(_, user_type)| **user_type == UserType::Controller);

                    // if found controller send Deny
                    if let Some((controller, _)) = controller {
                        let deny_packet = Packet::DenyControl {
                            username: controller.to_string(),
                        };
                        deny_packet.send(socket).unwrap();
                    }

                    // send to allowed user
                    let allow_packet = Packet::RequestControl {
                        username: user_handled.to_string(),
                    };
                    allow_packet.send(socket).unwrap();

                    // send Deny to all other users and clear
                    for user in requesting_control.iter() {
                        let deny_packet = Packet::DenyControl {
                            username: user.to_string(),
                        };
                        deny_packet.send(socket).unwrap();
                    }

                    // clear requesting users
                    requesting_control.clear();
                }
            }

            // requesting join
            {
                let mut requesting_join = self.requesting_join.lock().unwrap();
                let mut user_handled = String::new();

                for user in requesting_join.iter() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} is requesting to join.", user));

                        if ui.button("Allow").clicked() {
                            user_handled = user.to_string();

                            let packet = Packet::Join {
                                code: 0,
                                username: user.to_string(),
                            };
                            packet.send(socket).unwrap();
                        }

                        if ui.button("Deny").clicked() {
                            user_handled = user.to_string();

                            let deny_packet = Packet::DenyJoin {
                                username: user.to_string(),
                            };
                            deny_packet.send(socket).unwrap();
                        }
                    });
                }

                if !user_handled.is_empty() {
                    requesting_join.remove(&user_handled);
                }
            }

            if ui.button("End Session").clicked() {
                result = self.disconnect(socket);
            }
        });

        result
    }

    fn on_exit(&mut self, socket: &mut Option<TcpStream>) {
        let socket = socket.as_mut().unwrap();

        self.disconnect(socket);

        let signout_packet = Packet::SignOut;
        signout_packet.send(socket).unwrap();

        let shutdown_packet = Packet::Shutdown;
        shutdown_packet.send(socket).unwrap();

        socket
            .shutdown(std::net::Shutdown::Both)
            .expect("Could not close socket.");
    }
}
