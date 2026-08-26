//! ZIP64 acceptance (TODO.containers task 04): the >65,535-entry
//! archive exercises the zip64 EOCD (entry-count overflow) and our
//! reader's zip64 central-directory parsing end to end.
use omnizip_archive_core::write_options::WriteOptions;
use omnizip_archive_core::{ArchiveReader, ArchiveWriter, NewEntry};
use omnizip_zip::{ZipMethod, ZipReader, ZipWriter};

#[test]
fn sixty_five_thousand_plus_entries_round_trip() {
    let opts = WriteOptions::deterministic();
    let mut w = ZipWriter::new().with_method(ZipMethod::Store);
    const COUNT: usize = 66_000;
    for i in 0..COUNT {
        let name = format!("f/{i:06}.bin");
        w.add_file(&NewEntry::file(name, &opts), &[i as u8; 4], &opts)
            .unwrap();
    }
    let bytes = w.finish_bytes().unwrap();

    // Deterministic: same bytes twice.
    let mut w2 = ZipWriter::new().with_method(ZipMethod::Store);
    for i in 0..COUNT {
        let name = format!("f/{i:06}.bin");
        w2.add_file(&NewEntry::file(name, &opts), &[i as u8; 4], &opts)
            .unwrap();
    }
    assert_eq!(bytes, w2.finish_bytes().unwrap());

    let mut r = ZipReader::from_bytes(&bytes).unwrap();
    let entries = r.entries().unwrap();
    assert_eq!(entries.len(), COUNT);
    // Spot-check contents across the range.
    for i in [0usize, 1, 32_767, 65_535, 65_999] {
        assert_eq!(r.read_entry(i).unwrap(), vec![i as u8; 4], "entry {i}");
    }
    assert_eq!(entries[65_535].name, "f/065535.bin");

    // The EOCD must be the zip64 form: classic EOCD cannot represent
    // 66,000 entries. Locate both records and assert the classic
    // entry count is saturated and a zip64 EOCD exists.
    let eocd = find_last(&bytes, &[0x50, 0x4B, 0x05, 0x06]).expect("EOCD");
    let total = u16::from_le_bytes([bytes[eocd + 10], bytes[eocd + 11]]);
    assert_eq!(total, 0xFFFF, "classic EOCD must be saturated");
    let locator = find_last(&bytes, &[0x50, 0x4B, 0x06, 0x07]).expect("zip64 locator");
    let z64_off = u64::from_le_bytes(bytes[locator + 8..locator + 16].try_into().unwrap());
    assert_eq!(
        &bytes[z64_off as usize..z64_off as usize + 4],
        &[0x50, 0x4B, 0x06, 0x06],
        "locator must point at the zip64 EOCD"
    );
    let z64_total = u64::from_le_bytes(
        bytes[z64_off as usize + 32..z64_off as usize + 40]
            .try_into()
            .unwrap(),
    );
    assert_eq!(z64_total as usize, COUNT);
}

fn find_last(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

/// A single entry whose sizes overflow u32 is impractical to build in
/// a test (4 GiB), but the local/central zip64 extra emission is
/// driven purely by the size comparison; assert the threshold
/// arithmetic and record shape via the entry-count overflow path
/// above plus a direct construction check on the writer's private
/// path is not possible — so pin the reader side instead: a crafted
/// central record with zip64 extra parses to the 64-bit sizes.
#[test]
fn reader_parses_zip64_central_sizes() {
    // Minimal zip: one STORE entry with a zip64 extra in the central
    // directory carrying 64-bit sizes and offset.
    let mut z: Vec<u8> = Vec::new();
    let name = b"big.bin";
    // Local header (no zip64 extra; sizes saturated there).
    z.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
    z.extend_from_slice(&45u16.to_le_bytes()); // version
    z.extend_from_slice(&0u16.to_le_bytes()); // flags
    z.extend_from_slice(&0u16.to_le_bytes()); // store
    z.extend_from_slice(&0u32.to_le_bytes()); // time
    z.extend_from_slice(&0u32.to_le_bytes()); // crc (unused here)
    z.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // csize saturated
    z.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // usize saturated
    z.extend_from_slice(&(name.len() as u16).to_le_bytes());
    z.extend_from_slice(&0u16.to_le_bytes()); // no extra
    z.extend_from_slice(name);
    // No data (we only exercise header parsing).
    // Central directory with zip64 extra: usize, csize, offset.
    let mut extra: Vec<u8> = Vec::new();
    extra.extend_from_slice(&0x0001u16.to_le_bytes());
    extra.extend_from_slice(&24u16.to_le_bytes());
    extra.extend_from_slice(&5_000_000_000u64.to_le_bytes()); // usize
    extra.extend_from_slice(&5_000_000_000u64.to_le_bytes()); // csize
    extra.extend_from_slice(&0u64.to_le_bytes()); // offset
    let cd_off = z.len();
    z.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]);
    z.extend_from_slice(&((3u16 << 8) | 45u16).to_le_bytes());
    z.extend_from_slice(&45u16.to_le_bytes());
    z.extend_from_slice(&0u16.to_le_bytes()); // flags
    z.extend_from_slice(&0u16.to_le_bytes()); // store
    z.extend_from_slice(&0u32.to_le_bytes());
    z.extend_from_slice(&0u32.to_le_bytes()); // crc
    z.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    z.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    z.extend_from_slice(&(name.len() as u16).to_le_bytes());
    z.extend_from_slice(&(extra.len() as u16).to_le_bytes());
    z.extend_from_slice(&0u16.to_le_bytes()); // comment
    z.extend_from_slice(&0u16.to_le_bytes()); // disk
    z.extend_from_slice(&0u16.to_le_bytes()); // internal
    z.extend_from_slice(&0u32.to_le_bytes()); // external
    z.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // offset saturated
    z.extend_from_slice(name);
    z.extend_from_slice(&extra);
    let cd_size = z.len() - cd_off;
    // EOCD.
    z.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06]);
    z.extend_from_slice(&0u16.to_le_bytes());
    z.extend_from_slice(&0u16.to_le_bytes());
    z.extend_from_slice(&1u16.to_le_bytes());
    z.extend_from_slice(&1u16.to_le_bytes());
    z.extend_from_slice(&(cd_size as u32).to_le_bytes());
    z.extend_from_slice(&(cd_off as u32).to_le_bytes());
    z.extend_from_slice(&0u16.to_le_bytes());

    let mut r = ZipReader::from_bytes(&z).unwrap();
    let entries = r.entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "big.bin");
    assert_eq!(
        entries[0].size,
        Some(5_000_000_000),
        "64-bit size from zip64 extra"
    );
}
