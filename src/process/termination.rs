//! Bounded native-child diagnostics. Never execution or retry authority.
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read},
    os::{
        fd::OwnedFd,
        unix::{net::UnixStream, process::ExitStatusExt},
    },
    process::{ExitStatus, Stdio},
};

/// Observed only after the existing Crowsi process-group cleanup has reaped the
/// child. Missing diagnostics are unknown, never evidence of successful work.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessTermination {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    /// An exact bounded CLI error envelope, not free-form native stderr.
    /// This report never changes request outcome, retry policy or readiness.
    pub reported_error_code: Option<String>,
}

const LIMIT: usize = 4096;
pub(super) struct Capture {
    input: UnixStream,
    bytes: [u8; LIMIT],
    used: usize,
    discarded: bool,
    closed: bool,
}
impl Capture {
    pub(super) fn open() -> io::Result<(Self, Stdio)> {
        let (input, output) = UnixStream::pair()?;
        input.set_nonblocking(true)?;
        Ok((
            Self {
                input,
                bytes: [0; LIMIT],
                used: 0,
                discarded: false,
                closed: false,
            },
            Stdio::from(OwnedFd::from(output)),
        ))
    }
    /// At most 64 KiB per existing monitor pass. No thread, unbounded collector,
    /// blocking read, or retained log. Overflow is drained and never parsed.
    pub(super) fn poll(&mut self) {
        if self.closed {
            return;
        }
        let mut buffer = [0u8; LIMIT];
        for _ in 0..16 {
            match self.input.read(&mut buffer) {
                Ok(0) => {
                    self.closed = true;
                    break;
                }
                Ok(count) => {
                    let retained = count.min(LIMIT - self.used);
                    self.bytes[self.used..self.used + retained]
                        .copy_from_slice(&buffer[..retained]);
                    self.used += retained;
                    self.discarded |= retained != count;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => {
                    self.discarded = true;
                    self.closed = true;
                    break;
                }
            }
        }
    }
    pub(super) fn finish(mut self, status: ExitStatus) -> ProcessTermination {
        self.poll();
        ProcessTermination {
            exit_code: status.code(),
            signal: status.signal(),
            reported_error_code: if self.discarded || !self.closed {
                None
            } else {
                reported_code(&self.bytes[..self.used])
            },
        }
    }
}
fn reported_code(bytes: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Envelope {
        schema: String,
        status: String,
        reason_code: String,
    }
    let value: Envelope = serde_json::from_slice(bytes).ok()?;
    (value.schema == "zixcel://local-inference/cli-error/v2"
        && value.status == "error"
        && !value.reason_code.is_empty()
        && value.reason_code.len() <= 96
        && value
            .reason_code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
    .then_some(value.reason_code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::{io::Write, process::Command, time::Duration};

    #[test]
    fn bounded_diagnostics_and_reaped_exit_remain_observations()
    -> Result<(), Box<dyn std::error::Error>> {
        let envelope = br#"{"schema":"zixcel://local-inference/cli-error/v2","status":"error","reasonCode":"timeout"}"#;
        assert_eq!(reported_code(envelope).as_deref(), Some("timeout"));
        for bytes in [b"/private/path: token=secret".as_slice(), br#"{"schema":"other","status":"error","reasonCode":"timeout"}"#, br#"{"schema":"zixcel://local-inference/cli-error/v2","status":"error","reasonCode":"/private/secret"}"#, br#"{"schema":"zixcel://local-inference/cli-error/v2","status":"error","reasonCode":"timeout","message":"secret"}"#] {
            assert!(reported_code(bytes).is_none());
        }
        let (mut capture, output) = Capture::open()?;
        let mut child = Command::new("sh").args(["-c", "printf '%s' '{\"schema\":\"zixcel://local-inference/cli-error/v2\",\"status\":\"error\",\"reasonCode\":\"timeout\"}' >&2; exit 2"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(output).process_group(0).spawn()?;
        let limit = std::time::Instant::now() + Duration::from_secs(2);
        while !capture.closed && std::time::Instant::now() < limit {
            capture.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
        // Never reap the leader before the process-group owner finishes cleanup.
        crowsi_transport_foundation::process::terminate(&mut child, Duration::from_millis(100))?;
        let result = capture.finish(child.wait()?);
        assert_eq!(result.exit_code, Some(2));
        assert_eq!(result.signal, None);
        assert_eq!(result.reported_error_code.as_deref(), Some("timeout"));

        let (input, mut output) = UnixStream::pair()?;
        input.set_nonblocking(true)?;
        let mut capture = Capture {
            input,
            bytes: [0; LIMIT],
            used: 0,
            discarded: false,
            closed: false,
        };
        capture.poll(); // Empty live stream must return immediately.
        for _ in 0..20 {
            output.write_all(&[b'x'; LIMIT])?;
            capture.poll();
        }
        drop(output);
        capture.poll();
        assert_eq!(capture.used, LIMIT);
        assert!(capture.closed && capture.discarded);
        let result = capture.finish(ExitStatus::from_raw(9));
        assert_eq!(result.exit_code, None);
        assert_eq!(result.signal, Some(9));
        assert_eq!(result.reported_error_code, None);
        Ok(())
    }
}
