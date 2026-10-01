//! Fixed llama.cpp adapter. All native names/HTTP details stay with this owner.
mod inference;
use super::termination::Capture;
use super::{ProcessSnapshot, ProcessState, error, snapshot};
use crate::{
    AdmittedArtifact, AdmittedRuntime, LocalInferenceError, RuntimeDistribution, RuntimeRegistry,
    VerificationControl,
};
pub(super) use inference::infer;
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    net::{SocketAddr, TcpListener},
    os::unix::{
        fs::{PermissionsExt, symlink},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

struct Prepared {
    runtime: AdmittedRuntime,
    distribution: RuntimeDistribution,
    artifacts: Vec<AdmittedArtifact>,
    model: PathBuf,
    layout: PathBuf,
}
fn prepare(
    root: &Path,
    runtime_ref: &str,
    process_ref: &str,
    control: &VerificationControl,
) -> Result<Prepared, LocalInferenceError> {
    let registry = RuntimeRegistry::open_existing(root)?;
    registry.verify_execution_artifacts(runtime_ref, control)?;
    let runtime = registry.runtime(runtime_ref)?;
    if runtime.definition.engine != "llama.cpp" || runtime.definition.engine_revision != "b10741" {
        return Err(error("unsupported-runtime"));
    }
    let image = registry.snapshot()?;
    let distribution = image
        .distributions
        .iter()
        .find(|d| d.distribution_ref == runtime.definition.implementation_ref)
        .ok_or_else(|| error("distribution-not-found"))?
        .definition
        .clone();
    // An explicitly admitted CPU derivation is necessary; never silently filter here.
    let selected = image
        .artifacts
        .iter()
        .find(|a| {
            a.definition.sha256
                == "469e17c98128fce2bd9e8397a6ec0e2c4b399e03a35a5c971ff8985f090688df"
        })
        .ok_or_else(|| error("cpu-selection-unavailable"))?;
    if distribution.modules.len() != 1
        || !distribution
            .files
            .iter()
            .any(|f| f.path == distribution.modules[0] && f.artifact_ref == selected.artifact_ref)
    {
        return Err(error("cpu-execution-distribution-required"));
    }
    if distribution.host.os != std::env::consts::OS
        || distribution.host.architecture != std::env::consts::ARCH
    {
        return Err(error("host-incompatible"));
    }
    let model = image
        .artifacts
        .iter()
        .find(|a| a.artifact_ref == runtime.definition.model_artifact_ref)
        .ok_or_else(|| error("artifact-not-found"))?;
    let model = root.join("objects").join(&model.definition.sha256);
    let layout = root.join("process-materialization").join(process_ref);
    Ok(Prepared {
        runtime,
        distribution,
        artifacts: image.artifacts,
        model,
        layout,
    })
}
fn materialize(root: &Path, p: &Prepared) -> Result<(), LocalInferenceError> {
    crate::fsutil::prepare_directory(&root.join("process-materialization"))?;
    fs::create_dir(&p.layout).map_err(|_| error("process-layout-conflict"))?;
    fs::set_permissions(&p.layout, fs::Permissions::from_mode(0o700))
        .map_err(|_| error("process-layout-invalid"))?;
    for f in &p.distribution.files {
        let a = p
            .artifacts
            .iter()
            .find(|a| a.artifact_ref == f.artifact_ref)
            .ok_or_else(|| error("artifact-not-found"))?;
        // Immutable native bytes already executable in the store; hardlink, never chmod store.
        fs::hard_link(
            root.join("objects").join(&a.definition.sha256),
            p.layout.join(&f.path),
        )
        .map_err(|_| error("process-materialization-failed"))?;
    }
    for a in &p.distribution.aliases {
        symlink(&a.target, p.layout.join(&a.path))
            .map_err(|_| error("process-materialization-failed"))?;
    }
    check_layout(p)
}
fn check_names(p: &Prepared) -> Result<(), LocalInferenceError> {
    let expected: BTreeSet<_> = p
        .distribution
        .files
        .iter()
        .map(|f| f.path.clone())
        .chain(p.distribution.aliases.iter().map(|a| a.path.clone()))
        .collect();
    let actual: BTreeSet<_> = fs::read_dir(&p.layout)
        .map_err(|_| error("process-layout-invalid"))?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()
        .map_err(|_| error("process-layout-invalid"))?;
    if actual != expected {
        return Err(error("process-layout-invalid"));
    }
    Ok(())
}
fn check_layout(p: &Prepared) -> Result<(), LocalInferenceError> {
    check_names(p)?;
    let control = VerificationControl::new(Duration::from_secs(30));
    for f in &p.distribution.files {
        let a = p
            .artifacts
            .iter()
            .find(|a| a.artifact_ref == f.artifact_ref)
            .ok_or_else(|| error("artifact-not-found"))?;
        crate::verification::verify(&p.layout.join(&f.path), &a.definition, &control)?;
    }
    for a in &p.distribution.aliases {
        if fs::read_link(p.layout.join(&a.path)).map_err(|_| error("process-layout-invalid"))?
            != Path::new(&a.target)
        {
            return Err(error("process-layout-invalid"));
        }
    }
    Ok(())
}
struct Cleanup(PathBuf);
pub(super) fn reclaim(root: &Path, reference: &str) -> Result<(), LocalInferenceError> {
    if !super::digest(reference) {
        return Err(error("process-layout-invalid"));
    }
    let layout = root.join("process-materialization").join(reference);
    match fs::symlink_metadata(&layout) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(m) if m.file_type().is_dir() => {
            drop(Cleanup(layout.clone()));
            if layout.exists() {
                Err(error("process-cleanup-failed"))
            } else {
                Ok(())
            }
        }
        _ => Err(error("process-layout-invalid")),
    }
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        // Only this fresh incarnation's exact flat directory. Never recursive owner deletion.
        if let Ok(entries) = fs::read_dir(&self.0) {
            for entry in entries.flatten() {
                let _ = fs::remove_file(entry.path());
            }
        }
        let _ = fs::remove_dir(&self.0);
    }
}
struct OwnedChild(Option<Child>);
/// Private per-incarnation transport capability, never serialized or published.
#[derive(Clone)]
pub(super) struct Endpoint {
    port: u16,
    key: String,
}
#[cfg(test)]
impl Endpoint {
    pub(super) fn specimen(port: u16) -> Self {
        Self {
            port,
            key: "synthetic".into(),
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ =
                crowsi_transport_foundation::process::terminate(child, Duration::from_millis(100));
        }
    }
}
pub(super) fn run(
    root: &Path,
    value: &Arc<Mutex<ProcessSnapshot>>,
    cancel: &Arc<AtomicBool>,
    endpoint: &Arc<Mutex<Option<Endpoint>>>,
) -> Result<(), LocalInferenceError> {
    let initial = snapshot(value)?;
    let control = VerificationControl::with_cancellation(Duration::from_secs(30), cancel.clone());
    let p = match prepare(root, &initial.runtime_ref, &initial.process_ref, &control) {
        Err(e) if e.code() == "cancelled" => return Ok(()),
        value => value?,
    };
    if cancel.load(Ordering::Acquire) {
        return Ok(());
    }
    let cleanup = Cleanup(p.layout.clone());
    materialize(root, &p)?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(|_| error("process-endpoint-unavailable"))?;
    let port = listener
        .local_addr()
        .map_err(|_| error("process-endpoint-unavailable"))?
        .port();
    let key = super::service::nonce()?;
    let own_exe = std::env::current_exe().map_err(|_| error("process-owner-unavailable"))?;
    // Child revalidates bytes/layout after fork and before exec. No inherited ambient env.
    let mut command = Command::new(own_exe);
    let (mut diagnostic, stderr) =
        Capture::open().map_err(|_| error("process-observation-unavailable"))?;
    command.args([
        "runtime-child",
        root.to_str().ok_or_else(|| error("path-invalid"))?,
        &initial.runtime_ref,
        &initial.process_ref,
        &std::process::id().to_string(),
        &port.to_string(),
        &key,
    ]);
    command
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .current_dir(&p.layout)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .process_group(0);
    drop(listener); // Failure to bind is child failure; socket ownership is verified before any HTTP.
    let child = command.spawn().map_err(|_| error("process-spawn-failed"))?;
    drop(command); // Close the parent's writer; EOF must reflect only the child.
    let mut child = OwnedChild(Some(child));
    let owned = child
        .0
        .as_mut()
        .ok_or_else(|| error("process-state-unavailable"))?;
    *endpoint
        .lock()
        .map_err(|_| error("process-state-unavailable"))? = Some(Endpoint {
        port,
        key: key.clone(),
    });
    let result = monitor(owned, &p, port, &key, value, cancel, &mut diagnostic);
    *endpoint
        .lock()
        .map_err(|_| error("process-state-unavailable"))? = None;
    crowsi_transport_foundation::process::terminate(owned, Duration::from_millis(100))
        .map_err(|_| error("process-cleanup-failed"))?;
    // wait() returns the cached status after Crowsi has killed the group and
    // reaped its leader; never reap earlier and risk a reused process-group ID.
    let status = owned
        .wait()
        .map_err(|_| error("process-observation-unavailable"))?;
    value
        .lock()
        .map_err(|_| error("process-state-unavailable"))?
        .termination = Some(diagnostic.finish(status));
    child.0 = None;
    drop(cleanup);
    if p.layout.exists() {
        return Err(error("process-cleanup-failed"));
    }
    result
}
fn bounded(path: &Path) -> Result<String, LocalInferenceError> {
    let mut text = String::new();
    File::open(path)
        .and_then(|f| f.take(256 * 1024 + 1).read_to_string(&mut text))
        .map_err(|_| error("process-observation-unavailable"))?;
    if text.len() > 256 * 1024 {
        return Err(error("capacity-exceeded"));
    }
    Ok(text)
}
fn owns_port(pid: u32, port: u16) -> Result<bool, LocalInferenceError> {
    let text = bounded(Path::new("/proc/net/tcp"))?;
    let target = format!("0100007F:{port:04X}");
    let inodes: BTreeSet<_> = text
        .lines()
        .skip(1)
        .filter_map(|l| {
            let c: Vec<_> = l.split_whitespace().collect();
            (c.get(1) == Some(&target.as_str()) && c.get(3) == Some(&"0A"))
                .then(|| c.get(9).copied())
                .flatten()
        })
        .collect();
    if inodes.is_empty() {
        return Ok(false);
    }
    let fds = fs::read_dir(format!("/proc/{pid}/fd"))
        .map_err(|_| error("process-observation-unavailable"))?;
    for fd in fds.take(256).flatten() {
        if let Ok(link) = fs::read_link(fd.path()) {
            let link = link.to_string_lossy();
            if inodes.iter().any(|i| link == format!("socket:[{i}]")) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
fn http(port: u16, key: &str, route: &str) -> Result<serde_json::Value, LocalInferenceError> {
    let address: SocketAddr = ([127, 0, 0, 1], port).into();
    let request = format!(
        "GET {route} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {key}\r\nConnection: close\r\n\r\n"
    );
    let bytes = crowsi_transport_foundation::process::tcp_exchange(
        address,
        request.as_bytes(),
        65536,
        Duration::from_millis(200),
    )
    .map_err(|_| error("process-not-ready"))?;
    let response = std::str::from_utf8(&bytes).map_err(|_| error("process-not-ready"))?;
    if response.len() > 65536 || !response.starts_with("HTTP/1.1 200 ") {
        return Err(error("process-not-ready"));
    }
    let body = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| error("process-not-ready"))?
        .1;
    serde_json::from_str(body).map_err(|_| error("process-not-ready"))
}
fn loaded(
    pid: u32,
    p: &Prepared,
    value: &Arc<Mutex<ProcessSnapshot>>,
) -> Result<Vec<String>, LocalInferenceError> {
    let maps = bounded(Path::new(&format!("/proc/{pid}/maps")))?;
    let mut refs = BTreeSet::new();
    let observed: BTreeSet<_> = maps
        .lines()
        .filter_map(|line| {
            if !file_execution(line) {
                return None;
            }
            let path = Path::new(line.split_whitespace().nth(5)?);
            let name = path.file_name()?.to_str()?;
            (path.parent() != Some(&p.layout)).then(|| name.to_owned())
        })
        .collect();
    value
        .lock()
        .map_err(|_| error("process-state-unavailable"))?
        .observed_host_libraries = observed.into_iter().collect();
    let started = maps.contains(
        p.layout
            .join(&p.distribution.executable)
            .to_str()
            .ok_or_else(|| error("path-invalid"))?,
    );
    let hosts: BTreeSet<_> = p
        .distribution
        .host
        .libraries
        .iter()
        .map(|name| {
            let dir = match p.distribution.host.architecture.as_str() {
                "x86_64" => "/lib/x86_64-linux-gnu",
                _ => "/lib/aarch64-linux-gnu",
            };
            fs::canonicalize(Path::new(dir).join(name)).map_err(|_| error("host-incompatible"))
        })
        .collect::<Result<_, _>>()?;
    for line in maps.lines() {
        let Some(path) = line.split_whitespace().nth(5) else {
            continue;
        };
        let path = Path::new(path);
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.parent() == Some(&p.layout) {
            let f = p
                .distribution
                .files
                .iter()
                .find(|f| f.path == name)
                .ok_or_else(|| error("process-undeclared-native-module"))?;
            refs.insert(f.artifact_ref.clone());
        } else if name.starts_with("libggml") || name.starts_with("libllama") {
            return Err(error("process-undeclared-native-module"));
        } else if started && file_execution(line) && !hosts.contains(path) {
            return Err(error("process-undeclared-host-library"));
        }
    }
    Ok(refs.into_iter().collect())
}
fn file_execution(line: &str) -> bool {
    let fields: Vec<_> = line.split_whitespace().take(6).collect();
    fields.get(1).is_some_and(|p| p.contains('x'))
        && fields.get(5).is_some_and(|p| Path::new(p).is_absolute())
}
fn monitor(
    child: &Child,
    p: &Prepared,
    port: u16,
    key: &str,
    value: &Arc<Mutex<ProcessSnapshot>>,
    cancel: &AtomicBool,
    diagnostic: &mut Capture,
) -> Result<(), LocalInferenceError> {
    let started = Instant::now();
    let pid = child.id();
    loop {
        diagnostic.poll();
        if cancel.load(Ordering::Acquire) {
            if let Ok(mut v) = value.lock() {
                v.state = ProcessState::Stopping;
            }
            return Ok(());
        }
        check_names(p)?;
        let status = bounded(Path::new(&format!("/proc/{pid}/status")))?;
        if status
            .lines()
            .any(|l| l.starts_with("State:") && (l.contains('Z') || l.contains('X')))
        {
            return Err(error("process-child-exited"));
        }
        let metric = |key: &str| {
            status
                .lines()
                .find_map(|l| {
                    l.strip_prefix(key)?
                        .split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()
                })
                .unwrap_or(0)
        };
        let rss = metric("VmRSS:") * 1024;
        let threads = metric("Threads:");
        if rss > p.runtime.definition.runtime_configuration.memory_bytes {
            return Err(error("process-memory-exceeded"));
        }
        let refs = loaded(pid, p, value)?;
        let mut v = value
            .lock()
            .map_err(|_| error("process-state-unavailable"))?;
        v.rss_bytes = rss;
        v.peak_rss_bytes = v.peak_rss_bytes.max(rss);
        v.threads = u32::try_from(threads).map_err(|_| error("capacity-exceeded"))?;
        v.loaded_artifact_refs = refs;
        if v.state == ProcessState::Loading
            && owns_port(pid, port)?
            && http(port, key, "/health").is_ok()
            && let Ok(props) = http(port, key, "/props")
        {
            let aliases = props.get("model_alias").and_then(serde_json::Value::as_str);
            if aliases == Some(&v.process_ref)
                && v.loaded_artifact_refs.len() == p.distribution.files.len()
            {
                v.state = ProcessState::Ready;
            }
        }
        if v.state == ProcessState::Loading && started.elapsed() > Duration::from_secs(90) {
            return Err(error("process-load-timeout"));
        }
        drop(v);
        std::thread::sleep(Duration::from_millis(100));
    }
}
/// Internal child entry. Parent guard precedes registry/loader work; not a public Start.
/// # Errors
/// Missing owner, changed bytes/layout, bad endpoint or exec failure rejects.
pub fn child(arguments: &[String]) -> Result<(), LocalInferenceError> {
    if arguments.len() != 6 {
        return Err(error("invalid-configuration"));
    }
    let parent = arguments[3]
        .parse()
        .map_err(|_| error("invalid-configuration"))?;
    crowsi_transport_foundation::process::bind_parent(parent)
        .map_err(|_| error("process-parent-gone"))?;
    if !super::digest(&arguments[1])
        || !super::digest(&arguments[2])
        || !super::digest(&arguments[5])
    {
        return Err(error("invalid-configuration"));
    }
    let port = arguments[4]
        .parse::<u16>()
        .map_err(|_| error("invalid-configuration"))?;
    if port == 0 {
        return Err(error("invalid-configuration"));
    }
    let p = prepare(
        Path::new(&arguments[0]),
        &arguments[1],
        &arguments[2],
        &VerificationControl::new(Duration::from_secs(30)),
    )?;
    check_layout(&p)?;
    let r = &p.runtime.definition.runtime_configuration;
    let m = &p.runtime.definition.model_configuration;
    let mut command = Command::new(p.layout.join(&p.distribution.executable));
    command
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("OMP_NUM_THREADS", r.threads.to_string())
        .current_dir(&p.layout);
    command.arg("--model").arg(&p.model).args([
        "--alias",
        &arguments[2],
        "--host",
        "127.0.0.1",
        "--port",
        &arguments[4],
        "--api-key",
        &arguments[5],
        "--threads",
        &r.threads.to_string(),
        "--threads-batch",
        &r.threads.to_string(),
        "--threads-http",
        "1",
        "--parallel",
        "1",
        "--ctx-size",
        &m.context_tokens.to_string(),
        "--batch-size",
        &m.batch_tokens.to_string(),
        "--n-gpu-layers",
        "0",
        "--no-webui",
    ]);
    let _ = command.exec();
    Err(error("process-exec-failed"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn loader_cache_is_data_not_an_executable_and_names_do_not_grant_trust() {
        assert!(!super::file_execution("1-2 r--p 0 0:0 1 /etc/ld.so.cache"));
        assert!(super::file_execution(
            "1-2 r-xp 0 0:0 1 /tmp/arbitrary-name"
        ));
        assert!(super::file_execution("1-2 r-xp 0 0:0 1 /lib/libc.so.6"));
        assert!(!super::file_execution("1-2 r-xp 0 0:0 0 [vdso]"));
    }
}
