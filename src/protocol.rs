use std::{
    io::{Read, Write},
    net::TcpStream,
};

use eframe::egui::PointerButton;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    None,
    MouseClick,
    MouseMove,
    Scroll,
    Keyboard,
    Screen,
    Shutdown,
}

#[derive(Debug)]
#[repr(C)]
pub struct Message {
    pub message_type: MessageType,
    pub mouse_button: PointerButton,
    pub mouse_position: (i32, i32),
    pub key: u16,
    pub pressed: bool,
    pub screen_data: Vec<u8>,
}

impl Default for Message {
    fn default() -> Self {
        Self {
            message_type: MessageType::None,
            mouse_button: PointerButton::Primary,
            mouse_position: Default::default(),
            key: Default::default(),
            pressed: Default::default(),
            screen_data: Default::default(),
        }
    }
}

impl Message {
    pub fn new_mouse_click(
        mouse_button: PointerButton,
        mouse_position: (i32, i32),
        pressed: bool,
    ) -> Self {
        Self {
            message_type: MessageType::MouseClick,
            mouse_button,
            mouse_position,
            pressed,
            ..Default::default()
        }
    }

    pub fn new_mouse_move(mouse_position: (i32, i32)) -> Self {
        Self {
            message_type: MessageType::MouseMove,
            mouse_position,
            ..Default::default()
        }
    }

    pub fn new_scroll(delta: f32) -> Self {
        Self {
            message_type: MessageType::Scroll,
            mouse_position: (0, delta as i32),
            ..Default::default()
        }
    }

    pub fn new_keyboard(key: u16, pressed: bool) -> Self {
        Self {
            message_type: MessageType::Keyboard,
            key,
            pressed,
            ..Default::default()
        }
    }

    pub fn new_screen(screen_data: Vec<u8>) -> Self {
        Self {
            message_type: MessageType::Screen,
            screen_data,
            ..Default::default()
        }
    }

    pub fn new_shutdown() -> Self {
        Self {
            message_type: MessageType::Shutdown,
            ..Default::default()
        }
    }

    pub fn send(&self, socket: &mut TcpStream) -> std::io::Result<()> {
        let bytes = self.to_bytes();

        // len (8 bytes) and then the message struct
        let len = bytes.len() as u64;
        socket.write_all(&len.to_be_bytes())?;
        socket.write_all(&bytes)?;

        Ok(())
    }

    pub fn receive(socket: &mut TcpStream) -> Option<Self> {
        let mut len_buffer = [0u8; 8];
        socket.read_exact(&mut len_buffer).ok()?;
        let len = u64::from_be_bytes(len_buffer) as usize;

        let mut bytes = vec![0u8; len];
        socket.read_exact(&mut bytes).ok()?;

        let message = Message::from_bytes(bytes)?;
        Some(message)
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes: Vec<u8> = vec![];
        bytes.push(self.message_type as u8);
        bytes.push(self.mouse_button as u8);
        bytes.extend_from_slice(&self.mouse_position.0.to_be_bytes());
        bytes.extend_from_slice(&self.mouse_position.1.to_be_bytes());
        bytes.extend_from_slice(&self.key.to_be_bytes());
        bytes.push(self.pressed as u8);
        bytes.extend_from_slice(&self.screen_data);
        bytes
    }

    fn from_bytes(bytes: Vec<u8>) -> Option<Self> {
        let message_type = match bytes[0] {
            0 => Some(MessageType::None),
            1 => Some(MessageType::MouseClick),
            2 => Some(MessageType::MouseMove),
            3 => Some(MessageType::Scroll),
            4 => Some(MessageType::Keyboard),
            5 => Some(MessageType::Screen),
            6 => Some(MessageType::Shutdown),
            _ => None,
        }?;
        let mouse_button = match bytes[1] {
            0 => Some(PointerButton::Primary),
            1 => Some(PointerButton::Secondary),
            2 => Some(PointerButton::Middle),
            _ => None,
        }?;
        let mouse_position = (
            i32::from_be_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]),
            i32::from_be_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]),
        );
        let key = u16::from_be_bytes([bytes[10], bytes[11]]);
        let pressed = bytes[12] != 0;

        let screen_data = &bytes[13..];
        Some(Self {
            message_type,
            mouse_button,
            mouse_position,
            key,
            pressed,
            screen_data: screen_data.to_vec(),
        })
    }
}
