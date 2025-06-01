use crate::UserType;
use eframe::egui::PointerButton;
use std::collections::VecDeque;

/// Defines a trait for messages that can be converted to and from bytes for network transmission.
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

/// Extracts a `u32` (unsigned 32-bit integer) from the beginning of a `VecDeque<u8>`.
///
/// This function assumes the `u32` is stored in big-endian format. It removes the
/// 4 bytes corresponding to the `u32` from the `VecDeque`.
///
/// # Arguments
///
/// * `bytes` - A mutable reference to a `VecDeque<u8>` containing the byte stream.
///
/// # Returns
///
/// An `Option<u32>` which is:
/// - `Some(value)` if 4 bytes were successfully read and converted to a `u32`.
/// - `None` if there were not enough bytes in the `VecDeque` to form a `u32`.
pub fn get_u32_from_packet(bytes: &mut VecDeque<u8>) -> Option<u32> {
    let data: Vec<u8> = bytes.drain(0..4).collect();
    Some(u32::from_be_bytes(data.try_into().ok()?))
}

/// Extracts an `i32` (signed 32-bit integer) from the beginning of a `VecDeque<u8>`.
///
/// This function assumes the `i32` is stored in big-endian format. It removes the
/// 4 bytes corresponding to the `i32` from the `VecDeque`.
///
/// # Arguments
///
/// * `bytes` - A mutable reference to a `VecDeque<u8>` containing the byte stream.
///
/// # Returns
///
/// An `Option<i32>` which is:
/// - `Some(value)` if 4 bytes were successfully read and converted to an `i32`.
/// - `None` if there were not enough bytes in the `VecDeque` to form an `i32`.
pub fn get_i32_from_packet(bytes: &mut VecDeque<u8>) -> Option<i32> {
    let data: Vec<u8> = bytes.drain(0..4).collect();
    Some(i32::from_be_bytes(data.try_into().ok()?))
}

/// Extracts a `u16` (unsigned 16-bit integer) from the beginning of a `VecDeque<u8>`.
///
/// This function assumes the `u16` is stored in big-endian format. It removes the
/// 2 bytes corresponding to the `u16` from the `VecDeque`.
///
/// # Arguments
///
/// * `bytes` - A mutable reference to a `VecDeque<u8>` containing the byte stream.
///
/// # Returns
///
/// An `Option<u16>` which is:
/// - `Some(value)` if 2 bytes were successfully read and converted to a `u16`.
/// - `None` if there were not enough bytes in the `VecDeque` to form a `u16`.
pub fn get_u16_from_packet(bytes: &mut VecDeque<u8>) -> Option<u16> {
    let data: Vec<u8> = bytes.drain(0..2).collect();
    Some(u16::from_be_bytes(data.try_into().ok()?))
}

/// Reads a `u32` integer from the beginning of a `VecDeque<u8>` (in big-endian format)
/// and then reads that number of subsequent bytes.
///
/// This function is typically used for length-prefixed data, where the first 4 bytes
/// indicate the length of the data that follows.
///
/// # Arguments
///
/// * `bytes` - A mutable reference to a `VecDeque<u8>` containing the byte stream.
///
/// # Returns
///
/// An `Option<Vec<u8>>` which is:
/// - `Some(data)` if a length and the corresponding data were successfully read.
/// - `None` if there were not enough bytes for the length or the data itself.
fn read_length_and_data(bytes: &mut VecDeque<u8>) -> Option<Vec<u8>> {
    let len = get_u32_from_packet(bytes)? as usize;

    Some(bytes.drain(..len).collect())
}

/// Writes the length of a string as a `u32` (big-endian) followed by the string's bytes
/// into a mutable byte vector.
///
/// This function is used to serialize length-prefixed strings for network transmission.
///
/// # Arguments
///
/// * `bytes` - A mutable reference to a `Vec<u8>` where the length and string bytes will be appended.
/// * `string` - A string slice (`&str`) to be written.
fn write_length_and_string(bytes: &mut Vec<u8>, string: &str) {
    bytes.extend_from_slice(&(string.len() as u32).to_be_bytes());
    bytes.extend_from_slice(string.as_bytes());
}

/// Represents the result of an operation, either a success or a failure,
/// both carrying a descriptive message.
pub enum ResultPacket {
    /// Indicates a failed operation with an associated error message.
    Failure(String),
    /// Indicates a successful operation with an associated success message.
    Success(String),
}

impl ProtocolMessage for ResultPacket {
    /// Converts a `ResultPacket` into a byte vector for network transmission.
    ///
    /// The first byte indicates the packet type (0 for Failure, 1 for Success),
    /// followed by a length-prefixed UTF-8 string for the message.
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

    /// Attempts to create a `ResultPacket` from a byte vector.
    ///
    /// The function expects the first byte to indicate the packet type (0 or 1),
    /// followed by a length-prefixed UTF-8 string.
    ///
    /// # Arguments
    ///
    /// * `bytes` - A `Vec<u8>` containing the raw byte data of the packet.
    ///
    /// # Returns
    ///
    /// An `Option<ResultPacket>` which is:
    /// - `Some(ResultPacket)` if the bytes represent a valid `ResultPacket`.
    /// - `None` if the bytes are malformed or the packet type is unknown.
    ///
    /// # Panics
    ///
    /// Panics if the message part of the bytes is not valid UTF-8.
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

/// Represents the various types of packets that can be sent between the client and server.
/// Each variant encapsulates specific data related to its purpose.
#[derive(PartialEq, Default, Clone)]
pub enum Packet {
    /// Default state, representing no specific packet.
    #[default]
    None,

    /// Packet for user login attempts.
    Login { username: String, password: String },

    /// Packet for new user registration.
    Register { username: String, password: String },

    /// Packet indicating a user wants to host a session.
    Host,

    /// Packet for a user attempting to join an existing session.
    Join { code: u32, username: String },

    /// Packet to update information about a user in a session.
    UserUpdate {
        user_type: UserType,
        joined_before: bool,
        username: String,
    },

    /// Packet containing control input (mouse, keyboard, scroll).
    Control { payload: ControlPayload },

    /// Packet containing screen image data.
    Screen { bytes: Vec<u8> },

    /// Packet to signal a user is exiting the current session.
    SessionExit,

    /// Packet for a participant requesting control of the host's screen.
    RequestControl { username: String },

    /// Packet for the host denying a control request from a participant.
    DenyControl { username: String },

    /// Packet for a user signing out.
    SignOut,

    /// Packet to initiate a shutdown.
    Shutdown,

    /// Packet to signal the end of a session.
    SessionEnd,

    /// Packet for sending chat messages.
    Chat { message: String },

    /// Packet to request watching a specific recording by its ID.
    WatchRecording { id: i32 },

    /// Packet containing the name of a recording, identified by its ID.
    RecordingName { id: i32, name: String },

    /// Packet for the host denying a join request from a user.
    DenyJoin { username: String },

    /// Packet to initialize seeking in a recording.
    SeekInit,

    /// Packet to seek to a specific time in a recording.
    SeekTo { time_seconds: i32 },
}

impl ProtocolMessage for Packet {
    /// Turns a `Packet` into bytes that can be sent over a socket.
    ///
    /// Each packet type is prefixed with a unique byte identifier, followed by
    /// its specific data, often length-prefixed strings or fixed-size integers
    /// in big-endian format.
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

            Packet::SeekTo { time_seconds } => {
                result.push(8);

                result.extend_from_slice(&time_seconds.to_be_bytes());
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
        }

        result
    }

    /// Attempts to create a `Packet` from a byte vector.
    ///
    /// The function reads the first byte to determine the packet type and then
    /// parses the subsequent bytes according to the expected structure of that
    /// packet type.
    ///
    /// # Arguments
    ///
    /// * `bytes` - A `Vec<u8>` containing the raw byte data of the packet.
    ///
    /// # Returns
    ///
    /// An `Option<Packet>` which is:
    /// - `Some(Packet)` if the bytes represent a valid `Packet`.
    /// - `None` if the bytes are malformed, incomplete, or the packet type is unknown.
    ///
    /// # Panics
    ///
    /// Panics if any string data within the packet bytes is not valid UTF-8.
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

            // SeekTo
            8 => {
                let time_seconds = get_i32_from_packet(&mut bytes)?;

                Some(Self::SeekTo { time_seconds })
            }

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

            _ => None,
        }
    }
}

/// Represents different types of control inputs that can be sent over the network.
/// These payloads are typically encapsulated within a `Packet::Control` variant.
#[derive(PartialEq, Clone)]
pub enum ControlPayload {
    /// Represents a mouse movement event.
    MouseMove { mouse_x: u32, mouse_y: u32 },

    /// Represents a mouse click event.
    MouseClick {
        mouse_x: u32,
        mouse_y: u32,
        pressed: bool,
        button: PointerButton,
    },

    /// Represents a keyboard event (key press or release).
    Keyboard { pressed: bool, key: u16 },

    /// Represents a scroll wheel event.
    Scroll { delta: i32 },
}

impl ControlPayload {
    /// Turns a `ControlPayload` into bytes that will then be appended to a `Control` packet.
    ///
    /// Each `ControlPayload` type is prefixed with a unique byte identifier, followed by
    /// its specific data.
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
    /// The function reads the first byte to determine the payload type and then
    /// parses the subsequent bytes according to the expected structure of that
    /// payload type.
    ///
    /// # Arguments
    ///
    /// * `bytes` - A `Vec<u8>` containing the raw byte data of the control payload.
    ///
    /// # Returns
    ///
    /// Will return `None` if the bytes are not a valid `ControlPayload` (e.g.,
    /// insufficient bytes for the payload type, or unknown payload type).
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

    /// Returns the expected length in bytes of a specific `ControlPayload` type.
    ///
    /// This is useful for pre-allocating buffers or validating incoming data.
    ///
    /// # Arguments
    ///
    /// * `payload_type` - The byte identifier of the `ControlPayload` type.
    ///
    /// # Returns
    ///
    /// The expected length in bytes for the given payload type. Returns 0 for unknown types.
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
