use eframe::egui::InputState;
use winapi::um::winuser::{VK_CONTROL, VK_MENU, VK_SHIFT};

#[derive(PartialEq, Eq)]
enum ModifierKeyState {
    None,
    Pressed,
    Release,
}

pub struct ModifierKey {
    pub key: u16,
    pub pressed: bool,
}

pub struct ModifiersState {
    command: ModifierKeyState,
    alt: ModifierKeyState,
    shift: ModifierKeyState,
    pub keys: Vec<ModifierKey>,
}

impl ModifiersState {
    pub fn new() -> Self {
        Self {
            command: ModifierKeyState::None,
            alt: ModifierKeyState::None,
            shift: ModifierKeyState::None,
            keys: Vec::new(),
        }
    }

    pub fn update(&mut self, input: &InputState) {
        self.keys.clear();

        if input.modifiers.command {
            if self.command != ModifierKeyState::Pressed {
                self.command = ModifierKeyState::Pressed;
                self.keys.push(ModifierKey {
                    key: VK_CONTROL as u16,
                    pressed: true,
                });
            }
        } else {
            if self.command == ModifierKeyState::Pressed {
                self.command = ModifierKeyState::Release;
                self.keys.push(ModifierKey {
                    key: VK_CONTROL as u16,
                    pressed: false,
                });
            } else {
                self.command = ModifierKeyState::None;
            }
        }

        if input.modifiers.alt {
            if self.alt != ModifierKeyState::Pressed {
                self.alt = ModifierKeyState::Pressed;
                self.keys.push(ModifierKey {
                    key: VK_MENU as u16,
                    pressed: true,
                });
            }
        } else {
            if self.alt == ModifierKeyState::Pressed {
                self.alt = ModifierKeyState::Release;
                self.keys.push(ModifierKey {
                    key: VK_MENU as u16,
                    pressed: false,
                });
            } else {
                self.alt = ModifierKeyState::None;
            }
        }

        if input.modifiers.shift {
            if self.shift != ModifierKeyState::Pressed {
                self.shift = ModifierKeyState::Pressed;
                self.keys.push(ModifierKey {
                    key: VK_SHIFT as u16,
                    pressed: true,
                });
            }
        } else {
            if self.shift == ModifierKeyState::Pressed {
                self.shift = ModifierKeyState::Release;
                self.keys.push(ModifierKey {
                    key: VK_SHIFT as u16,
                    pressed: false,
                });
            } else {
                self.shift = ModifierKeyState::None;
            }
        }
    }
}
