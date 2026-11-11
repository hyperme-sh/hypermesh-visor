//! Explicit refusal backend for builds that do not include desktop support.
use super::*;

pub struct HeadlessDesktop;

fn unavailable<T>() -> Result<T, DesktopError> {
    Err(DesktopError::Unavailable(
        "desktop operations are unavailable in a headless build".into(),
    ))
}

impl Desktop for HeadlessDesktop {
    fn backend_name(&self) -> &'static str {
        "headless"
    }
    fn view(&self) -> Result<Frame, DesktopError> {
        unavailable()
    }
    fn open_audio(&self) -> Result<Box<dyn AudioRead>, DesktopError> {
        unavailable()
    }
    fn mouse(&self, _: &MouseOp) -> Result<(), DesktopError> {
        unavailable()
    }
    fn type_input(&self, _: &[Stroke]) -> Result<(), DesktopError> {
        unavailable()
    }
    fn pointer_steps(&self, _: &[PointerStep]) -> Result<(), DesktopError> {
        unavailable()
    }
    fn open_pointer(&self) -> Result<Box<dyn PointerPipe + '_>, DesktopError> {
        unavailable()
    }
    fn open_keyboard(&self) -> Result<Box<dyn KeyboardPipe + '_>, DesktopError> {
        unavailable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headless_never_opens_desktop_devices() {
        let backend = HeadlessDesktop;
        assert_eq!(backend.backend_name(), "headless");
        assert!(backend.view().is_err());
        assert!(backend.open_audio().is_err());
        assert!(backend.open_pointer().is_err());
        assert!(backend.open_keyboard().is_err());
        assert!(backend.type_input(&[]).is_err());
        assert!(backend.pointer_steps(&[]).is_err());
    }
}
