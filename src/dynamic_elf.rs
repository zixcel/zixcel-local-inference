//! Non-executing ELF64 dynamic metadata inspection. Never uses ldd/dlopen.
use crate::{LocalInferenceError, VerificationControl};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DynamicMetadata {
    pub architecture: String,
    pub interpreter: Option<String>,
    pub needed: Vec<String>,
    pub soname: Option<String>,
    pub runpath: Option<String>,
    pub symbol_versions: BTreeMap<String, Vec<String>>,
}

fn error() -> LocalInferenceError {
    LocalInferenceError::new("unsupported-runtime")
}
fn number(bytes: &[u8], offset: usize) -> Result<u64, LocalInferenceError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or_else(error)?
            .try_into()
            .map_err(|_| error())?,
    ))
}
fn read(
    file: &mut File,
    offset: u64,
    length: u64,
    total: u64,
    control: &VerificationControl,
) -> Result<Vec<u8>, LocalInferenceError> {
    control.check()?;
    if length > 2 * 1024 * 1024 || offset.checked_add(length).is_none_or(|end| end > total) {
        return Err(error());
    }
    let mut bytes = vec![0; usize::try_from(length).map_err(|_| error())?];
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_exact(&mut bytes))
        .map_err(|_| error())?;
    Ok(bytes)
}
fn string(bytes: &[u8], offset: u64) -> Result<String, LocalInferenceError> {
    let start = usize::try_from(offset).map_err(|_| error())?;
    let tail = bytes.get(start..).ok_or_else(error)?;
    let end = tail
        .iter()
        .take(513)
        .position(|b| *b == 0)
        .ok_or_else(error)?;
    let value = std::str::from_utf8(&tail[..end]).map_err(|_| error())?;
    if value.is_empty() || !value.is_ascii() || value.bytes().any(|b| b.is_ascii_control()) {
        return Err(error());
    }
    Ok(value.to_owned())
}

pub(crate) fn inspect(
    file: &mut File,
    total: u64,
    control: &VerificationControl,
) -> Result<DynamicMetadata, LocalInferenceError> {
    let header = read(file, 0, 64, total, control)?;
    if &header[..7] != b"\x7fELF\x02\x01\x01"
        || header[16..18] != [3, 0]
        || header[52..54] != [64, 0]
        || header[54..56] != [56, 0]
    {
        return Err(error());
    }
    let architecture = match u16::from_le_bytes([header[18], header[19]]) {
        62 => "x86_64",
        183 => "aarch64",
        _ => return Err(error()),
    }
    .to_owned();
    let count = u16::from_le_bytes([header[56], header[57]]);
    if count == 0 || count > 128 {
        return Err(error());
    }
    let table = read(
        file,
        number(&header, 32)?,
        u64::from(count) * 56,
        total,
        control,
    )?;
    let mut loads = Vec::new();
    let mut dynamic = None;
    let mut interpreter = None;
    let mut executable_entry = false;
    let entry = number(&header, 24)?;
    for ph in table.chunks_exact(56) {
        let kind = u32::from_le_bytes(ph[..4].try_into().map_err(|_| error())?);
        let offset = number(ph, 8)?;
        let address = number(ph, 16)?;
        let size = number(ph, 32)?;
        if offset.checked_add(size).is_none_or(|end| end > total) {
            return Err(error());
        }
        match kind {
            1 => {
                if size > number(ph, 40)? {
                    return Err(error());
                }
                executable_entry |= ph[4] & 1 != 0
                    && entry >= address
                    && address.checked_add(size).is_some_and(|end| entry < end);
                loads.push((address, size, offset));
            }
            2 => {
                if dynamic.replace((offset, size)).is_some() {
                    return Err(error());
                }
            }
            3 => {
                if interpreter.is_some() || size > 512 {
                    return Err(error());
                }
                interpreter = Some(string(&read(file, offset, size, total, control)?, 0)?);
            }
            _ => (),
        }
    }
    if interpreter.is_some() && !executable_entry {
        return Err(error());
    }
    let (offset, length) = dynamic.ok_or_else(error)?;
    if length > 64 * 1024 || length % 16 != 0 {
        return Err(error());
    }
    let bytes = read(file, offset, length, total, control)?;
    let (tags, needed_offsets) = dynamic_tags(&bytes)?;
    dynamic_strings(
        file,
        total,
        control,
        StringTable {
            loads: &loads,
            tags: &tags,
        },
        needed_offsets,
        DynamicMetadata {
            architecture,
            interpreter,
            needed: Vec::new(),
            soname: None,
            runpath: None,
            symbol_versions: BTreeMap::new(),
        },
    )
}

fn dynamic_tags(bytes: &[u8]) -> Result<(BTreeMap<u64, u64>, Vec<u64>), LocalInferenceError> {
    let mut tags = BTreeMap::new();
    let mut needed_offsets = Vec::new();
    let mut terminated = false;
    for pair in bytes.chunks_exact(16) {
        let tag = number(pair, 0)?;
        let value = number(pair, 8)?;
        if tag == 0 {
            terminated = true;
            break;
        }
        // RPATH and loader auditing/filter objects are deliberately outside this profile.
        if matches!(
            tag,
            15 | 0x6fff_fefb | 0x6fff_fefc | 0x7fff_fffd | 0x7fff_ffff
        ) {
            return Err(error());
        }
        if tag == 1 {
            if needed_offsets.len() >= 64 {
                return Err(error());
            }
            needed_offsets.push(value);
        } else if matches!(tag, 5 | 10 | 14 | 29 | 0x6fff_fffe | 0x6fff_ffff)
            && tags.insert(tag, value).is_some()
        {
            return Err(error());
        }
    }
    if !terminated {
        return Err(error());
    }
    Ok((tags, needed_offsets))
}

#[derive(Clone, Copy)]
struct StringTable<'a> {
    loads: &'a [(u64, u64, u64)],
    tags: &'a BTreeMap<u64, u64>,
}

fn dynamic_strings(
    file: &mut File,
    total: u64,
    control: &VerificationControl,
    tables: StringTable<'_>,
    needed_offsets: Vec<u64>,
    mut result: DynamicMetadata,
) -> Result<DynamicMetadata, LocalInferenceError> {
    let StringTable { loads, tags } = tables;
    let address = *tags.get(&5).ok_or_else(error)?;
    let size = *tags.get(&10).ok_or_else(error)?;
    let offset = loads
        .iter()
        .find_map(|(base, length, offset)| {
            let delta = address.checked_sub(*base)?;
            (delta.checked_add(size)? <= *length)
                .then(|| offset.checked_add(delta))
                .flatten()
        })
        .ok_or_else(error)?;
    let strings = read(file, offset, size, total, control)?;
    let mut needed = needed_offsets
        .into_iter()
        .map(|offset| string(&strings, offset))
        .collect::<Result<Vec<_>, _>>()?;
    needed.sort();
    if needed.windows(2).any(|pair| pair[0] == pair[1])
        || needed.iter().any(|name| !crate::distribution::leaf(name))
    {
        return Err(error());
    }
    let soname = tags
        .get(&14)
        .map(|offset| string(&strings, *offset))
        .transpose()?;
    if soname
        .as_ref()
        .is_some_and(|name| !crate::distribution::leaf(name))
    {
        return Err(error());
    }
    let runpath = tags
        .get(&29)
        .map(|offset| string(&strings, *offset))
        .transpose()?;
    // Single-directory layout profile; no implicit external/working-directory search.
    if runpath.as_deref().is_some_and(|path| path != "$ORIGIN") {
        return Err(error());
    }
    result.needed = needed;
    result.soname = soname;
    result.runpath = runpath;
    result.symbol_versions = versions(file, total, control, &tables, &strings)?;
    Ok(result)
}

fn mapped(tables: &StringTable<'_>, address: u64, size: u64) -> Result<u64, LocalInferenceError> {
    tables
        .loads
        .iter()
        .find_map(|(base, length, offset)| {
            let delta = address.checked_sub(*base)?;
            (delta.checked_add(size)? <= *length)
                .then(|| offset.checked_add(delta))
                .flatten()
        })
        .ok_or_else(error)
}

fn versions(
    file: &mut File,
    total: u64,
    control: &VerificationControl,
    tables: &StringTable<'_>,
    strings: &[u8],
) -> Result<BTreeMap<String, Vec<String>>, LocalInferenceError> {
    let mut output = BTreeMap::new();
    let (Some(mut address), Some(&count)) = (
        tables.tags.get(&0x6fff_fffe).copied(),
        tables.tags.get(&0x6fff_ffff),
    ) else {
        if tables.tags.contains_key(&0x6fff_fffe) || tables.tags.contains_key(&0x6fff_ffff) {
            return Err(error());
        }
        return Ok(output);
    };
    if count == 0 || count > 64 {
        return Err(error());
    }
    let read32 = |bytes: &[u8], at: usize| -> Result<u64, LocalInferenceError> {
        Ok(u64::from(u32::from_le_bytes(
            bytes[at..at + 4].try_into().map_err(|_| error())?,
        )))
    };
    for index in 0..count {
        let bytes = read(file, mapped(tables, address, 16)?, 16, total, control)?;
        if bytes[..2] != [1, 0] {
            return Err(error());
        }
        let count = u16::from_le_bytes([bytes[2], bytes[3]]);
        if count == 0 || count > 256 {
            return Err(error());
        }
        let name = string(strings, read32(&bytes, 4)?)?;
        let mut auxiliary = address.checked_add(read32(&bytes, 8)?).ok_or_else(error)?;
        let mut values = Vec::new();
        for item in 0..count {
            let aux = read(file, mapped(tables, auxiliary, 16)?, 16, total, control)?;
            values.push(string(strings, read32(&aux, 8)?)?);
            let next = read32(&aux, 12)?;
            if (item + 1 == count) != (next == 0) {
                return Err(error());
            }
            auxiliary = auxiliary.checked_add(next).ok_or_else(error)?;
        }
        values.sort();
        if values.windows(2).any(|v| v[0] == v[1]) || output.insert(name, values).is_some() {
            return Err(error());
        }
        let next = read32(&bytes, 12)?;
        if (index + 1 == *tables.tags.get(&0x6fff_ffff).ok_or_else(error)?) != (next == 0) {
            return Err(error());
        }
        address = address.checked_add(next).ok_or_else(error)?;
    }
    Ok(output)
}
