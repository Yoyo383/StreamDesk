use crate::UserType;
use eframe::egui::PointerButton;
use std::collections::VecDeque;

pub trait ProtocolMessage {
    /// Turns a `ProtocolMessage` into bytes that can be sent over a socket.
    fn to_bytes(&self) -> Vec<u8>;

    /// Turns an array of bytes into a `ProtocolMessage`.
    ///
    /// Returns `None` if the bytes are invalid for this type of `ProtocolMessage`.
    fn from_bytes(bytes: Vec<u8>) -> Option<Self>
    where
        Self: Sized;
}

pub fn get_u32_from_packet(bytes: &mut VecDeque<u8>) -> Option<u32> {
    let data: Vec<u8> = bytes.drain(0..4).collect();
    Some(u32::from_be_bytes(data.try_into().ok()?))
}

pub fn get_i32_from_packet(bytes: &mut VecDeque<u8>) -> Option<i32> {
    let data: Vec<u8> = bytes.drain(0..4).collect();
    Some(i32::from_be_bytes(data.try_into().ok()?))
}

pub fn get_u16_from_packet(bytes: &mut VecDeque<u8>) -> Option<u16> {
    let data: Vec<u8> = bytes.drain(0..2).collect();
    Some(u16::from_be_bytes(data.try_into().ok()?))
}

/// Reads a `u32` integer from the socket (in big endian) and then reads that number of bytes.
fn read_length_and_data(bytes: &mut VecDeque<u8>) -> Option<Vec<u8>> {
    let len = get_u32_from_packet(bytes)? as usize;

    Some(bytes.drain(..len).collect())
}

fn write_length_and_string(bytes: &mut Vec<u8>, string: &str) {
    bytes.extend_from_slice(&(string.len() as u32).to_be_bytes());
    bytes.extend_from_slice(string.as_bytes());
}

pub enum ResultPacket {
    Failure(String),
    Success(String),
}

impl ProtocolMessage for ResultPacket {
    fn to_bytes(&self) -> Vec<u8> {
        match self {
            ResultPacket::Failure(msg) => {
                let mut result = vec![0u8];
                write_length_and_string(&mut result, msg);

                result
            }

            ResultPacket::Success(msg) => {
                let mut result = vec![1u8];
                write_length_and_string(&mut result, msg);

                result
            }
        }
    }

    fn from_bytes(bytes: Vec<u8>) -> Option<Self> {
        let mut bytes = VecDeque::from(bytes);

        let packet_type = bytes.pop_front()?;
        let msg = String::from_utf8(read_length_and_data(&mut bytes)?)
            .expect("bytes should be valid utf8.");

        match packet_type {
            0 => Some(Self::Failure(msg)),
            1 => Some(Self::Success(msg)),
            _ => None,
        }
    }
}

#[derive(PartialEq, Default, Clone)]
pub enum Packet {
    #[default]
    None,

    Login {
        username: String,
        password: String,
    },

    Register {
        username: String,
        password: String,
    },

    Host,

    Join {
        code: u32,
        username: String,
    },

    UserUpdate {
        user_type: UserType,
        joined_before: bool,
        username: String,
    },

    Control {
        payload: ControlPayload,
    },

    Screen {
        bytes: Vec<u8>,
    },

    MergeUnready,

    SessionExit,

    RequestControl {
        username: String,
    },

    DenyControl {
        username: String,
    },

    SignOut,

    Shutdown,

    SessionEnd,

    Chat {
        message: String,
    },

    WatchRecording {
        id: i32,
    },

    RecordingName {
        id: i32,
        name: String,
    },

    DenyJoin {
        username: String,
    },

    SeekInit,

    SeekTo {
        time_seconds: i32,
    },
}

impl ProtocolMessage for Packet {
    /// Turns a `Packet` into bytes that can be sent over a socket.
    fn to_bytes(&self) -> Vec<u8> {
        let mut result: Vec<u8> = vec![];

        match self {
            Packet::None => {
                result.push(0);
            }

            Packet::Login { username, password } => {
                result.push(1);

                write_length_and_string(&mut result, &username);
                write_length_and_string(&mut result, &password);
            }

            Packet::Register { username, password } => {
                result.push(2);

                write_length_and_string(&mut result, &username);
                write_length_and_string(&mut result, &password);
            }

            Packet::Host => {
                result.push(3);
            }

            Packet::Join { code, username } => {
                result.push(4);

                result.extend_from_slice(&code.to_be_bytes());
                write_length_and_string(&mut result, &username);
            }

            Packet::UserUpdate {
                user_type,
                joined_before,
                username,
            } => {
                result.push(5);

                result.push(*user_type as u8);
                result.push(*joined_before as u8);
                write_length_and_string(&mut result, &username);
            }

            Packet::Control { payload } => {
                result.push(6);

                result.extend_from_slice(&payload.to_bytes());
            }

            Packet::Screen { bytes } => {
                result.push(7);

                result.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
                result.extend_from_slice(bytes);
            }

            Packet::MergeUnready => {
                result.push(8);
            }

            Packet::SessionExit => {
                result.push(9);
            }

            Packet::RequestControl { username } => {
                result.push(10);

                write_length_and_string(&mut result, &username);
            }

            Packet::DenyControl { username } => {
                result.push(11);

                write_length_and_string(&mut result, &username);
            }

            Packet::SignOut => {
                result.push(12);
            }

            Packet::Shutdown => {
                result.push(13);
            }

            Packet::SessionEnd => {
                result.push(14);
            }

            Packet::Chat { message } => {
                result.push(15);

                write_length_and_string(&mut result, &message);
            }

            Packet::WatchRecording { id } => {
                result.push(16);

                result.extend_from_slice(&id.to_be_bytes());
            }

            Packet::RecordingName { id, name } => {
                result.push(17);

                result.extend_from_slice(&id.to_be_bytes());
                write_length_and_string(&mut result, &name);
            }

            Packet::DenyJoin { username } => {
                result.push(18);

                write_length_and_string(&mut result, &username);
            }

            Packet::SeekInit => {
                result.push(19);
            }

            Packet::SeekTo { time_seconds } => {
                result.push(20);

                result.extend_from_slice(&time_seconds.to_be_bytes());
            }
        }

        result
    }

    fn from_bytes(bytes: Vec<u8>) -> Option<Self> {
        let mut bytes = VecDeque::from(bytes);
        let packet_type = bytes.pop_front()?;

        match packet_type {
            0 => Some(Self::None),

            // Login/Register
            1 | 2 => {
                let username = String::from_utf8(read_length_and_data(&mut bytes)?)
                    .expect("bytes should be valid utf8.");
                let password = String::from_utf8(read_length_and_data(&mut bytes)?)
                    .expect("bytes should be valid utf8.");

                if packet_type == 1 {
                    Some(Self::Login { username, password })
                } else {
                    Some(Self::Register { username, password })
                }
            }

            // Host
            3 => Some(Self::Host),

            // Join
            4 => {
                let code = get_u32_from_packet(&mut bytes)?;

                let username = String::from_utf8(read_length_and_data(&mut bytes)?)
                    .expect("bytes should be valid utf8");

                Some(Self::Join { code, username })
            }

            // UserUpdate
            5 => {
                let user_type_raw = bytes.pop_front()?;
                let user_type = match user_type_raw {
                    0 => UserType::Leaving,
                    1 => UserType::Host,
                    2 => UserType::Controller,
                    3 => UserType::Participant,
                    _ => return None,
                };

                let joined_before = bytes.pop_front()? != 0;

                let username = String::from_utf8(read_length_and_data(&mut bytes)?)
                    .expect("bytes should be valid utf8.");

                Some(Self::UserUpdate {
                    user_type,
                    joined_before,
                    username,
                })
            }

            // Control
            6 => {
                let payload_type = bytes.pop_front()?;

                let payload_length = ControlPayload::get_length(payload_type);
                let mut payload_raw: Vec<u8> = bytes.drain(..payload_length).collect();
                payload_raw.insert(0, payload_type);

                let payload = ControlPayload::from_bytes(payload_raw)?;

                Some(Self::Control { payload })
            }

            // Screen
            7 => {
                let bytes = read_length_and_data(&mut bytes)?;

                Some(Self::Screen { bytes })
            }

            // MergeUnready
            8 => Some(Self::MergeUnready),

            // SessionExit
            9 => Some(Self::SessionExit),

            // RequestControl
            10 => {
                let username = String::from_utf8(read_length_and_data(&mut bytes)?)
                    .expect("bytes should be valid utf8.");
                Some(Self::RequestControl { username })
            }

            // DenyControl
            11 => {
                let username = String::from_utf8(read_length_and_data(&mut bytes)?)
                    .expect("bytes should be valid utf8.");
                Some(Self::DenyControl { username })
            }

            // SignOut
            12 => Some(Self::SignOut),

            // Shutdown
            13 => Some(Self::Shutdown),

            // SessionEnd
            14 => Some(Self::SessionEnd),

            // Chat
            15 => {
                let message = String::from_utf8(read_length_and_data(&mut bytes)?)
                    .expect("bytes should be valid utf8");
                Some(Self::Chat { message })
            }

            // WatchRecording
            16 => {
                let id = get_i32_from_packet(&mut bytes)?;

                Some(Self::WatchRecording { id })
            }

            // RecordingName
            17 => {
                let id = get_i32_from_packet(&mut bytes)?;
                let name = String::from_utf8(read_length_and_data(&mut bytes)?)
                    .expect("bytes should be valid utf8");

                Some(Self::RecordingName { id, name })
            }

            // DenyJoin
            18 => {
                let username = String::from_utf8(read_length_and_data(&mut bytes)?)
                    .expect("bytes should be valid utf8");

                Some(Self::DenyJoin { username })
            }

            // SeekInit
            19 => Some(Self::SeekInit),

            // SeekTo
            20 => {
                let time_seconds = get_i32_from_packet(&mut bytes)?;

                Some(Self::SeekTo { time_seconds })
            }

            _ => None,
        }
    }
}

#[derive(PartialEq, Clone)]
pub enum ControlPayload {
    MouseMove {
        mouse_x: u32,
        mouse_y: u32,
    },

    MouseClick {
        mouse_x: u32,
        mouse_y: u32,
        pressed: bool,
        button: PointerButton,
    },

    Keyboard {
        pressed: bool,
        key: u16,
    },

    Scroll {
        delta: i32,
    },
}

impl ControlPayload {
    /// Turns a `ControlPayload` into bytes that will then be appended to a `Control` packet.
    fn to_bytes(&self) -> Vec<u8> {
        let mut result: Vec<u8> = vec![];

        match self {
            ControlPayload::MouseMove { mouse_x, mouse_y } => {
                result.push(0);

                result.extend_from_slice(&mouse_x.to_be_bytes());
                result.extend_from_slice(&mouse_y.to_be_bytes());
            }

            ControlPayload::MouseClick {
                mouse_x,
                mouse_y,
                pressed,
                button,
            } => {
                result.push(1);

                result.extend_from_slice(&mouse_x.to_be_bytes());
                result.extend_from_slice(&mouse_y.to_be_bytes());
                result.push(*pressed as u8);
                result.push(*button as u8);
            }

            ControlPayload::Keyboard { pressed, key } => {
                result.push(2);

                result.push(*pressed as u8);
                result.extend_from_slice(&key.to_be_bytes());
            }

            ControlPayload::Scroll { delta } => {
                result.push(3);

                result.extend_from_slice(&delta.to_be_bytes());
            }
        }

        result
    }

    /// Turns a vector of bytes into a `ControlPayload`.
    ///
    /// Will return `None` if the bytes are not a valid `ControlPayload`.
    fn from_bytes(bytes: Vec<u8>) -> Option<Self> {
        let mut bytes = VecDeque::from(bytes);
        let payload_type = bytes.pop_front()?;

        match payload_type {
            // MouseMove
            0 => {
                let mouse_x = get_u32_from_packet(&mut bytes)?;
                let mouse_y = get_u32_from_packet(&mut bytes)?;

                Some(Self::MouseMove { mouse_x, mouse_y })
            }

            // MouseClick
            1 => {
                let mouse_x = get_u32_from_packet(&mut bytes)?;
                let mouse_y = get_u32_from_packet(&mut bytes)?;
                let pressed = bytes.pop_front()? != 0;
                let raw_button = bytes.pop_front()?;
                let button = match raw_button {
                    0 => PointerButton::Primary,
                    1 => PointerButton::Secondary,
                    2 => PointerButton::Middle,
                    _ => return None,
                };

                Some(Self::MouseClick {
                    mouse_x,
                    mouse_y,
                    pressed,
                    button,
                })
            }

            // Keyboard
            2 => {
                let pressed = bytes.pop_front()? != 0;
                let key = get_u16_from_packet(&mut bytes)?;

                Some(Self::Keyboard { pressed, key })
            }

            // Scroll
            3 => {
                let delta = get_i32_from_packet(&mut bytes)?;

                Some(Self::Scroll { delta })
            }

            _ => None,
        }
    }

    /// Returns the length in bytes of a payload type.
    fn get_length(payload_type: u8) -> usize {
        match payload_type {
            0 => 8,  // MouseMove
            1 => 10, // MouseClick
            2 => 3,  // Keyboard
            3 => 4,  // Scroll
            _ => 0,
        }
    }
}
