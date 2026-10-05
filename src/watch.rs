//! inotify mínimo: avisa cuando cambia algo en las carpetas vigiladas, sin sondear.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub struct Watcher {
    fd: i32,
}

impl Watcher {
    pub fn new() -> Option<Self> {
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
        (fd >= 0).then_some(Watcher { fd })
    }

    pub fn add(&self, dir: &Path) -> bool {
        let Ok(c) = CString::new(dir.as_os_str().as_bytes()) else {
            return false;
        };
        let mask = libc::IN_CLOSE_WRITE
            | libc::IN_MOVED_TO
            | libc::IN_MOVED_FROM
            | libc::IN_CREATE
            | libc::IN_DELETE;
        unsafe { libc::inotify_add_watch(self.fd, c.as_ptr(), mask) >= 0 }
    }

    /// Bloquea hasta el siguiente lote de eventos y devuelve los nombres de archivo.
    pub fn wait(&self) -> Vec<String> {
        let mut buf = [0u8; 8192];
        let n = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            return Vec::new();
        }
        let mut names = Vec::new();
        let mut off = 0usize;
        let header = std::mem::size_of::<libc::inotify_event>();
        while off + header <= n as usize {
            // SAFETY: el kernel escribe eventos completos y alineados en el buffer.
            let ev = unsafe { std::ptr::read_unaligned(buf.as_ptr().add(off).cast::<libc::inotify_event>()) };
            let name_bytes = &buf[off + header..off + header + ev.len as usize];
            let end = name_bytes.iter().position(|&b| b == 0).unwrap_or(name_bytes.len());
            names.push(String::from_utf8_lossy(&name_bytes[..end]).into_owned());
            off += header + ev.len as usize;
        }
        names
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}
