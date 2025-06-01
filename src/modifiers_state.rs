use eframe::egui::InputState;
use winapi::um::winuser::{VK_CONTROL, VK_MENU, VK_SHIFT};

/// Represents the state of a modifier key (e.g., Ctrl, Alt, Shift).
///
/// This enum tracks whether a modifier key is currently not pressed,
/// has just been pressed, or has just been released.
#[derive(PartialEq, Eq)]
enum ModifierKeyState {
    /// The modifier key is not currently pressed and has not just changed state.
    None,
    /// The modifier key has just been pressed down.
    Pressed,
    /// The modifier key has just been released.
    Release,
}

/// Represents a single modifier key event, indicating its virtual key code and press state.
pub struct ModifierKey {
    /// The Windows virtual key code for the modifier (e.g., `VK_CONTROL`, `VK_MENU`, `VK_SHIFT`).
    pub key: u16,
    /// A boolean indicating `true` if the key was pressed, `false` if released.
    pub pressed: bool,
}

/// Manages the state of common modifier keys (Ctrl, Alt, Shift) and detects changes.
///
/// This struct is crucial for tracking which modifier keys are active
/// and generating events when their states change, enabling accurate
/// remote input simulation.
pub struct ModifiersState {
    /// Internal state for the Control (Command) key.
    command: ModifierKeyState,
    /// Internal state for the Alt (Menu) key.
    alt: ModifierKeyState,
    /// Internal state for the Shift key.
    shift: ModifierKeyState,
    /// A vector of `ModifierKey` events generated during the last update,
    /// indicating which modifiers changed state.
    pub keys: Vec<ModifierKey>,
}

impl ModifiersState {
    /// Creates a new `ModifiersState` with all modifier keys initialized to `None`.
    ///
    /// # Returns
    ///
    /// A new `ModifiersState` instance.
    pub fn new() -> Self {
        Self {
            command: ModifierKeyState::None,
            alt: ModifierKeyState::None,
            shift: ModifierKeyState::None,
            keys: Vec::new(),
        }
    }

    /// Updates the state of modifier keys based on the current `egui::InputState`.
    ///
    /// This method should be called once per frame. It detects changes in the
    /// Control, Alt, and Shift key states and populates the `self.keys` vector
    /// with `ModifierKey` events for any keys whose state has changed.
    ///
    /// # Arguments
    ///
    /// * `input` - A reference to the `egui::InputState` which contains the current
    ///             status of keyboard modifiers.
    pub fn update(&mut self, input: &InputState) {
        self.keys.clear();

        // Handle Control (Command) key state
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

        // Handle Alt (Menu) key state
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

        // Handle Shift key state
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
