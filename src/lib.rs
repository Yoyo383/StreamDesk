use std::net::TcpStream;

use eframe::egui::{self, Key, Pos2};

pub mod protocol;

pub fn egui_key_to_vk(key: &Key) -> Option<u16> {
    use winapi::um::winuser::*;
    use Key::*;
    Some(match key {
        ArrowDown => VK_DOWN as u16,
        ArrowLeft => VK_LEFT as u16,
        ArrowRight => VK_RIGHT as u16,
        ArrowUp => VK_UP as u16,
        Escape => VK_ESCAPE as u16,
        Tab => VK_TAB as u16,
        Backspace => VK_BACK as u16,
        Enter => VK_RETURN as u16,
        Space => VK_SPACE as u16,
        Insert => VK_INSERT as u16,
        Delete => VK_DELETE as u16,
        Home => VK_HOME as u16,
        End => VK_END as u16,
        PageUp => VK_PRIOR as u16,
        PageDown => VK_NEXT as u16,
        A => 0x41,
        B => 0x42,
        C => 0x43,
        D => 0x44,
        E => 0x45,
        F => 0x46,
        G => 0x47,
        H => 0x48,
        I => 0x49,
        J => 0x4A,
        K => 0x4B,
        L => 0x4C,
        M => 0x4D,
        N => 0x4E,
        O => 0x4F,
        P => 0x50,
        Q => 0x51,
        R => 0x52,
        S => 0x53,
        T => 0x54,
        U => 0x55,
        V => 0x56,
        W => 0x57,
        X => 0x58,
        Y => 0x59,
        Z => 0x5A,
        Num0 => 0x30,
        Num1 => 0x31,
        Num2 => 0x32,
        Num3 => 0x33,
        Num4 => 0x34,
        Num5 => 0x35,
        Num6 => 0x36,
        Num7 => 0x37,
        Num8 => 0x38,
        Num9 => 0x39,
        F1 => VK_F1 as u16,
        F2 => VK_F2 as u16,
        F3 => VK_F3 as u16,
        F4 => VK_F4 as u16,
        F5 => VK_F5 as u16,
        F6 => VK_F6 as u16,
        F7 => VK_F7 as u16,
        F8 => VK_F8 as u16,
        F9 => VK_F9 as u16,
        F10 => VK_F10 as u16,
        F11 => VK_F11 as u16,
        F12 => VK_F12 as u16,
        F13 => VK_F13 as u16,
        F14 => VK_F14 as u16,
        F15 => VK_F15 as u16,
        F16 => VK_F16 as u16,
        F17 => VK_F17 as u16,
        F18 => VK_F18 as u16,
        F19 => VK_F19 as u16,
        F20 => VK_F20 as u16,
        Minus => VK_OEM_MINUS as u16,
        Plus => VK_OEM_PLUS as u16,
        Equals => VK_OEM_PLUS as u16, // Same as Plus in Windows
        Comma => VK_OEM_COMMA as u16,
        Period => VK_OEM_PERIOD as u16,
        Slash => VK_OEM_2 as u16, // Forward slash
        Backslash => VK_OEM_5 as u16,
        Colon => VK_OEM_1 as u16, // Actually semicolon on US layout
        Semicolon => VK_OEM_1 as u16,
        Quote => VK_OEM_7 as u16,
        OpenBracket => VK_OEM_4 as u16,
        CloseBracket => VK_OEM_6 as u16,
        Backtick => VK_OEM_3 as u16,
        _ => return None,
    })
}

pub fn normalize_mouse_position(mouse_position: Pos2, width: f32, height: f32) -> (i32, i32) {
    let x = (mouse_position.x / width) * 65535.0;
    let y = (mouse_position.y / height) * 65535.0;
    (x as i32, y as i32)
}

pub struct AppData {
    pub socket: Option<TcpStream>,
    pub width: f32,
    pub height: f32,
}

pub enum SceneChange {
    To(Box<dyn Scene>),
    Quit,
}

pub trait Scene {
    fn update(&mut self, ctx: &egui::Context, app_data: &mut AppData) -> Option<SceneChange>;
    fn on_exit(&mut self, app_data: &mut AppData);
}
