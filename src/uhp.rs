use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::session::Session;

static CURRENT: Mutex<Option<Arc<Hub>>> = Mutex::new(None);

struct Hub {
    emit: Box<dyn Fn(&str, bool) + Send + Sync>,
    focus_on: Box<dyn Fn(u64) + Send + Sync>,
    inputs: Mutex<VecDeque<String>>,
    permit: Mutex<Option<bool>>,
    asking: AtomicU64,
    session_id: String,
}

impl Hub {
    fn install(self) -> Arc<Hub> {
        let hub = Arc::new(self);
        *CURRENT.lock().expect("uhp") = Some(Arc::clone(&hub));
        hub
    }

    fn focus(&self, id: u64) {
        (self.focus_on)(id);
    }

    fn raw(&self, line: &str) {
        (self.emit)(line, false);
    }

    fn note(&self, method: &str, mut params: Value, focused: bool) {
        if let Some(object) = params.as_object_mut() {
            object
                .entry("session_id")
                .or_insert_with(|| json!(self.session_id));
        }
        let line = json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string();
        (self.emit)(&line, focused);
    }

    fn push_input(&self, text: &str) {
        self.inputs.lock().expect("uhp").push_back(text.to_string());
    }

    fn answer(&self, id: u64, allow: bool) {
        let open = self
            .asking
            .compare_exchange(id, 0, Ordering::SeqCst, Ordering::SeqCst);
        if id != 0 && open.is_ok() {
            *self.permit.lock().expect("uhp") = Some(allow);
        }
    }

    #[cfg(windows)]
    pub(super) fn new(
        session_id: String,
        emit: Box<dyn Fn(&str, bool) + Send + Sync>,
        focus_on: Box<dyn Fn(u64) + Send + Sync>,
    ) -> Self {
        Self {
            emit,
            focus_on,
            inputs: Mutex::new(VecDeque::new()),
            permit: Mutex::new(None),
            asking: AtomicU64::new(0),
            session_id,
        }
    }
}

pub(crate) fn publish(method: &str, params: Value) {
    let Some(hub) = CURRENT.lock().expect("uhp").clone() else {
        return;
    };
    hub.note(method, params, false);
}

pub(crate) fn publish_ask(name: &str, arguments: &str) {
    static NEXT_ASK: AtomicU64 = AtomicU64::new(1);
    let Some(hub) = CURRENT.lock().expect("uhp").clone() else {
        return;
    };
    let id = NEXT_ASK.fetch_add(1, Ordering::SeqCst);
    *hub.permit.lock().expect("uhp") = None;
    hub.asking.store(id, Ordering::SeqCst);
    hub.note(
        "permission/ask",
        json!({"id": id, "name": name, "arguments": arguments}),
        true,
    );
}

pub(crate) fn publish_diff(repo: &Path) {
    publish("session/diff", json!({"diff": git_diff(repo)}));
}

pub(crate) fn poll_input() -> Option<String> {
    let Some(hub) = CURRENT.lock().expect("uhp").clone() else {
        return None;
    };
    let next = hub.inputs.lock().expect("uhp").pop_front();
    next
}

pub(crate) fn poll_permission() -> Option<bool> {
    let Some(hub) = CURRENT.lock().expect("uhp").clone() else {
        return None;
    };
    let next = hub.permit.lock().expect("uhp").take();
    next
}

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
    hub: Option<Arc<Hub>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(hub) = &self.hub {
            let mut current = CURRENT.lock().expect("uhp");
            if current.as_ref().is_some_and(|live| Arc::ptr_eq(live, hub)) {
                *current = None;
            }
        }
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
        // macOS sockaddr_un holds 104 bytes, including the trailing NUL.
        if unix_len(&socket) >= 103 {
            return Ok(Server::idle(socket));
        }
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
        let clients: Arc<Mutex<Vec<(u64, std::os::unix::net::UnixStream)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let focused: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
        let ids = Arc::new(AtomicU64::new(1));
        let hub = Hub {
            emit: {
                let clients = Arc::clone(&clients);
                let focused = Arc::clone(&focused);
                Box::new(move |line, only| emit_unix(&clients, &focused, line, only))
            },
            focus_on: {
                let focused = Arc::clone(&focused);
                Box::new(move |id| {
                    *focused.lock().expect("clients") = Some(id);
                })
            },
            inputs: Mutex::new(VecDeque::new()),
            permit: Mutex::new(None),
            asking: AtomicU64::new(0),
            session_id: session_id.clone(),
        };
        let hub = hub.install();
        let live = Arc::clone(&hub);
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
                let id = ids.fetch_add(1, Ordering::Relaxed);
                clients.lock().expect("clients").push((id, writer));
                let hub = Arc::clone(&live);
                let session_dir = session_dir.clone();
                let session_id = session_id.clone();
                let repo = repo.clone();
                thread::spawn(move || {
                    let _ = reader(stream, &hub, id, &session_dir, &session_id, &repo);
                });
            }
        });
        return Ok(Server {
            socket,
            shutdown,
            thread: Some(thread),
            hub: Some(hub),
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
fn unix_len(path: &Path) -> usize {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().len()
}

#[cfg(unix)]
fn emit_unix(
    clients: &Mutex<Vec<(u64, std::os::unix::net::UnixStream)>>,
    focused: &Mutex<Option<u64>>,
    line: &str,
    only_focused: bool,
) {
    let target = focused.lock().expect("clients").clone();
    let streams = {
        let guard = clients.lock().expect("clients");
        guard
            .iter()
            .filter(|(id, _)| !only_focused || target == Some(*id))
            .filter_map(|(_, stream)| stream.try_clone().ok())
            .collect::<Vec<_>>()
    };
    let payload = format!("{line}\n");
    for mut stream in streams {
        let _ = stream.write_all(payload.as_bytes());
        let _ = stream.flush();
    }
}

#[cfg(unix)]
fn reader(
    stream: std::os::unix::net::UnixStream,
    hub: &Arc<Hub>,
    id: u64,
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
            hub.focus(id);
            let response = dispatch_rpc(&request, session_dir, session_id, repo);
            deliver(hub, &request);
            hub.raw(&response);
        }
        line.clear();
    }
    Ok(())
}

fn deliver(hub: &Hub, request: &Value) {
    match request.get("method").and_then(Value::as_str) {
        Some("session/input") => {
            let text = request
                .pointer("/params/text")
                .and_then(Value::as_str)
                .unwrap_or("");
            hub.push_input(text);
            hub.note("session/stream", json!({"text": text}), false);
        }
        Some("permission/respond") => {
            let id = request.pointer("/params/id").and_then(Value::as_u64);
            let allow = request.pointer("/params/allow").and_then(Value::as_bool);
            if let (Some(id), Some(allow)) = (id, allow) {
                hub.answer(id, allow);
            }
        }
        _ => {}
    }
}

fn dispatch_rpc(request: &Value, session_dir: &Path, session_id: &str, repo: &Path) -> String {
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let id = request.get("id").cloned();
    let params = request.get("params").cloned().unwrap_or(json!({}));
    let mut result = match method {
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
    if let Some(object) = result.as_object_mut() {
        object
            .entry("session_id")
            .or_insert_with(|| json!(session_id));
    }
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

impl Server {
    #[cfg(unix)]
    fn idle(socket: PathBuf) -> Self {
        Self {
            socket,
            shutdown: Arc::new(AtomicBool::new(true)),
            thread: None,
            hub: None,
        }
    }
}

fn read_transcript(session_dir: &Path) -> String {
    fs::read_to_string(session_dir.join("transcript.jsonl")).unwrap_or_default()
}

fn append_line(session_dir: &Path, value: &Value) {
    let _ = crate::session::append_record(session_dir, value);
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
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
        let clients: Arc<Mutex<Vec<(u64, PipeHandle)>>> = Arc::new(Mutex::new(Vec::new()));
        let focused: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
        let ids = Arc::new(AtomicU64::new(1));
        let hub = super::Hub::new(
            session_id.clone(),
            {
                let clients = Arc::clone(&clients);
                let focused = Arc::clone(&focused);
                Box::new(move |line, only| emit_pipe(&clients, &focused, line, only))
            },
            {
                let focused = Arc::clone(&focused);
                Box::new(move |id| {
                    *focused.lock().expect("clients") = Some(id);
                })
            },
        )
        .install();
        let live = Arc::clone(&hub);
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
                let id = ids.fetch_add(1, Ordering::Relaxed);
                clients
                    .lock()
                    .expect("clients")
                    .push((id, PipeHandle(writer)));
                let hub = Arc::clone(&live);
                let session_dir = session_dir.clone();
                let session_id = session_id.clone();
                let repo = repo.clone();
                let pipe = PipeHandle(pipe);
                thread::spawn(move || {
                    // Edition 2021 would otherwise capture only the raw pointer, which is not Send.
                    let pipe = pipe;
                    read_pipe(pipe.0, &hub, id, &session_dir, &session_id, &repo);
                    let _ = unsafe { DisconnectNamedPipe(pipe.0) };
                    close_pipe(pipe.0);
                });
            }
        });
        Ok(Server {
            socket,
            shutdown,
            thread: Some(thread),
            hub: Some(hub),
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

    fn emit_pipe(
        clients: &Mutex<Vec<(u64, PipeHandle)>>,
        focused: &Mutex<Option<u64>>,
        line: &str,
        only_focused: bool,
    ) {
        let target = *focused.lock().expect("clients");
        let payload = format!("{line}\n");
        let guard = clients.lock().expect("clients");
        for (id, handle) in guard.iter() {
            if only_focused && target != Some(*id) {
                continue;
            }
            let _ = write_handle(handle.0, payload.as_bytes());
        }
    }

    fn read_pipe(
        pipe: Handle,
        hub: &Arc<super::Hub>,
        id: u64,
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
                    hub.focus(id);
                    let response = dispatch_rpc(&request, session_dir, session_id, repo);
                    super::deliver(hub, &request);
                    hub.raw(&response);
                }
            }
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

    fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
        text.as_ref().encode_wide().chain(Some(0)).collect()
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
