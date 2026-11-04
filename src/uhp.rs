use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::session::Session;

pub fn endpoint(home: &Path, session_id: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(format!(r"\\.\pipe\converge-{session_id}"))
    } else {
        home.join("run").join(format!("{session_id}.sock"))
    }
}

pub struct Server {
    pub socket: PathBuf,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        {
            let _ = std::os::unix::net::UnixStream::connect(&self.socket);
        }
        #[cfg(windows)]
        let _wake = windows_pipe::connect(&self.socket);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.socket);
    }
}

pub fn serve(home: &Path, session: &Session, repo: &Path) -> Result<Server> {
    let socket = endpoint(home, &session.id);
    #[cfg(unix)]
    {
        if let Some(parent) = socket.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| Error::new(format!("create {}: {error}", parent.display())))?;
            private_dir(parent)?;
        }
        let _ = fs::remove_file(&socket);
        let listener = std::os::unix::net::UnixListener::bind(&socket)
            .map_err(|error| Error::new(format!("bind {}: {error}", socket.display())))?;
        private_socket(&socket)?;
        listener
            .set_nonblocking(false)
            .map_err(|error| Error::new(format!("bind {}: {error}", socket.display())))?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&shutdown);
        let session_dir = session.dir.clone();
        let session_id = session.id.clone();
        let repo = repo.to_path_buf();
        let clients: Arc<Mutex<Vec<std::os::unix::net::UnixStream>>> =
            Arc::new(Mutex::new(Vec::new()));
        let thread = thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                let Ok((stream, _)) = listener.accept() else {
                    break;
                };
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                let _ = stream.set_nonblocking(false);
                let Ok(writer) = stream.try_clone() else {
                    continue;
                };
                clients.lock().expect("clients").push(writer);
                let clients = Arc::clone(&clients);
                let session_dir = session_dir.clone();
                let session_id = session_id.clone();
                let repo = repo.clone();
                thread::spawn(move || {
                    let _ = reader(stream, &clients, &session_dir, &session_id, &repo);
                });
            }
        });
        return Ok(Server {
            socket,
            shutdown,
            thread: Some(thread),
        });
    }
    #[cfg(windows)]
    {
        windows_pipe::serve(socket, session, repo)
    }
}

#[cfg(unix)]
fn private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|error| Error::new(format!("stat {}: {error}", path.display())))?
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions)
        .map_err(|error| Error::new(format!("chmod {}: {error}", path.display())))
}

#[cfg(unix)]
fn private_socket(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|error| Error::new(format!("stat {}: {error}", path.display())))?
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)
        .map_err(|error| Error::new(format!("chmod {}: {error}", path.display())))
}

#[cfg(unix)]
fn reader(
    stream: std::os::unix::net::UnixStream,
    clients: &Arc<Mutex<Vec<std::os::unix::net::UnixStream>>>,
    session_dir: &Path,
    session_id: &str,
    repo: &Path,
) -> Result<()> {
    let mut incoming = BufReader::new(stream);
    let mut line = String::new();
    while incoming
        .read_line(&mut line)
        .ok()
        .filter(|count| *count > 0)
        .is_some()
    {
        let trimmed = line.trim();
        if let Ok(request) = serde_json::from_str::<Value>(trimmed) {
            let response = dispatch_rpc(&request, session_dir, session_id, repo);
            if request.get("method").and_then(Value::as_str) == Some("session/input") {
                let text = request
                    .pointer("/params/text")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let note =
                    json!({"jsonrpc":"2.0","method":"session/stream","params":{"text": text}});
                broadcast(clients, &note.to_string());
            }
            broadcast(clients, &response);
        }
        line.clear();
    }
    Ok(())
}

fn dispatch_rpc(request: &Value, session_dir: &Path, session_id: &str, repo: &Path) -> String {
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let id = request.get("id").cloned();
    let params = request.get("params").cloned().unwrap_or(json!({}));
    let result = match method {
        "session/attach" => json!({
            "session_id": session_id,
            "transcript": read_transcript(session_dir),
        }),
        "session/input" => {
            let text = params.get("text").and_then(Value::as_str).unwrap_or("");
            append_line(session_dir, &json!({"kind": "user", "text": text}));
            broadcast_note(
                session_dir,
                &json!({"jsonrpc": "2.0", "method": "session/stream", "params": {"text": text}}),
            );
            json!({"ok": true})
        }
        "session/selection" => {
            append_line(session_dir, &json!({"kind": "selection", "text": params}));
            json!({"ok": true})
        }
        "permission/respond" => {
            append_line(session_dir, &json!({"kind": "permission", "text": params}));
            json!({"ok": true})
        }
        "session/diff" => json!({"diff": git_diff(repo)}),
        "session/status" => json!({"session_id": session_id}),
        "session/tool" => {
            let note = json!({"jsonrpc": "2.0", "method": "session/tool", "params": params});
            broadcast_note(session_dir, &note);
            json!({"ok": true})
        }
        _ => json!({"error": "unknown method"}),
    };
    let response = if let Some(id) = id {
        json!({"jsonrpc": "2.0", "id": id, "result": result})
    } else {
        result
    };
    response.to_string()
}

fn broadcast_note(session_dir: &Path, note: &Value) {
    let path = session_dir.join("uhp-notes.jsonl");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{note}");
    }
}

#[cfg(unix)]
fn broadcast(clients: &Mutex<Vec<std::os::unix::net::UnixStream>>, line: &str) {
    let streams = {
        let guard = clients.lock().expect("clients");
        guard
            .iter()
            .filter_map(|stream| stream.try_clone().ok())
            .collect::<Vec<_>>()
    };
    let payload = format!("{line}\n");
    for mut stream in streams {
        let _ = stream.write_all(payload.as_bytes());
        let _ = stream.flush();
    }
}

fn read_transcript(session_dir: &Path) -> String {
    fs::read_to_string(session_dir.join("transcript.jsonl")).unwrap_or_default()
}

fn append_line(session_dir: &Path, value: &Value) {
    let path = session_dir.join("transcript.jsonl");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{value}");
    }
}

fn git_diff(repo: &Path) -> String {
    let output = std::process::Command::new("git")
        .args(["diff"])
        .current_dir(repo)
        .output();
    match output {
        Ok(output) => String::from_utf8_lossy(&output.stdout).into_owned(),
        Err(_) => String::new(),
    }
}

#[cfg(windows)]
mod windows_pipe {
    use std::ffi::OsStr;
    use std::fs::OpenOptions;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::ptr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;

    use crate::error::{Error, Result};
    use crate::session::Session;

    use super::{dispatch_rpc, Server};

    type Handle = *mut std::ffi::c_void;
    type Dword = u32;
    type Bool = i32;

    const PIPE_ACCESS_DUPLEX: Dword = 0x0000_0003;
    const PIPE_TYPE_BYTE: Dword = 0x0000_0000;
    const PIPE_READMODE_BYTE: Dword = 0x0000_0000;
    const PIPE_WAIT: Dword = 0x0000_0000;
    const PIPE_UNLIMITED_INSTANCES: Dword = 255;
    const ERROR_PIPE_CONNECTED: Dword = 535;
    const INVALID_HANDLE: isize = -1;

    #[repr(C)]
    struct SecurityAttributes {
        length: u32,
        descriptor: *mut std::ffi::c_void,
        inherit: i32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateNamedPipeW(
            name: *const u16,
            open_mode: Dword,
            pipe_mode: Dword,
            max_instances: Dword,
            out_buffer: Dword,
            in_buffer: Dword,
            default_timeout: Dword,
            security: *const SecurityAttributes,
        ) -> Handle;
        fn ConnectNamedPipe(pipe: Handle, overlapped: *mut u8) -> Bool;
        fn ReadFile(
            pipe: Handle,
            buffer: *mut u8,
            bytes: Dword,
            read: *mut Dword,
            overlapped: *mut u8,
        ) -> Bool;
        fn WriteFile(
            pipe: Handle,
            buffer: *const u8,
            bytes: Dword,
            written: *mut Dword,
            overlapped: *mut u8,
        ) -> Bool;
        fn CloseHandle(handle: Handle) -> Bool;
        fn GetLastError() -> Dword;
        fn DisconnectNamedPipe(pipe: Handle) -> Bool;
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl: *const u16,
            revision: Dword,
            descriptor: *mut *mut std::ffi::c_void,
            size: *mut Dword,
        ) -> Bool;
        fn LocalFree(memory: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }

    struct PipeHandle(Handle);

    unsafe impl Send for PipeHandle {}

    pub(super) fn connect(path: &Path) -> Option<std::fs::File> {
        OpenOptions::new().read(true).write(true).open(path).ok()
    }

    pub(super) fn serve(socket: PathBuf, session: &Session, repo: &Path) -> Result<Server> {
        let first = PipeHandle(create_pipe(&socket)?);
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&shutdown);
        let session_dir = session.dir.clone();
        let session_id = session.id.clone();
        let repo = repo.to_path_buf();
        let name = socket.clone();
        let clients: Arc<Mutex<Vec<PipeHandle>>> = Arc::new(Mutex::new(Vec::new()));
        let thread = thread::spawn(move || {
            let mut ready = Some(first);
            while !flag.load(Ordering::SeqCst) {
                let pipe = match ready.take() {
                    Some(pipe) => pipe.0,
                    None => match create_pipe(&name) {
                        Ok(pipe) => pipe,
                        Err(_) => break,
                    },
                };
                if !connect_pipe(pipe) || flag.load(Ordering::SeqCst) {
                    close_pipe(pipe);
                    break;
                }
                let Ok(writer) = duplicate(pipe) else {
                    close_pipe(pipe);
                    continue;
                };
                clients.lock().expect("clients").push(PipeHandle(writer));
                let clients = Arc::clone(&clients);
                let session_dir = session_dir.clone();
                let session_id = session_id.clone();
                let repo = repo.clone();
                let pipe = PipeHandle(pipe);
                thread::spawn(move || {
                    read_pipe(pipe.0, &clients, &session_dir, &session_id, &repo);
                    let _ = unsafe { DisconnectNamedPipe(pipe.0) };
                    close_pipe(pipe.0);
                });
            }
        });
        Ok(Server {
            socket,
            shutdown,
            thread: Some(thread),
        })
    }

    fn create_pipe(path: &Path) -> Result<Handle> {
        let name = wide(path);
        let sddl = wide("D:P(A;;GA;;;OW)");
        let mut descriptor = ptr::null_mut();
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if converted == 0 {
            return Err(Error::new(format!(
                "named pipe {} could not be secured",
                path.display()
            )));
        }
        let security = SecurityAttributes {
            length: std::mem::size_of::<SecurityAttributes>() as u32,
            descriptor,
            inherit: 0,
        };
        let pipe = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                4096,
                4096,
                0,
                &security,
            )
        };
        unsafe {
            LocalFree(descriptor);
        }
        if pipe.is_null() || pipe as isize == INVALID_HANDLE {
            return Err(Error::new(format!(
                "named pipe {} failed to listen",
                path.display()
            )));
        }
        Ok(pipe)
    }

    fn connect_pipe(pipe: Handle) -> bool {
        let ok = unsafe { ConnectNamedPipe(pipe, ptr::null_mut()) };
        ok != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED
    }

    fn duplicate(pipe: Handle) -> std::io::Result<Handle> {
        let mut copy = ptr::null_mut();
        let process = unsafe { GetCurrentProcess() };
        let ok = unsafe { DuplicateHandle(process, pipe, process, &mut copy, 0, 0, 0x0000_0002) };
        if ok == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(copy)
        }
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn DuplicateHandle(
            source_process: Handle,
            source: Handle,
            target_process: Handle,
            target: *mut Handle,
            access: Dword,
            inherit: Bool,
            options: Dword,
        ) -> Bool;
    }

    fn read_pipe(
        pipe: Handle,
        clients: &Arc<Mutex<Vec<PipeHandle>>>,
        session_dir: &Path,
        session_id: &str,
        repo: &Path,
    ) {
        let mut pending = Vec::new();
        let mut buffer = [0u8; 1024];
        loop {
            let mut read = 0;
            let ok = unsafe {
                ReadFile(
                    pipe,
                    buffer.as_mut_ptr(),
                    buffer.len() as Dword,
                    &mut read,
                    ptr::null_mut(),
                )
            };
            if ok == 0 || read == 0 {
                break;
            }
            pending.extend_from_slice(&buffer[..read as usize]);
            while let Some(index) = pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = pending.drain(..=index).collect();
                let text = String::from_utf8_lossy(&line);
                let trimmed = text.trim();
                if let Ok(request) = serde_json::from_str::<serde_json::Value>(trimmed) {
                    let response = dispatch_rpc(&request, session_dir, session_id, repo);
                    if request.get("method").and_then(|method| method.as_str())
                        == Some("session/input")
                    {
                        let text = request
                            .pointer("/params/text")
                            .and_then(|value| value.as_str())
                            .unwrap_or("");
                        let note = serde_json::json!({"jsonrpc":"2.0","method":"session/stream","params":{"text": text}});
                        broadcast(clients, &note.to_string());
                    }
                    broadcast(clients, &response);
                }
            }
        }
    }

    fn broadcast(clients: &Mutex<Vec<PipeHandle>>, line: &str) {
        let payload = format!("{line}\n");
        let guard = clients.lock().expect("clients");
        for handle in guard.iter() {
            let _ = write_handle(handle.0, payload.as_bytes());
        }
    }

    fn write_handle(pipe: Handle, mut bytes: &[u8]) -> std::io::Result<()> {
        while !bytes.is_empty() {
            let mut written = 0;
            let ok = unsafe {
                WriteFile(
                    pipe,
                    bytes.as_ptr(),
                    bytes.len() as Dword,
                    &mut written,
                    ptr::null_mut(),
                )
            };
            if ok == 0 || written == 0 {
                return Err(std::io::Error::last_os_error());
            }
            bytes = &bytes[written as usize..];
        }
        Ok(())
    }

    fn close_pipe(pipe: Handle) {
        unsafe {
            CloseHandle(pipe);
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        OsStr::new(text).encode_wide().chain(Some(0)).collect()
    }
}

pub fn allowed_methods() -> &'static [&'static str] {
    &[
        "session/attach",
        "session/input",
        "session/selection",
        "permission/respond",
        "session/stream",
        "session/tool",
        "permission/ask",
        "session/diff",
        "session/status",
    ]
}
