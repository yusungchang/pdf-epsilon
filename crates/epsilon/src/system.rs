//! How much memory the computer has, and how much is free now. Used to size the memory for
//! images worked on at once (see `images.rs`). Zero means unknown.

/// Installed memory and memory available now, in bytes.
pub fn memory() -> (u64, u64) {
    #[cfg(test)]
    if let Some(m) = *PRETEND.lock().unwrap_or_else(|e| e.into_inner()) {
        return m;
    }
    imp::memory()
}

/// Tests pretend to run on another machine.
#[cfg(test)]
pub static PRETEND: std::sync::Mutex<Option<(u64, u64)>> = std::sync::Mutex::new(None);

#[cfg(target_os = "macos")]
mod imp {
    pub fn memory() -> (u64, u64) {
        (total(), available())
    }

    fn total() -> u64 {
        let mut bytes: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        let ok = unsafe { libc::sysctlbyname(c"hw.memsize".as_ptr(), (&mut bytes as *mut u64).cast(), &mut len, std::ptr::null_mut(), 0) };
        if ok == 0 { bytes } else { 0 }
    }

    /// Free, inactive, purgeable and speculative pages: what macOS can hand out without
    /// compressing or swapping.
    #[allow(deprecated)]
    fn available() -> u64 {
        let mut stats: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
        let mut count = libc::HOST_VM_INFO64_COUNT;
        let ok = unsafe { libc::host_statistics64(libc::mach_host_self(), libc::HOST_VM_INFO64, (&mut stats as *mut libc::vm_statistics64).cast(), &mut count) };
        if ok != 0 {
            return 0;
        }
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(0) as u64;
        let pages = stats.free_count as u64 + stats.inactive_count as u64 + stats.purgeable_count as u64 + stats.speculative_count as u64;
        pages * page
    }
}

#[cfg(target_os = "linux")]
mod imp {
    pub fn memory() -> (u64, u64) {
        let Ok(info) = std::fs::read_to_string("/proc/meminfo") else { return (0, 0) };
        let field = |name: &str| {
            info.lines()
                .find(|l| l.starts_with(name))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
                .map(|kb| kb * 1024)
                .unwrap_or(0)
        };
        (field("MemTotal:"), field("MemAvailable:"))
    }
}

#[cfg(windows)]
mod imp {
    pub fn memory() -> (u64, u64) {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
            return (0, 0);
        }
        (status.ullTotalPhys, status.ullAvailPhys)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod imp {
    pub fn memory() -> (u64, u64) {
        (0, 0)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_this_computers_memory() {
        let (total, available) = super::imp::memory();
        assert!(total >= 1 << 30, "installed {total}");
        assert!(available > 0 && available <= total, "available {available} of {total}");
    }
}
