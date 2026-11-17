//! Opt-in macOS Instruments intervals. No payloads, per-frame logs or heap scans.
#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn canban_span_begin(name: *const std::ffi::c_char) -> u64;
    fn canban_span_end(id: u64, name: *const std::ffi::c_char, items: u64, bytes: u64);
}

pub struct Span {
    #[cfg(target_os = "macos")]
    native: Option<(u64, std::ffi::CString)>,
    items: u64,
    bytes: u64,
}
impl Span {
    pub fn new(name: &'static str) -> Self {
        #[cfg(target_os = "macos")]
        let native = {
            static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            if *ENABLED.get_or_init(|| std::env::var("CANBAN_PROFILE").as_deref() == Ok("1")) {
                let name = std::ffi::CString::new(name).expect("static profiling label");
                Some((unsafe { canban_span_begin(name.as_ptr()) }, name))
            } else {
                None
            }
        };
        #[cfg(not(target_os = "macos"))]
        let _ = name;
        Self {
            #[cfg(target_os = "macos")]
            native,
            items: 0,
            bytes: 0,
        }
    }
    pub fn counts(&mut self, items: usize, bytes: usize) {
        self.items = items as u64;
        self.bytes = bytes as u64;
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some((id, name)) = &self.native {
            unsafe { canban_span_end(*id, name.as_ptr(), self.items, self.bytes) };
        }
    }
}
