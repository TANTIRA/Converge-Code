// Platform termios ABI. Callers change only c_lflag and c_cc, and restore the saved copy.
#[cfg(all(unix, target_os = "macos"))]
pub(crate) fn zero_termios() -> Termios {
    Termios {
        c_iflag: 0,
        c_oflag: 0,
        c_cflag: 0,
        c_lflag: 0,
        c_cc: [0; 20],
        c_ispeed: 0,
        c_ospeed: 0,
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn zero_termios() -> Termios {
    Termios {
        c_iflag: 0,
        c_oflag: 0,
        c_cflag: 0,
        c_lflag: 0,
        c_line: 0,
        c_cc: [0; 32],
        c_ispeed: 0,
        c_ospeed: 0,
    }
}

#[cfg(unix)]
extern "C" {
    pub(crate) fn tcgetattr(fd: i32, termios: *mut Termios) -> i32;
    pub(crate) fn tcsetattr(fd: i32, optional_actions: i32, termios: *const Termios) -> i32;
}

#[cfg(all(unix, target_os = "macos"))]
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Termios {
    pub(crate) c_iflag: u64,
    pub(crate) c_oflag: u64,
    pub(crate) c_cflag: u64,
    pub(crate) c_lflag: u64,
    pub(crate) c_cc: [u8; 20],
    pub(crate) c_ispeed: u64,
    pub(crate) c_ospeed: u64,
}

#[cfg(all(unix, target_os = "macos"))]
const _: () = assert!(std::mem::size_of::<Termios>() == 72);

#[cfg(all(unix, not(target_os = "macos")))]
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Termios {
    pub(crate) c_iflag: u32,
    pub(crate) c_oflag: u32,
    pub(crate) c_cflag: u32,
    pub(crate) c_lflag: u32,
    pub(crate) c_line: u8,
    pub(crate) c_cc: [u8; 32],
    pub(crate) c_ispeed: u32,
    pub(crate) c_ospeed: u32,
}

#[cfg(all(unix, not(target_os = "macos")))]
const _: () = assert!(std::mem::size_of::<Termios>() == 60);
