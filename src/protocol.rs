use eframe::egui::PointerButton;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    Mouse = 0,
    Keyboard = 1,
    Shutdown = 2,
}

#[derive(Debug)]
#[repr(C)]
pub struct Message {
    pub message_type: MessageType,
    pub mouse_button: PointerButton,
    pub mouse_position: (i32, i32),
    pub key: u16,
    pub pressed: bool,
}

impl Message {
    pub fn new(
        message_type: MessageType,
        mouse_button: PointerButton,
        mouse_position: (i32, i32),
        key: u16,
        pressed: bool,
    ) -> Self {
        Self {
            message_type,
            mouse_button,
            mouse_position,
            key,
            pressed,
        }
    }

    pub fn size() -> usize {
        13
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes: Vec<u8> = vec![];
        bytes.push(self.message_type as u8);
        bytes.push(self.mouse_button as u8);
        bytes.extend_from_slice(&self.mouse_position.0.to_be_bytes());
        bytes.extend_from_slice(&self.mouse_position.1.to_be_bytes());
        bytes.extend_from_slice(&self.key.to_be_bytes());
        bytes.push(self.pressed as u8);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Message::size() {
            return None;
        }
        let message_type = match bytes[0] {
            0 => Some(MessageType::Mouse),
            1 => Some(MessageType::Keyboard),
            2 => Some(MessageType::Shutdown),
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
        Some(Self {
            message_type,
            mouse_button,
            mouse_position,
            key,
            pressed,
        })
    }
}
