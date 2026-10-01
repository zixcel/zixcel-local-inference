//! Bounded format/byte verification. File descriptors, not source filenames, pin reads.
use std::collections::BTreeSet;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::admission::{ArtifactAdmission, ArtifactFormat};
use crate::fsutil::validate_absolute;
use crate::{LocalInferenceError, VerificationControl};

const MAX_HEADER_BYTES: u64 = 2 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 64 * 1024;
// Format-profile budget, not an allocation size. Tokenizer metadata can exceed
// the generic envelope limit; every individual field and count stays bounded.
const MAX_GGUF_METADATA_BYTES: usize = 64 * 1024 * 1024;

pub(crate) struct Verified {
    file: File,
    metadata: Metadata,
    pub architecture: String,
    pub dynamic: Option<crate::DynamicMetadata>,
}

pub(crate) fn verify(
    path: &Path,
    definition: &ArtifactAdmission,
    control: &VerificationControl,
) -> Result<Verified, LocalInferenceError> {
    definition.validate()?;
    validate_absolute(path)?;
    for ancestor in path.ancestors() {
        if fs::symlink_metadata(ancestor)
            .map_err(|_| err("artifact-not-found"))?
            .file_type()
            .is_symlink()
        {
            return Err(err("unsupported-artifact"));
        }
    }
    let before = fs::symlink_metadata(path).map_err(|_| err("artifact-not-found"))?;
    if !before.file_type().is_file() || before.len() != definition.bytes {
        return Err(err("digest-mismatch"));
    }
    // Nonblocking prevents a regular-file-to-FIFO swap from blocking indefinitely.
    #[cfg(target_os = "linux")]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .custom_flags(0x800 | 0x20000)
            .open(path)
    };
    #[cfg(not(target_os = "linux"))]
    let file: std::io::Result<File> = Err(std::io::ErrorKind::Unsupported.into());
    let mut file = file.map_err(|_| err("artifact-unavailable"))?;
    let metadata = file.metadata().map_err(|_| err("artifact-unavailable"))?;
    if !same(&before, &metadata) || !metadata.file_type().is_file() {
        return Err(err("artifact-changed"));
    }
    hash_copy(&mut file, None, definition, control)?;
    file.seek(SeekFrom::Start(0))
        .map_err(|_| err("artifact-unavailable"))?;
    let mut dynamic = None;
    let architecture = match definition.format {
        ArtifactFormat::DynamicElf64 => {
            let metadata = crate::dynamic_elf::inspect(&mut file, definition.bytes, control)?;
            let architecture = metadata.architecture.clone();
            dynamic = Some(metadata);
            architecture
        }
        ArtifactFormat::GgufV3 => gguf(&mut file, definition.bytes, control)?,
        ArtifactFormat::StaticElf64 => {
            let mut bytes = Vec::new();
            (&mut file)
                .take(MAX_HEADER_BYTES)
                .read_to_end(&mut bytes)
                .map_err(|_| err("artifact-unavailable"))?;
            control.check()?;
            elf(&bytes, definition.bytes)?
        }
    };
    if !same(
        &metadata,
        &file.metadata().map_err(|_| err("artifact-unavailable"))?,
    ) || !same(
        &metadata,
        &fs::symlink_metadata(path).map_err(|_| err("artifact-changed"))?,
    ) {
        return Err(err("artifact-changed"));
    }
    Ok(Verified {
        file,
        metadata,
        architecture,
        dynamic,
    })
}

pub(crate) fn place(
    verified: &mut Verified,
    destination: &Path,
    definition: &ArtifactAdmission,
    control: &VerificationControl,
) -> Result<(), LocalInferenceError> {
    if destination
        .try_exists()
        .map_err(|_| err("artifact-unavailable"))?
    {
        return verify(destination, definition, control).map(|_| ());
    }
    let parent = destination.parent().ok_or_else(|| err("path-invalid"))?;
    // Bound abandoned preparation bytes as well as admitted metadata.
    let mut total_bytes = definition.bytes;
    for (count, item) in fs::read_dir(parent)
        .map_err(|_| err("artifact-unavailable"))?
        .enumerate()
    {
        // Reserve both the temporary and published directory entries.
        if count >= 126 {
            return Err(err("capacity-exceeded"));
        }
        let item = item.map_err(|_| err("artifact-unavailable"))?;
        let metadata =
            fs::symlink_metadata(item.path()).map_err(|_| err("artifact-unavailable"))?;
        if !metadata.file_type().is_file() {
            return Err(err("artifact-corrupt"));
        }
        total_bytes = total_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| err("capacity-exceeded"))?;
        if total_bytes > 16 * 1024 * 1024 * 1024 {
            return Err(err("capacity-exceeded"));
        }
    }
    let temporary = destination.with_extension(format!("partial-{}", std::process::id()));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| err("admission-conflict"))?;
    let result = (|| {
        verified
            .file
            .seek(SeekFrom::Start(0))
            .map_err(|_| err("artifact-unavailable"))?;
        hash_copy(&mut verified.file, Some(&mut output), definition, control)?;
        if !same(
            &verified.metadata,
            &verified
                .file
                .metadata()
                .map_err(|_| err("artifact-unavailable"))?,
        ) {
            return Err(err("artifact-changed"));
        }
        output.sync_all().map_err(|_| err("artifact-unavailable"))?;
        let mut permissions = output
            .metadata()
            .map_err(|_| err("artifact-unavailable"))?
            .permissions();
        permissions.set_readonly(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(match definition.format {
                ArtifactFormat::GgufV3 => 0o400,
                ArtifactFormat::StaticElf64 | ArtifactFormat::DynamicElf64 => 0o500,
            });
        }
        output
            .set_permissions(permissions)
            .map_err(|_| err("artifact-unavailable"))?;
        // No rename-overwrite: a concurrent/tampered exact object must be verified.
        match fs::hard_link(&temporary, destination) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                verify(destination, definition, control)?;
            }
            Err(_) => return Err(err("artifact-unavailable")),
        }
        File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|_| err("artifact-unavailable"))
    })();
    drop(output);
    fs::remove_file(&temporary).map_err(|_| err("artifact-cleanup-failed"))?;
    result
}

fn hash_copy(
    file: &mut File,
    mut output: Option<&mut File>,
    definition: &ArtifactAdmission,
    control: &VerificationControl,
) -> Result<(), LocalInferenceError> {
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = vec![0u8; COPY_BUFFER_BYTES].into_boxed_slice();
    loop {
        control.check()?;
        let length = file
            .read(&mut buffer)
            .map_err(|_| err("artifact-unavailable"))?;
        if length == 0 {
            break;
        }
        bytes += length as u64;
        if bytes > definition.bytes {
            return Err(err("digest-mismatch"));
        }
        hash.update(&buffer[..length]);
        if let Some(target) = output.as_deref_mut() {
            target
                .write_all(&buffer[..length])
                .map_err(|_| err("artifact-unavailable"))?;
        }
    }
    if bytes != definition.bytes || format!("{:x}", hash.finalize()) != definition.sha256 {
        return Err(err("digest-mismatch"));
    }
    Ok(())
}

#[cfg(unix)]
fn same(left: &Metadata, right: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}
#[cfg(not(unix))]
fn same(_: &Metadata, _: &Metadata) -> bool {
    false
}

struct Cursor<'a, R: Read> {
    reader: BufReader<R>,
    control: &'a VerificationControl,
    offset: usize,
}
impl<R: Read> Cursor<'_, R> {
    fn read(&mut self, target: &mut [u8]) -> Result<(), LocalInferenceError> {
        self.control.check()?;
        let end = self
            .offset
            .checked_add(target.len())
            .ok_or_else(|| err("capacity-exceeded"))?;
        if end > MAX_GGUF_METADATA_BYTES {
            return Err(err("capacity-exceeded"));
        }
        self.reader
            .read_exact(target)
            .map_err(|_| err("unsupported-artifact"))?;
        self.offset = end;
        Ok(())
    }
    fn u32(&mut self) -> Result<u32, LocalInferenceError> {
        let mut bytes = [0; 4];
        self.read(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }
    fn u64(&mut self) -> Result<u64, LocalInferenceError> {
        let mut bytes = [0; 8];
        self.read(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }
    fn string(&mut self, bound: u64) -> Result<String, LocalInferenceError> {
        let size = self.u64()?;
        if size > bound {
            return Err(err("capacity-exceeded"));
        }
        let mut bytes = vec![0; usize::try_from(size).map_err(|_| err("capacity-exceeded"))?];
        self.read(&mut bytes)?;
        String::from_utf8(bytes).map_err(|_| err("unsupported-artifact"))
    }
    fn skip_value(&mut self, kind: u32, array: bool) -> Result<(), LocalInferenceError> {
        let size = match kind {
            0 | 1 | 7 => 1,
            2 | 3 => 2,
            4..=6 => 4,
            10..=12 => 8,
            8 => {
                self.string(65_536)?;
                return Ok(());
            }
            9 if !array => {
                let element = self.u32()?;
                let count = self.u64()?;
                if count > 262_144 {
                    return Err(err("capacity-exceeded"));
                }
                for _ in 0..count {
                    self.skip_value(element, true)?;
                }
                return Ok(());
            }
            _ => return Err(err("unsupported-artifact")),
        };
        self.read(&mut [0; 8][..size])?;
        Ok(())
    }
}

// https://github.com/ggml-org/ggml/blob/master/docs/gguf.md
// Admission validates the envelope/tensor extents, not model quality or inference.
fn gguf(
    reader: impl Read,
    file_bytes: u64,
    control: &VerificationControl,
) -> Result<String, LocalInferenceError> {
    let mut cursor = Cursor {
        reader: BufReader::with_capacity(COPY_BUFFER_BYTES, reader),
        control,
        offset: 0,
    };
    let mut magic = [0; 4];
    cursor.read(&mut magic)?;
    if &magic != b"GGUF" || cursor.u32()? != 3 {
        return Err(err("unsupported-artifact"));
    }
    let tensors = cursor.u64()?;
    let metadata = cursor.u64()?;
    if tensors == 0 || tensors > 16_384 || metadata > 4096 {
        return Err(err("capacity-exceeded"));
    }
    let mut architecture = None;
    let mut alignment = 32u64;
    let mut keys = BTreeSet::new();
    for _ in 0..metadata {
        let key = cursor.string(256)?;
        if !keys.insert(key.clone()) {
            return Err(err("unsupported-artifact"));
        }
        let kind = cursor.u32()?;
        match (key.as_str(), kind) {
            ("general.architecture", 8) => architecture = Some(cursor.string(128)?),
            ("general.alignment", 4) => alignment = u64::from(cursor.u32()?),
            ("general.architecture" | "general.alignment", _) => {
                return Err(err("unsupported-artifact"));
            }
            _ => cursor.skip_value(kind, false)?,
        }
    }
    if !alignment.is_power_of_two() || alignment > 4096 {
        return Err(err("unsupported-artifact"));
    }
    let mut ranges = Vec::new();
    let mut names = BTreeSet::new();
    for _ in 0..tensors {
        let name = cursor.string(64)?;
        if name.is_empty() || !names.insert(name) {
            return Err(err("unsupported-artifact"));
        }
        let dimensions = cursor.u32()?;
        if !(1..=4).contains(&dimensions) {
            return Err(err("unsupported-artifact"));
        }
        let mut elements = 1u64;
        for _ in 0..dimensions {
            elements = elements
                .checked_mul(cursor.u64()?)
                .ok_or_else(|| err("capacity-exceeded"))?;
        }
        let (block, width) = match cursor.u32()? {
            0 => (1, 4),
            1 => (1, 2),
            2 => (32, 18),
            3 => (32, 20),
            6 => (32, 22),
            7 => (32, 24),
            8 => (32, 34),
            10 => (256, 84),
            11 => (256, 110),
            12 => (256, 144),
            13 => (256, 176),
            14 => (256, 210),
            _ => return Err(err("unsupported-artifact")),
        };
        if elements == 0 || !elements.is_multiple_of(block) {
            return Err(err("unsupported-artifact"));
        }
        let length = (elements / block)
            .checked_mul(width)
            .ok_or_else(|| err("capacity-exceeded"))?;
        let offset = cursor.u64()?;
        if offset % alignment != 0 {
            return Err(err("unsupported-artifact"));
        }
        ranges.push((
            offset,
            offset
                .checked_add(length)
                .ok_or_else(|| err("capacity-exceeded"))?,
        ));
    }
    let data_start = (cursor.offset as u64).div_ceil(alignment) * alignment;
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0)
        || ranges.iter().any(|(_, end)| {
            end.checked_add(data_start)
                .is_none_or(|value| value > file_bytes)
        })
    {
        return Err(err("unsupported-artifact"));
    }
    architecture
        .filter(|value| crate::fsutil::safe_identifier(value))
        .ok_or_else(|| err("unsupported-artifact"))
}

fn elf(bytes: &[u8], file_bytes: u64) -> Result<String, LocalInferenceError> {
    if bytes.len() < 64 || &bytes[..7] != b"\x7fELF\x02\x01\x01" || bytes[16..18] != [2, 0] {
        return Err(err("unsupported-runtime"));
    }
    let architecture = match u16::from_le_bytes([bytes[18], bytes[19]]) {
        62 => "x86_64",
        183 => "aarch64",
        _ => return Err(err("unsupported-runtime")),
    };
    let read64 = |offset| -> Result<u64, LocalInferenceError> {
        Ok(u64::from_le_bytes(
            bytes
                .get(offset..offset + 8)
                .ok_or_else(|| err("unsupported-artifact"))?
                .try_into()
                .map_err(|_| err("unsupported-artifact"))?,
        ))
    };
    let entry = read64(24)?;
    let table = read64(32)?;
    let count = u16::from_le_bytes([bytes[56], bytes[57]]);
    if bytes[52..54] != [64, 0] || bytes[54..56] != [56, 0] || count == 0 || count > 128 {
        return Err(err("unsupported-runtime"));
    }
    let mut executable = false;
    for index in 0..u64::from(count) {
        let offset = usize::try_from(
            table
                .checked_add(index * 56)
                .ok_or_else(|| err("capacity-exceeded"))?,
        )
        .map_err(|_| err("capacity-exceeded"))?;
        let header = bytes
            .get(
                offset
                    ..offset
                        .checked_add(56)
                        .ok_or_else(|| err("capacity-exceeded"))?,
            )
            .ok_or_else(|| err("unsupported-runtime"))?;
        let kind = u32::from_le_bytes(
            header[..4]
                .try_into()
                .map_err(|_| err("unsupported-runtime"))?,
        );
        if kind == 2 || kind == 3 {
            return Err(err("unsupported-runtime"));
        }
        if kind == 1 {
            let start = read64(offset + 8)?;
            let address = read64(offset + 16)?;
            let size = read64(offset + 32)?;
            let memory = read64(offset + 40)?;
            if size > memory || start.checked_add(size).is_none_or(|end| end > file_bytes) {
                return Err(err("unsupported-runtime"));
            }
            executable |= header[4] & 1 != 0
                && entry >= address
                && address.checked_add(size).is_some_and(|end| entry < end);
        }
    }
    if !executable {
        return Err(err("unsupported-runtime"));
    }
    Ok(architecture.into())
}

fn err(code: &'static str) -> LocalInferenceError {
    LocalInferenceError::new(code)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArtifactProvenance;
    use std::time::Duration;
    mod formats {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/admission_formats.rs"
        ));
    }

    #[test]
    fn gguf_large_metadata_streams_and_truncation_counts_and_cancel_reject() {
        struct BoundedRead<'a> {
            bytes: &'a [u8],
            largest: usize,
        }
        impl Read for BoundedRead<'_> {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                self.largest = self.largest.max(output.len());
                assert!(output.len() <= COPY_BUFFER_BYTES);
                self.bytes.read(output)
            }
        }
        let original = formats::gguf(1.0);
        let mut bytes = original[..76].to_vec();
        bytes[16..24].copy_from_slice(&2u64.to_le_bytes());
        bytes.extend(6u64.to_le_bytes());
        bytes.extend(b"tokens");
        bytes.extend(9u32.to_le_bytes());
        bytes.extend(8u32.to_le_bytes());
        bytes.extend(96u64.to_le_bytes());
        for _ in 0..96 {
            bytes.extend(32768u64.to_le_bytes());
            bytes.extend(vec![b'x'; 32768]);
        }
        bytes.extend(&original[76..114]);
        bytes.resize(bytes.len().div_ceil(32) * 32, 0);
        bytes.extend(1.0f32.to_le_bytes());
        let control = VerificationControl::new(Duration::from_secs(2));
        let mut reader = BoundedRead {
            bytes: &bytes,
            largest: 0,
        };
        assert_eq!(
            gguf(&mut reader, bytes.len() as u64, &control).expect("metadata >2MiB"),
            "test-fixture"
        );
        assert_eq!(reader.largest, COPY_BUFFER_BYTES);
        assert!(gguf(&bytes[..2 * 1024 * 1024], bytes.len() as u64, &control).is_err());
        bytes[16..24].copy_from_slice(&4097u64.to_le_bytes());
        assert_eq!(
            gguf(bytes.as_slice(), bytes.len() as u64, &control)
                .expect_err("bounded count")
                .code(),
            "capacity-exceeded"
        );
        let mut cursor = Cursor {
            reader: BufReader::new(&b"x"[..]),
            control: &control,
            offset: MAX_GGUF_METADATA_BYTES,
        };
        assert_eq!(
            cursor.read(&mut [0]).expect_err("cumulative budget").code(),
            "capacity-exceeded"
        );
        let expired = VerificationControl::new(Duration::ZERO);
        assert!(gguf(original.as_slice(), original.len() as u64, &expired).is_err());
    }

    #[test]
    fn changed_source_after_verification_never_publishes_replacement_bytes() {
        for original in [formats::gguf(1.0), formats::static_elf()] {
            let temporary = tempfile::tempdir().expect("isolated verification storage");
            let source = temporary.path().join("source");
            let destination = temporary.path().join("managed");
            fs::write(&source, &original).expect("authored fixture");
            let definition = ArtifactAdmission {
                sha256: format!("{:x}", Sha256::digest(&original)),
                bytes: original.len() as u64,
                format: if original.starts_with(b"GGUF") {
                    ArtifactFormat::GgufV3
                } else {
                    ArtifactFormat::StaticElf64
                },
                provenance: ArtifactProvenance {
                    publisher: "test-author".into(),
                    license: "MIT".into(),
                    source: "urn:zixcel:test:toctou".into(),
                    revision: "fixture-1".into(),
                },
            };
            let control = VerificationControl::new(Duration::from_secs(2));
            let mut verified =
                verify(&source, &definition, &control).expect("verified exact handle");
            fs::rename(&source, temporary.path().join("previous")).expect("replace source path");
            fs::write(&source, vec![0; original.len()]).expect("different bytes at same path");
            match place(&mut verified, &destination, &definition, &control) {
                Ok(()) => assert_eq!(fs::read(&destination).expect("managed bytes"), original),
                Err(error) => {
                    assert_eq!(error.code(), "artifact-changed");
                    assert!(!destination.exists());
                }
            }
            assert!(
                fs::read_dir(temporary.path())
                    .expect("no temporary leaks")
                    .all(|entry| !entry
                        .expect("entry")
                        .file_name()
                        .to_string_lossy()
                        .contains("partial"))
            );
        }
    }
}
