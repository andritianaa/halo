//! Synthetic input (SendInput) — used by Quick Accent to type the chosen character.

use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput,
    VIRTUAL_KEY,
};

fn key(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

/// Press + release a virtual key (e.g. `VK_BACK` = 0x08).
pub fn tap(vk: u16) -> bool {
    let inputs = [key(vk, 0, KEYBD_EVENT_FLAGS(0)), key(vk, 0, KEYEVENTF_KEYUP)];
    unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) == inputs.len() as u32 }
}

/// Type arbitrary text as Unicode keystrokes (handles surrogate pairs).
pub fn type_text(text: &str) -> bool {
    let mut inputs = Vec::new();
    for unit in text.encode_utf16() {
        inputs.push(key(0, unit, KEYEVENTF_UNICODE));
        inputs.push(key(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
    }
    if inputs.is_empty() {
        return true;
    }
    unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) == inputs.len() as u32 }
}
