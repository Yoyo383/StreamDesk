use eframe::egui::{self, pos2, Color32, Rect, Sense, Stroke, Ui, Vec2};
use remote_desktop::protocol::{ControlPayload, Packet};
use remote_desktop::secure_channel::SecureChannel;
use remote_desktop::{
    chat_ui, egui_key_to_vk, normalize_mouse_position, users_list, Scene, SceneChange, UserType,
};

use crate::{menu_scene::MenuScene, modifiers_state::ModifiersState};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Instant,
};

const REQUEST_CONTROL_MSG: &'static str = "Request Control";
const WAITING_CONTROL_MSG: &'static str = "Waiting for response...";
const CONTROLLING_MSG: &'static str = "You're the controller!";

#[derive(PartialEq, Eq)]
enum RightPanelType {
    UsersList,
    Chat,
}

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

fn thread_receive_socket(
    mut channel: SecureChannel,
    mut stdin: ChildStdin,
    stop_flag: Arc<AtomicBool>,
    usernames: Arc<Mutex<HashMap<String, UserType>>>,
    control_msg: Arc<Mutex<String>>,
    chat_log: Arc<Mutex<Vec<String>>>,
) -> JoinHandle<()> {
    thread::spawn(move || loop {
        let packet = channel.receive().unwrap_or_default();

        match packet {
            Packet::Screen { bytes } => stdin.write_all(&bytes).unwrap(),

            Packet::UserUpdate {
                user_type,
                joined_before,
                username,
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
                    } else if !joined_before {
                        chat_log.push(format!("#g{} has joined the session.", username));
                    }

                    usernames.insert(username.clone(), user_type);
                }
            }

            Packet::RequestControl { .. } => {
                let mut control_msg = control_msg.lock().unwrap();
                *control_msg = CONTROLLING_MSG.to_string();
            }

            Packet::DenyControl { .. } => {
                let mut control_msg = control_msg.lock().unwrap();

                if *control_msg != CONTROLLING_MSG {
                    let mut chat_log = chat_log.lock().unwrap();
                    chat_log.push("#rYour control request was denied by the host.".to_string());
                }

                *control_msg = REQUEST_CONTROL_MSG.to_string();
            }

            Packet::SessionExit => {
                // signal all threads
                stop_flag.store(true, Ordering::Relaxed);
                break;
            }

            Packet::SessionEnd => {
                stop_flag.store(true, Ordering::Relaxed);
                channel.send(packet).unwrap();
                break;
            }

            Packet::Chat { message } => {
                let mut chat_log = chat_log.lock().unwrap();
                chat_log.push(message);
            }

            _ => (),
        }
    })
}

fn thread_read_decoded(
    frame_queue: Arc<Mutex<VecDeque<Vec<u8>>>>,
    mut stdout: ChildStdout,
    stop_flag: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut rgba_buffer = vec![0u8; 1920 * 1080 * 4];
        while !stop_flag.load(Ordering::Relaxed) {
            if let Ok(()) = stdout.read_exact(&mut rgba_buffer) {
                let mut queue = frame_queue.lock().unwrap();

                if queue.len() > 3 {
                    queue.pop_front();
                }
                queue.push_back(rgba_buffer.clone());
            }
        }
    })
}

pub struct MainScene {
    now: Instant,
    elapsed_time: f32,

    frame_queue: Arc<Mutex<VecDeque<Vec<u8>>>>,
    current_frame: Vec<u8>,

    modifiers_state: ModifiersState,
    stop_flag: Arc<AtomicBool>,
    image_rect: Rect,
    right_panel_type: RightPanelType,

    usernames: Arc<Mutex<HashMap<String, UserType>>>,
    username: String,
    control_msg: Arc<Mutex<String>>,

    chat_log: Arc<Mutex<Vec<String>>>,
    chat_message: String,

    thread_receive_socket: Option<JoinHandle<()>>,
    thread_read_decoded: Option<JoinHandle<()>>,
    ffmpeg_command: Child,
}

impl MainScene {
    pub fn new(channel: &mut SecureChannel, username: String) -> Self {
        let mut ffmpeg = start_ffmpeg();
        let stdin = ffmpeg.stdin.take().unwrap();
        let stdout = ffmpeg.stdout.take().unwrap();

        let frame_queue = Arc::new(Mutex::new(VecDeque::new()));
        let frame_queue_clone = frame_queue.clone();

        let stop_flag = Arc::new(AtomicBool::new(false));

        let usernames = Arc::new(Mutex::new(HashMap::new()));
        let control_msg = Arc::new(Mutex::new("Request Control".to_owned()));

        let chat_log = Arc::new(Mutex::new(Vec::new()));

        let thread_receive_socket = thread_receive_socket(
            channel.clone(),
            stdin,
            stop_flag.clone(),
            usernames.clone(),
            control_msg.clone(),
            chat_log.clone(),
        );
        let thread_read_decoded = thread_read_decoded(frame_queue_clone, stdout, stop_flag.clone());

        Self {
            now: Instant::now(),
            elapsed_time: 0.,

            frame_queue,
            current_frame: vec![0u8; 1920 * 1080 * 4],

            modifiers_state: ModifiersState::new(),
            stop_flag,
            image_rect: Rect {
                min: pos2(0.0, 0.0),
                max: pos2(0.0, 0.0),
            },
            right_panel_type: RightPanelType::UsersList,

            usernames,
            username,
            control_msg,

            chat_log,
            chat_message: String::new(),

            thread_receive_socket: Some(thread_receive_socket),
            thread_read_decoded: Some(thread_read_decoded),
            ffmpeg_command: ffmpeg,
        }
    }

    fn handle_input(&mut self, input: &egui::InputState, channel: &mut SecureChannel) {
        self.modifiers_state.update(input);

        for key in &self.modifiers_state.keys {
            let key_packet = Packet::Control {
                payload: ControlPayload::Keyboard {
                    pressed: key.pressed,
                    key: key.key,
                },
            };
            channel.send(key_packet).unwrap();
        }

        for event in &input.events {
            match event {
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    ..
                } => {
                    let (mouse_x, mouse_y) = normalize_mouse_position(*pos, self.image_rect);

                    let click_packet = Packet::Control {
                        payload: ControlPayload::MouseClick {
                            mouse_x,
                            mouse_y,
                            pressed: *pressed,
                            button: *button,
                        },
                    };
                    channel.send(click_packet).unwrap();
                }

                egui::Event::Key {
                    physical_key,
                    pressed,
                    ..
                } => {
                    if let Some(key) = physical_key {
                        let vk = egui_key_to_vk(key).unwrap();

                        let key_packet = Packet::Control {
                            payload: ControlPayload::Keyboard {
                                pressed: *pressed,
                                key: vk,
                            },
                        };
                        channel.send(key_packet).unwrap();
                    }
                }

                egui::Event::PointerMoved(new_pos) => {
                    let (mouse_x, mouse_y) = normalize_mouse_position(*new_pos, self.image_rect);

                    let mouse_move_packet = Packet::Control {
                        payload: ControlPayload::MouseMove { mouse_x, mouse_y },
                    };
                    channel.send(mouse_move_packet).unwrap();
                }

                egui::Event::MouseWheel { delta, .. } => {
                    let scroll_packet = Packet::Control {
                        payload: ControlPayload::Scroll {
                            delta: delta.y.signum() as i32,
                        },
                    };
                    channel.send(scroll_packet).unwrap();
                }

                _ => (),
            }
        }
    }

    fn central_panel_ui(&mut self, ui: &mut Ui, ctx: &egui::Context, channel: &mut SecureChannel) {
        let texture = egui::ColorImage::from_rgba_unmultiplied([1920, 1080], &self.current_frame);
        let handle = ctx.load_texture("screen", texture, egui::TextureOptions::default());

        let available_size = ui.available_size();

        let scale = {
            let scale_x = available_size.x / 1920.0;
            let scale_y = available_size.y / 1080.0;
            scale_x.min(scale_y)
        };

        let final_size = Vec2::new(1920.0, 1080.0) * scale;

        let available_rect = ui.max_rect();
        let top_left = available_rect.center() - final_size * 0.5;
        let centered_rect = Rect::from_min_size(top_left, final_size);

        // allocate the space exactly at the centered position
        let response = ui.allocate_rect(centered_rect, Sense::click_and_drag());
        self.image_rect = centered_rect;

        ui.painter().image(
            handle.id(),
            self.image_rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );

        // put a border
        let stroke = Stroke::new(1.0, Color32::WHITE);
        ui.painter()
            .rect_stroke(centered_rect, 0.0, stroke, egui::StrokeKind::Outside);

        // make sure the user is the controller before handling input
        if response.hovered() && self.control_msg.lock().unwrap().as_str() == CONTROLLING_MSG {
            ui.ctx().memory_mut(|mem| mem.request_focus(egui::Id::NULL));
            ui.input(|input| self.handle_input(input, channel));
        }
    }

    fn disconnect(&mut self, channel: &mut SecureChannel) -> SceneChange {
        channel.send(Packet::SessionExit).unwrap();

        let _ = self.thread_receive_socket.take().unwrap().join();
        let _ = self.thread_read_decoded.take().unwrap().join();
        let _ = self.ffmpeg_command.kill();

        SceneChange::To(Box::new(MenuScene::new(self.username.clone(), channel, "")))
    }
}

impl Scene for MainScene {
    fn update(&mut self, ctx: &egui::Context, channel: &mut Option<SecureChannel>) -> SceneChange {
        let channel = channel.as_mut().unwrap();
        let mut result: SceneChange = SceneChange::None;

        let now = Instant::now();
        let dt = now.duration_since(self.now).as_secs_f32();
        self.now = now;
        self.elapsed_time += dt;

        if self.elapsed_time > 1. / 30. {
            self.elapsed_time = 0.;
            if let Some(image) = self.frame_queue.lock().unwrap().pop_front() {
                self.current_frame = image;
            }
        }

        if self.stop_flag.load(Ordering::Relaxed) {
            let _ = self.ffmpeg_command.kill();
            let _ = self.thread_receive_socket.take().unwrap().join();
            let _ = self.thread_read_decoded.take().unwrap().join();

            return SceneChange::To(Box::new(MenuScene::new(
                self.username.clone(),
                channel,
                "The host ended the session.",
            )));
        }

        egui::SidePanel::right("participants").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut self.right_panel_type,
                    RightPanelType::UsersList,
                    "User List",
                );
                ui.selectable_value(&mut self.right_panel_type, RightPanelType::Chat, "Chat");
            });

            match self.right_panel_type {
                RightPanelType::UsersList => {
                    ui.heading("Users");
                    ui.separator();

                    users_list(
                        ui,
                        self.usernames.lock().unwrap(),
                        self.username.clone(),
                        false,
                    );
                }

                RightPanelType::Chat => {
                    chat_ui(
                        ui,
                        self.chat_log.lock().unwrap(),
                        &mut self.chat_message,
                        channel,
                    );
                }
            }
        });

        egui::TopBottomPanel::bottom("bottom_panel")
            .resizable(false)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    if ui.button("Disconnect").clicked() {
                        result = self.disconnect(channel);
                    }

                    let mut control_msg = self.control_msg.lock().unwrap();

                    if ui
                        .add_enabled(
                            control_msg.as_str() == REQUEST_CONTROL_MSG,
                            |ui: &mut Ui| ui.button(control_msg.as_str()),
                        )
                        .clicked()
                    {
                        *control_msg = WAITING_CONTROL_MSG.to_string();

                        let request_control = Packet::RequestControl {
                            username: self.username.clone(),
                        };
                        channel.send(request_control).unwrap();
                    }
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.central_panel_ui(ui, ctx, channel);
        });

        result
    }

    fn on_exit(&mut self, channel: &mut Option<SecureChannel>) {
        let channel = channel.as_mut().unwrap();

        self.disconnect(channel);

        channel.send(Packet::SignOut).unwrap();
        channel.send(Packet::Shutdown).unwrap();

        channel.close();
    }
}
