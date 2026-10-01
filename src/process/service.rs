// Hatter ecosystem 2026: preserve framing timeout identity independently of execution.
use super::{Owner, ProcessCommand, ProcessReply, error};
use crate::{LocalInferenceError, RuntimeRegistry};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) fn nonce() -> Result<String, LocalInferenceError> {
    let mut bytes = [0; 32];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|_| error("process-identity-unavailable"))?;
    crate::configuration::identity("runtime/process/incarnation/1", &bytes)
}
fn endpoint(root: &Path) -> Result<PathBuf, LocalInferenceError> {
    let root = crate::fsutil::open_registry_directory(root)?;
    let id = crate::configuration::identity("runtime/process/endpoint/1", &root)?;
    Ok(std::env::temp_dir().join(format!("zixcel-runtime-{id}.sock")))
}
fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>, LocalInferenceError> {
    crowsi_transport_foundation::process::read_frame(stream, 128 * 1024, Duration::from_millis(500))
        .map_err(|_| error("process-transport-unavailable"))
}
fn write_frame(stream: &mut UnixStream, body: &[u8]) -> Result<(), LocalInferenceError> {
    crowsi_transport_foundation::process::write_frame(
        stream,
        body,
        128 * 1024,
        Duration::from_millis(500),
    )
    .map_err(|_| error("process-transport-unavailable"))
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    result: Option<ProcessReply>,
    reason: Option<String>,
}
/// Contacts an already running owner. Never starts a service or repairs an endpoint.
/// # Errors
/// Missing owner, bounded transport failure and exact owner failures remain errors.
pub fn request(root: &Path, command: &ProcessCommand) -> Result<ProcessReply, LocalInferenceError> {
    command.validate()?;
    let mut stream =
        UnixStream::connect(endpoint(root)?).map_err(|_| error("process-owner-unavailable"))?;
    let timeout = Some(Duration::from_secs(35));
    stream
        .set_read_timeout(timeout)
        .and_then(|()| stream.set_write_timeout(timeout))
        .map_err(|_| error("process-transport-unavailable"))?;
    write_frame(
        &mut stream,
        &serde_json::to_vec(&command).map_err(|_| error("invalid-configuration"))?,
    )?;
    let wire: Wire = serde_json::from_slice(
        &crowsi_transport_foundation::process::read_frame(
            &mut stream,
            16 * 1024,
            Duration::from_secs(35),
        )
        .map_err(|_| error("process-transport-unavailable"))?,
    )
    .map_err(|_| error("process-transport-invalid"))?;
    match (wire.result, wire.reason) {
        (Some(result), None) => Ok(result),
        (None, Some(reason))
            if !reason.is_empty()
                && reason.len() <= 128
                && reason.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') =>
        {
            Ok(ProcessReply {
                process: None,
                rejection: Some(reason),
            })
        }
        _ => Err(error("process-transport-invalid")),
    }
}
struct Socket(PathBuf);
pub(crate) fn inference_request(
    root: &Path,
    command: &crate::inference::Command,
) -> Result<crate::inference::Reply, crate::inference::InferenceError> {
    use crate::inference::InferenceError as E;
    let mut stream = UnixStream::connect(endpoint(root).map_err(|_| E::RuntimeUnavailable)?)
        .map_err(|_| E::RuntimeUnavailable)?;
    let bytes = serde_json::to_vec(command).map_err(|_| E::InvalidRequest)?;
    let framing = |e: std::io::Error| match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => E::TransportTimeout,
        _ => E::BackendUnavailable,
    };
    crowsi_transport_foundation::process::write_frame(
        &mut stream,
        &bytes,
        128 * 1024,
        Duration::from_millis(500),
    )
    .map_err(framing)?;
    // The owner does no model computation on this lane. No automatic resend if
    // the acceptance reply is lost; inspect the original exact request instead.
    let bytes = crowsi_transport_foundation::process::read_frame(
        &mut stream,
        128 * 1024,
        Duration::from_millis(500),
    )
    .map_err(framing)?;
    serde_json::from_slice(&bytes).map_err(|_| E::BackendProtocolError)
}
impl Drop for Socket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
/// Explicit foreground owner. No runtime starts until an exact Start request.
/// A held OS file lock prevents a second owner; stale socket removal is explicit setup.
/// # Errors
/// Missing registry, existing owner, unsafe endpoint and transport failure reject.
fn owner_lock(root: &Path) -> Result<File, LocalInferenceError> {
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(0x20000)
        .open(root.join("process-owner.lock"))
        .map_err(|_| error("process-owner-unavailable"))?;
    if !lock
        .metadata()
        .map_err(|_| error("process-owner-unavailable"))?
        .is_file()
    {
        return Err(error("process-owner-unavailable"));
    }
    lock.try_lock()
        .map_err(|_| error("process-owner-already-running"))?;
    Ok(lock)
}
/// Explicit foreground service; see the owner lifecycle contract for limits.
/// # Errors
/// Missing registry, existing owner and unsafe endpoints reject.
pub fn serve(root: &Path) -> Result<(), LocalInferenceError> {
    serve_inner(root, None)
}

/// Explicit foreground supervisor for a registry containing exactly one
/// admitted runtime. The caller owns this process lifetime; Zixcel retains
/// exclusive ownership of the native runtime and its resources.
/// # Errors
/// Missing/ambiguous runtime admission and all normal owner startup failures
/// remain explicit.
pub fn supervise(root: &Path, requested: Option<&str>) -> Result<(), LocalInferenceError> {
    let registry = RuntimeRegistry::open_existing(root)?;
    let runtime = if let Some(reference) = requested {
        registry.runtime(reference)?.runtime_ref
    } else {
        let runtimes = registry.snapshot()?.runtimes;
        if runtimes.len() != 1 {
            return Err(error(if runtimes.is_empty() {
                "runtime-not-found"
            } else {
                "runtime-selection-required"
            }));
        }
        runtimes[0].runtime_ref.clone()
    };
    serve_inner(root, Some(runtime))
}

fn initial_owner(
    root: &Path,
    registry: &RuntimeRegistry,
    runtime: Option<String>,
) -> Result<Owner, LocalInferenceError> {
    let mut owner = Owner {
        requests: super::inference::Requests::default(),
        live: None,
        accepted: registry.process_requests()?,
    };
    // Explicit owner recovery reclaims only materializations recorded by this owner.
    // A restarted owner never restores a child/PID/Ready snapshot.
    for (_, reference) in owner.accepted.values() {
        super::launch::reclaim(root, reference)?;
    }
    if let Some(runtime_ref) = runtime {
        let request_ref = nonce()?;
        owner.start(root, &runtime_ref, &request_ref, None)?;
    }
    Ok(owner)
}

fn serve_inner(root: &Path, runtime: Option<String>) -> Result<(), LocalInferenceError> {
    let registry = RuntimeRegistry::open_existing(root)?;
    let lock = owner_lock(root)?;
    let socket = endpoint(root)?;
    if let Ok(meta) = fs::symlink_metadata(&socket) {
        use std::os::unix::fs::FileTypeExt;
        if !meta.file_type().is_socket()
            || meta.uid()
                != fs::metadata(root)
                    .map_err(|_| error("process-endpoint-invalid"))?
                    .uid()
        {
            return Err(error("process-endpoint-invalid"));
        }
        fs::remove_file(&socket).map_err(|_| error("process-endpoint-invalid"))?;
    }
    let listener = UnixListener::bind(&socket).map_err(|_| error("process-endpoint-invalid"))?;
    let socket_guard = Socket(socket.clone());
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .map_err(|_| error("process-endpoint-invalid"))?;
    let mut owner = initial_owner(root, &registry, runtime)?;
    for client in listener.incoming() {
        let Ok(mut stream) = client else { continue };
        let timeout = Some(Duration::from_millis(500));
        if stream
            .set_read_timeout(timeout)
            .and_then(|()| stream.set_write_timeout(timeout))
            .is_err()
        {
            continue;
        }
        let mut shutdown = false;
        let Ok(bytes) = read_frame(&mut stream) else {
            continue;
        };
        if let Ok(command) = serde_json::from_slice::<crate::inference::Command>(&bytes) {
            let reply = owner.requests.command(owner.live.as_ref(), command);
            if let Ok(bytes) = serde_json::to_vec(&reply) {
                let _ = write_frame(&mut stream, &bytes);
            }
            continue;
        }
        let result = (|| {
            let command: ProcessCommand =
                serde_json::from_slice(&bytes).map_err(|_| error("invalid-configuration"))?;
            command.validate()?;
            match command {
                ProcessCommand::Inspect {} => owner.inspect(),
                ProcessCommand::Start {
                    runtime_ref,
                    request_ref,
                } => owner.start(root, &runtime_ref, &request_ref, None),
                ProcessCommand::Stop { process_ref } => owner.stop(&process_ref),
                ProcessCommand::Restart {
                    process_ref,
                    runtime_ref,
                    request_ref,
                } => {
                    if owner.accepted.contains_key(&request_ref) {
                        return owner.start(root, &runtime_ref, &request_ref, Some(&process_ref));
                    }
                    owner.stop(&process_ref)?;
                    owner.start(root, &runtime_ref, &request_ref, Some(&process_ref))
                }
                ProcessCommand::Shutdown {} => {
                    if let Some(live) = owner.live.as_mut() {
                        live.stop()?;
                    }
                    shutdown = true;
                    owner.inspect()
                }
            }
        })();
        let wire = match result {
            Ok(result) => Wire {
                result: Some(result),
                reason: None,
            },
            Err(e) => Wire {
                result: None,
                reason: Some(e.code().into()),
            },
        };
        if let Ok(bytes) = serde_json::to_vec(&wire) {
            let _ = write_frame(&mut stream, &bytes);
        }
        if shutdown {
            break;
        }
    }
    drop(owner);
    drop(socket_guard);
    drop(lock);
    Ok(())
}
