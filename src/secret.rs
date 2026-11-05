use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
#[cfg(unix)]
use crate::termios::{tcgetattr, tcsetattr, zero_termios, Termios};

pub fn read_secret(home: &Path, name: &str) -> Result<Option<String>> {
    let path = env_path(home);
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path)
        .map_err(|error| Error::new(format!("read {}: {error}", path.display())))?;
    for line in text.lines() {
        if let Some((key, value)) = parse_assignment(line) {
            if key == name && !value.is_empty() {
                return Ok(Some(value));
            }
        }
    }
    Ok(None)
}

pub fn write_secret(home: &Path, name: &str, value: &str) -> Result<()> {
    if !is_env_name(name) {
        return Err(Error::new("secret name is invalid"));
    }
    if value.is_empty() || value.contains('\n') || value.contains('\r') {
        return Err(Error::new("secret value is empty"));
    }
    let path = env_path(home);
    let mut lines = Vec::new();
    if path.is_file() {
        let text = fs::read_to_string(&path)
            .map_err(|error| Error::new(format!("read {}: {error}", path.display())))?;
        let mut replaced = false;
        for line in text.lines() {
            if parse_assignment(line).is_some_and(|(key, _)| key == name) {
                if !replaced {
                    lines.push(format!("{name}={}", encode(value)));
                    replaced = true;
                }
            } else {
                lines.push(line.to_string());
            }
        }
        if !replaced {
            lines.push(format!("{name}={}", encode(value)));
        }
    } else {
        lines.push(format!("{name}={}", encode(value)));
    }
    write_private(&path, &lines.join("\n"))
}

pub fn prompt_secret(label: &str) -> Result<String> {
    if !io::stdin().is_terminal() {
        return Err(Error::new("converge setup needs a terminal"));
    }
    let mut stdout = io::stdout();
    write!(stdout, "{label}").map_err(|error| Error::new(error.to_string()))?;
    stdout
        .flush()
        .map_err(|error| Error::new(error.to_string()))?;
    let _guard = EchoOff::enable()?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|error| Error::new(format!("stdin: {error}")))?;
    writeln!(stdout).map_err(|error| Error::new(error.to_string()))?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

pub fn read_reply() -> Result<String> {
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|error| Error::new(format!("stdin: {error}")))?;
    Ok(line.trim().to_string())
}

fn env_path(home: &Path) -> PathBuf {
    home.join(".env")
}

fn write_private(path: &Path, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| Error::new(format!("write {}: {error}", path.display())))?;
    }
    let tmp = path.with_file_name(".env.tmp");
    let mut file = open_private(&tmp)?;
    file.write_all(body.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|error| Error::new(format!("write {}: {error}", tmp.display())))?;
    fs::rename(&tmp, path).map_err(|error| Error::new(format!("write {}: {error}", path.display())))
}

fn open_private(path: &Path) -> Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|error| Error::new(format!("write {}: {error}", path.display())))
}

fn encode(value: &str) -> String {
    if value
        .chars()
        .any(|ch| ch.is_whitespace() || ch == '#' || ch == '"')
    {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

fn parse_assignment(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let rest = trimmed.strip_prefix("export ").unwrap_or(trimmed);
    let (name, value) = rest.split_once('=')?;
    let name = name.trim();
    if !is_env_name(name) {
        return None;
    }
    Some((name.to_string(), unquote(value.trim())))
}

fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[0] == bytes[bytes.len() - 1]
    {
        let inner = &value[1..value.len() - 1];
        if bytes[0] == b'"' {
            return inner.replace("\\\"", "\"").replace("\\\\", "\\");
        }
        return inner.to_string();
    }
    value.to_string()
}

fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

struct EchoOff {
    #[cfg(unix)]
    fd: i32,
    #[cfg(unix)]
    original: Termios,
    #[cfg(windows)]
    handle: *mut std::ffi::c_void,
    #[cfg(windows)]
    original: u32,
}

impl EchoOff {
    fn enable() -> Result<Self> {
        #[cfg(unix)]
        {
            enable_unix()
        }
        #[cfg(windows)]
        {
            enable_windows()
        }
        #[cfg(not(any(unix, windows)))]
        {
            Ok(Self {})
        }
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            tcsetattr(self.fd, 0, &self.original);
        }
        #[cfg(windows)]
        unsafe {
            SetConsoleMode(self.handle, self.original);
        }
    }
}

#[cfg(unix)]
fn enable_unix() -> Result<EchoOff> {
    use std::os::unix::io::AsRawFd;
    let fd = io::stdin().as_raw_fd();
    let mut original = zero_termios();
    let rc = unsafe { tcgetattr(fd, &mut original) };
    if rc != 0 {
        return Err(Error::new("could not read the terminal"));
    }
    let mut hidden = original;
    hidden.c_lflag &= !8;
    let rc = unsafe { tcsetattr(fd, 0, &hidden) };
    if rc != 0 {
        return Err(Error::new("could not hide terminal input"));
    }
    Ok(EchoOff { fd, original })
}

#[cfg(windows)]
fn enable_windows() -> Result<EchoOff> {
    let handle = unsafe { GetStdHandle(-10) };
    let mut original = 0;
    if unsafe { GetConsoleMode(handle, &mut original) } == 0 {
        return Err(Error::new("could not read the terminal"));
    }
    if unsafe { SetConsoleMode(handle, original & !4) } == 0 {
        return Err(Error::new("could not hide terminal input"));
    }
    Ok(EchoOff { handle, original })
}

#[cfg(windows)]
extern "system" {
    fn GetStdHandle(nstdhandle: i32) -> *mut std::ffi::c_void;
    fn GetConsoleMode(handle: *mut std::ffi::c_void, mode: *mut u32) -> i32;
    fn SetConsoleMode(handle: *mut std::ffi::c_void, mode: u32) -> i32;
}
