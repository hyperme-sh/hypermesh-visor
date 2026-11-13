//! Native desktop callback bridge owned by the visible app.
use super::*;
use crate::input::{Button, MouseOp, PointerStep, Stroke};
use base64::Engine;
use serde_json::{json, Value};
use std::sync::{Condvar, Mutex};

/// Trusted in-process ABI supplied by the native app. No socket, path, argument,
/// environment variable or external process can register this callback.
pub(crate) type NativeCall =
    unsafe extern "C" fn(*const u8, usize, *mut *mut u8, *mut usize) -> i32;
#[derive(Default)]
pub struct NativeDesktop {
    callback: Option<NativeCall>,
    sequence: Sequence,
}
impl NativeDesktop {
    pub(crate) fn embedded(callback: NativeCall) -> Self {
        Self {
            callback: Some(callback),
            sequence: Sequence::default(),
        }
    }
    fn bridge(&self) -> Result<Bridge<'_>, DesktopError> {
        let callback = self.callback.ok_or_else(|| unavailable("not embedded"))?;
        let guard = self.sequence.acquire()?;
        Ok(Bridge {
            callback,
            _guard: guard,
        })
    }
}
#[derive(Default)]
struct Sequence {
    held: Mutex<bool>,
    ready: Condvar,
}
impl Sequence {
    fn acquire(&self) -> Result<Permit<'_>, DesktopError> {
        let held = self.held.lock().map_err(unavailable)?;
        let mut held = self
            .ready
            .wait_while(held, |held| *held)
            .map_err(unavailable)?;
        *held = true;
        Ok(Permit(self))
    }
}
struct Permit<'a>(&'a Sequence);
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        if let Ok(mut held) = self.0.held.lock() {
            *held = false;
            self.0.ready.notify_one();
        }
    }
}
struct Bridge<'a> {
    callback: NativeCall,
    _guard: Permit<'a>,
}
fn unavailable(_: impl std::fmt::Display) -> DesktopError {
    DesktopError::Unavailable(
        "Desktop requires the native app and its visible automation approval".into(),
    )
}
impl Bridge<'_> {
    fn call(&mut self, request: Value) -> Result<Value, DesktopError> {
        let request = serde_json::to_vec(&request).map_err(unavailable)?;
        let mut output = std::ptr::null_mut();
        let mut length = 0usize;
        let result =
            unsafe { (self.callback)(request.as_ptr(), request.len(), &mut output, &mut length) };
        if output.is_null() {
            return Err(unavailable("empty response"));
        }
        let response = if result == 0 && length <= 48 * 1024 * 1024 {
            serde_json::from_slice::<Value>(unsafe { std::slice::from_raw_parts(output, length) })
                .map_err(unavailable)
        } else {
            Err(unavailable("native response refused"))
        };
        unsafe {
            libc::free(output.cast());
        }
        let response = response?;
        response
            .get("result")
            .cloned()
            .ok_or_else(|| unavailable("operation denied"))
    }
}
impl Drop for Bridge<'_> {
    fn drop(&mut self) {
        let _ = self.call(json!({"action":"release"}));
    }
}
fn button(button: Button) -> u8 {
    match button {
        Button::Left => 0,
        Button::Right => 1,
        Button::Middle => 2,
    }
}
impl PointerPipe for Bridge<'_> {
    fn send(&mut self, steps: &[PointerStep]) -> Result<(), DesktopError> {
        let steps: Vec<Value> = steps
            .iter()
            .map(|step| match step {
                PointerStep::Move { x, y } => json!({"kind":"move","x":x,"y":y}),
                PointerStep::Scroll { direction } => json!({"kind":"scroll","direction":direction}),
                PointerStep::Button { button: b, down } => {
                    json!({"kind":"button","button":button(*b),"down":down})
                }
            })
            .collect();
        self.call(json!({"action":"pointer","steps":steps}))
            .map(|_| ())
    }
}
impl KeyboardPipe for Bridge<'_> {
    fn send(&mut self, strokes: &[Stroke]) -> Result<(), DesktopError> {
        let strokes: Vec<Value> = strokes
            .iter()
            .map(|s| json!({"keysym":s.keysym,"down":s.down}))
            .collect();
        self.call(json!({"action":"keyboard","strokes":strokes}))
            .map(|_| ())
    }
}
impl Desktop for NativeDesktop {
    fn view(&self) -> Result<Frame, DesktopError> {
        let value = self.bridge()?.call(json!({"action":"view"}))?;
        let png = value["png"].as_str().ok_or_else(|| unavailable("image"))?;
        Ok(Frame::png(
            base64::engine::general_purpose::STANDARD
                .decode(png)
                .map_err(unavailable)?,
        ))
    }
    fn open_audio(&self) -> Result<Box<dyn AudioRead>, DesktopError> {
        Err(DesktopError::AudioUnavailable(
            "Native audio capture is not yet available".into(),
        ))
    }
    fn mouse(&self, op: &MouseOp) -> Result<(), DesktopError> {
        use PointerStep::*;
        let steps = match *op {
            MouseOp::Move { x, y, .. } => vec![Move { x, y }],
            MouseOp::Click { x, y, button } => vec![
                Move { x, y },
                Button { button, down: true },
                Button {
                    button,
                    down: false,
                },
            ],
            MouseOp::Drag {
                x,
                y,
                to_x,
                to_y,
                button,
            } => vec![
                Move { x, y },
                Button { button, down: true },
                Move { x: to_x, y: to_y },
                Button {
                    button,
                    down: false,
                },
            ],
            MouseOp::Scroll {
                x,
                y,
                delta_x,
                delta_y,
            } => crate::input::scroll_steps(x, y, delta_x, delta_y)
                .ok_or_else(|| unavailable("scroll"))?,
        };
        PointerPipe::send(&mut self.bridge()?, &steps)
    }
    fn type_input(&self, strokes: &[Stroke]) -> Result<(), DesktopError> {
        KeyboardPipe::send(&mut self.bridge()?, strokes)
    }
    fn pointer_steps(&self, steps: &[PointerStep]) -> Result<(), DesktopError> {
        PointerPipe::send(&mut self.bridge()?, steps)
    }
    fn open_pointer(&self) -> Result<Box<dyn PointerPipe + '_>, DesktopError> {
        Ok(Box::new(self.bridge()?))
    }
    fn open_keyboard(&self) -> Result<Box<dyn KeyboardPipe + '_>, DesktopError> {
        Ok(Box::new(self.bridge()?))
    }
    fn root_size(&self) -> Result<Option<(u16, u16)>, DesktopError> {
        let info = self.bridge()?.call(json!({"action":"info"}))?;
        let dimension = |name| {
            info[name]
                .as_u64()
                .and_then(|n| u16::try_from(n).ok())
                .filter(|n| *n > 0)
                .ok_or_else(|| unavailable("size"))
        };
        Ok(Some((dimension("width")?, dimension("height")?)))
    }
    fn pointer_position(&self) -> Result<Option<(i32, i32)>, DesktopError> {
        let info = self.bridge()?.call(json!({"action":"info"}))?;
        let position = |name| {
            info[name]
                .as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| unavailable("point"))
        };
        Ok(Some((position("x")?, position("y")?)))
    }
    fn focused_app(&self) -> Result<Option<crate::mcp::FocusedApp>, DesktopError> {
        let info = self.bridge()?.call(json!({"action":"info"}))?;
        let executable = info["pid"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .and_then(super::focus::process_executable);
        if executable.is_none() {
            return Ok(None);
        }
        Ok(Some(crate::mcp::FocusedApp {
            wm_class: None,
            app_id: info["app_id"].as_str().map(str::to_string),
            executable,
        }))
    }
    fn backend_name(&self) -> &'static str {
        match (cfg!(windows), self.callback.is_some()) {
            (true, true) => "windows-embedded",
            (true, false) => "windows",
            (false, true) => "macos-embedded",
            (false, false) => "macos",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standalone_backend_cannot_acquire_native_permissions() {
        let desktop = NativeDesktop::default();
        assert!(desktop.view().is_err());
        assert!(desktop.open_keyboard().is_err());
        assert!(desktop.open_pointer().is_err());
        assert!(desktop.root_size().is_err());
        assert_eq!(desktop.backend_name(), "macos");
    }

    #[test]
    fn native_sequence_releases_its_permit_after_refusal() {
        unsafe extern "C" fn refuse(_: *const u8, _: usize, _: *mut *mut u8, _: *mut usize) -> i32 {
            1
        }
        let desktop = NativeDesktop::embedded(refuse);
        assert!(desktop.view().is_err());
        assert!(!*desktop.sequence.held.lock().unwrap());
        let keyboard = desktop.open_keyboard().unwrap();
        assert!(*desktop.sequence.held.lock().unwrap());
        drop(keyboard);
        assert!(!*desktop.sequence.held.lock().unwrap());
    }
}
