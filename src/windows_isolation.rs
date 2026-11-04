use crate::error::{Error, Result};

pub fn admit() -> Result<()> {
    if !cfg!(windows) {
        return Err(Error::new("windows isolation does not run on this host"));
    }
    // CExecSvc is the guest service Microsoft runs inside a Windows container.
    if !container_guest() {
        return Err(Error::new("windows isolation requires a container guest"));
    }
    Ok(())
}

fn container_guest() -> bool {
    #[cfg(windows)]
    {
        service_present(r"SYSTEM\CurrentControlSet\Services\cexecsvc")
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
fn service_present(subkey: &str) -> bool {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    type Handle = *mut std::ffi::c_void;
    const KEY_READ: u32 = 0x20019;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegOpenKeyExW(
            key: Handle,
            subkey: *const u16,
            options: u32,
            access: u32,
            result: *mut Handle,
        ) -> i32;
        fn RegCloseKey(key: Handle) -> i32;
    }

    let name: Vec<u16> = OsStr::new(subkey).encode_wide().chain(Some(0)).collect();
    let mut opened = ptr::null_mut();
    let root = 0x8000_0002u32 as i32 as isize as Handle;
    let status = unsafe { RegOpenKeyExW(root, name.as_ptr(), 0, KEY_READ, &mut opened) };
    if status == 0 {
        unsafe {
            RegCloseKey(opened);
        }
        true
    } else {
        false
    }
}

pub fn laptop_bypass() -> Result<()> {
    Err(Error::new("bypass is rejected on a laptop workspace"))
}
