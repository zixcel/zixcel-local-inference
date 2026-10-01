// Authored format fixtures, not downloaded weights or a production inference engine.
pub fn gguf(value: f32) -> Vec<u8> {
    let mut data = b"GGUF".to_vec();
    data.extend(3u32.to_le_bytes());
    data.extend(1u64.to_le_bytes());
    data.extend(1u64.to_le_bytes());
    data.extend(20u64.to_le_bytes());
    data.extend(b"general.architecture");
    data.extend(8u32.to_le_bytes());
    data.extend(12u64.to_le_bytes());
    data.extend(b"test-fixture");
    data.extend(6u64.to_le_bytes());
    data.extend(b"weight");
    data.extend(1u32.to_le_bytes());
    data.extend(1u64.to_le_bytes());
    data.extend(0u32.to_le_bytes());
    data.extend(0u64.to_le_bytes());
    data.resize(data.len().div_ceil(32) * 32, 0);
    data.extend(value.to_le_bytes());
    data
}

pub fn static_elf() -> Vec<u8> {
    // ELF64 x86-64 single PT_LOAD exit(0). No interpreter or shared-library dependency.
    let mut bytes = vec![0; 132];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
    bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
    bytes[24..32].copy_from_slice(&0x0040_0078_u64.to_le_bytes());
    bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
    bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
    bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
    bytes[64..68].copy_from_slice(&1u32.to_le_bytes());
    bytes[68..72].copy_from_slice(&5u32.to_le_bytes());
    bytes[80..88].copy_from_slice(&0x0040_0000_u64.to_le_bytes());
    bytes[96..104].copy_from_slice(&132u64.to_le_bytes());
    bytes[104..112].copy_from_slice(&132u64.to_le_bytes());
    bytes[112..120].copy_from_slice(&4096u64.to_le_bytes());
    bytes[120..132].copy_from_slice(&[0xb8, 60, 0, 0, 0, 0xbf, 0, 0, 0, 0, 0x0f, 0x05]);
    bytes
}
