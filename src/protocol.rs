use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::TcpStream,
};

use eframe::egui::PointerButton;

use crate::UserType;

pub fn get_u32_from_packet(bytes: &mut VecDeque<u8>) -> Option<u32> {
    let data: Vec<u8> = bytes.drain(0..4).collect();
    Some(u32::from_be_bytes(data.try_into().ok()?))
}

pub fn get_i32_from_packet(bytes: &mut VecDeque<u8>) -> Option<i32> {
    let data: Vec<u8> = bytes.drain(0..4).collect();
    Some(i32::from_be_bytes(data.try_into().ok()?))
}

pub fn get_u16_from_packet(bytes: &mut VecDeque<u8>) -> Option<u16> {
    let data: Vec<u8> = bytes.drain(0..4).collect();
    Some(u16::from_be_bytes(data.try_into().ok()?))
}

/// Reads a `u32` integer from the socket (in big endian) and then reads that number of bytes.
fn read_length_and_data(socket: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    socket.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf);

    let mut data = vec![0u8; len as usize];
    socket.read_exact(&mut data)?;

    Ok(data)
}

fn write_length_and_string(bytes: &mut Vec<u8>, string: &str) {
    bytes.extend_from_slice(&(string.len() as u32).to_be_bytes());
    bytes.extend_from_slice(string.as_bytes());
}

pub enum ResultPacket {
    Failure(String),
    Success(String),
}

impl ResultPacket {
    /// Turns a `ResultPacket` into bytes that can be sent over a socket.
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

    /// Sends the `Packet` through the socket.
    pub fn send(&self, socket: &mut TcpStream) -> std::io::Result<()> {
        let bytes = self.to_bytes();
        socket.write_all(&bytes)
    }

    /// Reads a `Packet` from the socket.
    pub fn receive(socket: &mut TcpStream) -> std::io::Result<Self> {
        let mut packet_type_buf = [0u8; 1];
        socket.read_exact(&mut packet_type_buf)?;
        let packet_type = packet_type_buf[0];

        let msg =
            String::from_utf8(read_length_and_data(socket)?).expect("bytes should be valid utf8.");

        match packet_type {
            0 => Ok(Self::Failure(msg)),
            1 => Ok(Self::Success(msg)),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "result type is invalid",
            )),
        }
    }
}

#[derive(PartialEq, Default)]
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
    },

    UserUpdate {
        user_type: UserType,
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
}

impl Packet {
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

            Packet::Join { code } => {
                result.push(4);

                result.extend_from_slice(&code.to_be_bytes());
            }

            Packet::UserUpdate {
                user_type,
                username,
            } => {
                result.push(5);

                result.push(*user_type as u8);
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
        }

        result
    }

    /// Sends the `Packet` through the socket.
    pub fn send(&self, socket: &mut TcpStream) -> std::io::Result<()> {
        let bytes = self.to_bytes();
        socket.write_all(&bytes)
    }

    /// Reads a `Packet` from the socket.
    pub fn receive(socket: &mut TcpStream) -> std::io::Result<Self> {
        let mut packet_type_buf = [0u8; 1];
        socket.read_exact(&mut packet_type_buf)?;
        let packet_type = packet_type_buf[0];

        match packet_type {
            0 => Ok(Self::None),

            // Login/Register
            1 | 2 => {
                let username = String::from_utf8(read_length_and_data(socket)?)
                    .expect("bytes should be valid utf8.");
                let password = String::from_utf8(read_length_and_data(socket)?)
                    .expect("bytes should be valid utf8.");

                if packet_type == 1 {
                    Ok(Self::Login { username, password })
                } else {
                    Ok(Self::Register { username, password })
                }
            }

            // Host
            3 => Ok(Self::Host),

            // Join
            4 => {
                let mut code_buf = [0u8; 4];
                socket.read_exact(&mut code_buf)?;
                let code = u32::from_be_bytes(code_buf);

                Ok(Self::Join { code })
            }

            // UserUpdate
            5 => {
                let mut user_type_buf = [0u8; 1];
                socket.read_exact(&mut user_type_buf)?;
                let user_type_raw = user_type_buf[0];
                let user_type = match user_type_raw {
                    0 => UserType::Leaving,
                    1 => UserType::Host,
                    2 => UserType::Controller,
                    3 => UserType::Participant,
                    _ => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            "user type is incorrect",
                        ))
                    }
                };

                let username = String::from_utf8(read_length_and_data(socket)?)
                    .expect("bytes should be valid utf8.");

                Ok(Self::UserUpdate {
                    user_type,
                    username,
                })
            }

            // Control
            6 => {
                let mut payload_type_buf = [0u8; 1];
                socket.read_exact(&mut payload_type_buf)?;
                let payload_type = payload_type_buf[0];

                let payload_length = ControlPayload::get_length(payload_type);
                let mut payload_buf = vec![0u8; payload_length];
                socket.read_exact(&mut payload_buf)?;
                payload_buf.insert(0, payload_type);

                let payload = ControlPayload::from_bytes(payload_buf).ok_or(
                    std::io::Error::new(std::io::ErrorKind::Other, "control payload is invalid"),
                )?;

                Ok(Self::Control { payload })
            }

            // Screen
            7 => {
                let bytes = read_length_and_data(socket)?;

                Ok(Self::Screen { bytes })
            }

            // MergeUnready
            8 => Ok(Self::MergeUnready),

            // SessionExit
            9 => Ok(Self::SessionExit),

            // RequestControl
            10 => {
                let username = String::from_utf8(read_length_and_data(socket)?)
                    .expect("bytes should be valid utf8.");
                Ok(Self::RequestControl { username })
            }

            // DenyControl
            11 => {
                let username = String::from_utf8(read_length_and_data(socket)?)
                    .expect("bytes should be valid utf8.");
                Ok(Self::DenyControl { username })
            }

            // SignOut
            12 => Ok(Self::SignOut),

            // Shutdown
            13 => Ok(Self::Shutdown),

            14 => Ok(Self::SessionEnd),

            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "packet type is invalid",
            )),
        }
    }
}

#[derive(PartialEq)]
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
