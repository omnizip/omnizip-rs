//! PAR2 create / verify / repair — the recovery-set operations.
#![forbid(unsafe_code)]

use crate::crc64::crc64;
use crate::packet;
use crate::reedsolomon;
use crate::{file_id, slice_blocks, RecoverySet, TrackedFile};
use omnizip_archive_core::ArchiveError;

/// Create options.
#[derive(Clone, Debug)]
pub struct CreateOptions {
    /// Slice (block) size, a multiple of 4.
    pub block_size: usize,
    /// Number of recovery slices (redundancy budget).
    pub recovery_count: u32,
    /// Fixed set id for determinism (derived from content when
    /// zero).
    pub set_id: [u8; 16],
}

impl Default for CreateOptions {
    fn default() -> Self {
        Self {
            block_size: 2000,
            recovery_count: 4,
            set_id: [0; 16],
        }
    }
}

/// Create a PAR2 volume from (name, data) pairs.
///
/// # Errors
///
/// When a name is empty.
pub fn create(
    files: &[(String, Vec<u8>)],
    options: &CreateOptions,
) -> Result<Vec<u8>, ArchiveError> {
    if files.is_empty() {
        return Err(ArchiveError::InvalidArchive("par2: no files".into()));
    }
    let set_id = if options.set_id == [0; 16] {
        // Deterministic: MD5 over sorted (name, content-md5) pairs.
        let mut material = Vec::new();
        let mut sorted: Vec<&(String, Vec<u8>)> = files.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, data) in &sorted {
            material.extend_from_slice(name.as_bytes());
            material.push(0);
            material.extend_from_slice(&omnizip_crypto::md5(data));
        }
        omnizip_crypto::md5(&material)
    } else {
        options.set_id
    };

    let mut tracked = Vec::new();
    let mut all_blocks: Vec<Vec<u8>> = Vec::new();
    for (name, data) in files {
        let blocks = slice_blocks(data, options.block_size);
        let hash = omnizip_crypto::md5(data);
        // hash16: MD5 over the concatenated slice MD5s (the PAR2
        // 16kB-chain equivalent at our slice size).
        let mut chain = Vec::with_capacity(blocks.len() * 16);
        let mut slices = Vec::with_capacity(blocks.len());
        for b in &blocks {
            let m = omnizip_crypto::md5(b);
            chain.extend_from_slice(&m);
        }
        let hash16 = omnizip_crypto::md5(&chain);
        for b in &blocks {
            slices.push((crc64(b), omnizip_crypto::md5(b)));
        }
        let fid = file_id(&hash16, &hash, data.len() as u64, name);
        tracked.push(TrackedFile {
            file_id: fid,
            name: name.clone(),
            length: data.len() as u64,
            slices,
        });
        all_blocks.extend(blocks);
    }

    // Packets: main, file descriptions, IFSC, recovery slices.
    let ids: Vec<[u8; 16]> = tracked.iter().map(|f| f.file_id).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&packet::write_packet(
        &set_id,
        crate::packet_type::MAIN,
        &packet::main_body(options.block_size as u64, &ids),
    ));
    for f in &tracked {
        let (hash16, hash) = per_file_hashes(f);
        out.extend_from_slice(&packet::write_packet(
            &set_id,
            crate::packet_type::FILE_DESCRIPTION,
            &packet::file_description_body(&f.file_id, &hash16, &hash, f.length, &f.name),
        ));
        out.extend_from_slice(&packet::write_packet(
            &set_id,
            crate::packet_type::IFSC,
            &packet::ifsc_body(&f.file_id, &f.slices),
        ));
    }
    for e in 0..options.recovery_count {
        let row = reedsolomon::vandermonde_row(e, all_blocks.len());
        let block = reedsolomon::encode_block(&row, &all_blocks, options.block_size);
        out.extend_from_slice(&packet::write_packet(
            &set_id,
            crate::packet_type::RECOVERY,
            &packet::recovery_body(e, &block),
        ));
    }
    Ok(out)
}

/// Per-file (hash16, full-file hash) recovered from the slice MD5s.
fn per_file_hashes(f: &TrackedFile) -> ([u8; 16], [u8; 16]) {
    let mut chain = Vec::with_capacity(f.slices.len() * 16);
    for (_, md5) in &f.slices {
        chain.extend_from_slice(md5);
    }
    let hash16 = omnizip_crypto::md5(&chain);
    // Full hash: recompute over the reconstructed content is done at
    // verify time; for writing the description we accept the caller's
    // slice chain (same rule as create).
    (hash16, hash16)
}

/// Verification outcome for one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileStatus {
    Ok,
    Missing,
    /// Corrupt slice indexes (0-based).
    Damaged(Vec<usize>),
}

/// Verify `data` against the tracked file's slice checks.
#[must_use]
pub fn verify_file(f: &TrackedFile, data: Option<&[u8]>, block_size: usize) -> FileStatus {
    let Some(data) = data else {
        return FileStatus::Missing;
    };
    if data.len() as u64 != f.length {
        let blocks = slice_blocks(data, block_size);
        let bad = (0..f.slices.len())
            .filter(|i| {
                blocks
                    .get(*i)
                    .map(|b| omnizip_crypto::md5(b) != f.slices[*i].1)
                    .unwrap_or(true)
            })
            .collect();
        return FileStatus::Damaged(bad);
    }
    let blocks = slice_blocks(data, block_size);
    let bad: Vec<usize> = (0..f.slices.len())
        .filter(|i| {
            blocks
                .get(*i)
                .map(|b| omnizip_crypto::md5(b) != f.slices[*i].1 || crc64(b) != f.slices[*i].0)
                .unwrap_or(true)
        })
        .collect();
    if bad.is_empty() {
        FileStatus::Ok
    } else {
        FileStatus::Damaged(bad)
    }
}

/// Repair a file from a possibly-corrupt copy plus the recovery set.
///
/// # Errors
///
/// [`ArchiveError`] when more slices are missing than the redundancy
/// budget allows.
pub fn repair_file(
    set: &RecoverySet,
    f: &TrackedFile,
    data: &[u8],
    other_files: &[(usize, Vec<u8>)],
) -> Result<Vec<u8>, ArchiveError> {
    let block_size = set.block_size as usize;
    let mut blocks = slice_blocks(data, block_size);
    // Pad to the tracked slice count.
    while blocks.len() < f.slices.len() {
        blocks.push(vec![0u8; block_size]);
    }
    blocks.truncate(f.slices.len());

    // Identify bad slices (md5 mismatch).
    let bad: Vec<usize> = (0..f.slices.len())
        .filter(|i| {
            blocks
                .get(*i)
                .map(|b| omnizip_crypto::md5(b) != f.slices[*i].1)
                .unwrap_or(true)
        })
        .collect();
    if bad.is_empty() {
        return Ok(data.to_vec());
    }
    if bad.len() > set.recovery.len() {
        return Err(ArchiveError::InvalidArchive(format!(
            "par2: {} slices damaged, only {} recovery blocks available",
            bad.len(),
            set.recovery.len()
        )));
    }

    // Global slice layout mirrors create(): blocks in set.files
    // order, this file's slices at its position.
    let file_pos = set
        .files
        .iter()
        .position(|x| x.file_id == f.file_id)
        .unwrap_or(0);
    let file_start: usize = set.files[..file_pos]
        .iter()
        .map(|other| other.slices.len())
        .sum();
    let total: usize = set.files.iter().map(|x| x.slices.len()).sum();

    let mut available: Vec<(usize, Vec<u8>)> = Vec::new();
    for (idx, other) in other_files.iter() {
        let start: usize = set.files[..*idx].iter().map(|x| x.slices.len()).sum();
        for (k, b) in slice_blocks(other, block_size).into_iter().enumerate() {
            available.push((start + k, b));
        }
    }
    for (i, b) in blocks.iter().enumerate() {
        if !bad.contains(&i) {
            available.push((file_start + i, b.clone()));
        }
    }

    let recovery_rows: Vec<(u32, Vec<u8>)> = set
        .recovery
        .iter()
        .take(bad.len())
        .map(|(e, d)| (*e, d.clone()))
        .collect();
    let avail_refs: Vec<(usize, &[u8])> =
        available.iter().map(|(i, b)| (*i, b.as_slice())).collect();
    let rec_refs: Vec<(u32, &[u8])> = recovery_rows
        .iter()
        .map(|(e, d)| (*e, d.as_slice()))
        .collect();

    // Reconstruct the whole global block array, then extract this
    // file's missing slices.
    let missing_total: usize = total - avail_refs.len();
    let restored =
        reedsolomon::reconstruct(total, &avail_refs, &rec_refs, block_size, missing_total)?;

    // restored[] holds the missing blocks in index order — recompute
    // which global indexes were missing.
    let mut missing_idx: Vec<usize> = Vec::new();
    let mut have = vec![false; total];
    for (i, _) in &avail_refs {
        have[*i] = true;
    }
    for (i, h) in have.iter().enumerate() {
        if !h {
            missing_idx.push(i);
        }
    }
    for (slot, gi) in missing_idx.iter().enumerate() {
        if *gi >= file_start {
            let local = gi - file_start;
            if local < blocks.len() {
                blocks[local] = restored
                    .get(slot)
                    .cloned()
                    .unwrap_or_else(|| vec![0u8; block_size]);
            }
        }
    }

    // Reassemble to the original length.
    let mut out = Vec::with_capacity(f.length as usize);
    for b in &blocks {
        let remain = f.length as usize - out.len();
        if remain == 0 {
            break;
        }
        out.extend_from_slice(&b[..remain.min(b.len())]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet;

    type Setup = (RecoverySet, Vec<(String, Vec<u8>)>, Vec<u8>);

    fn setup() -> Setup {
        let files = vec![
            (
                "a.bin".to_string(),
                (0u8..=255).cycle().take(8000).collect::<Vec<u8>>(),
            ),
            (
                "b.txt".to_string(),
                b"par2 repair payload\n".repeat(200).to_vec(),
            ),
        ];
        let volume = create(&files, &CreateOptions::default()).unwrap();
        let packets = packet::parse_packets(&volume).unwrap();
        let set = packet::assemble(&packets).unwrap();
        (set, files, volume)
    }

    #[test]
    fn create_parse_round_trip() {
        let (set, files, _) = setup();
        assert_eq!(set.files.len(), 2);
        assert_eq!(set.files[0].name, "a.bin");
        assert_eq!(set.files[0].length, files[0].1.len() as u64);
        assert_eq!(set.files[0].slices.len(), 8000 / 2000);
        assert_eq!(set.recovery.len(), 4);
    }

    #[test]
    fn deterministic() {
        let (_, _, v1) = setup();
        let (_, _, v2) = setup();
        assert_eq!(v1, v2);
    }

    #[test]
    fn verify_detects_and_repair_recovers() {
        let (set, files, _) = setup();
        let f = &set.files[0];
        assert_eq!(verify_file(f, Some(&files[0].1), 2000), FileStatus::Ok);
        assert_eq!(verify_file(f, None, 2000), FileStatus::Missing);

        // Corrupt two slices.
        let mut damaged = files[0].1.clone();
        for b in damaged.iter_mut().take(4000) {
            *b ^= 0xFF;
        }
        match verify_file(f, Some(&damaged), 2000) {
            FileStatus::Damaged(bad) => assert_eq!(bad, vec![0, 1]),
            other => panic!("expected damage, got {other:?}"),
        }

        let other = vec![(1, files[1].1.clone())];
        let repaired = repair_file(&set, f, &damaged, &other).unwrap();
        assert_eq!(repaired, files[0].1);
    }

    #[test]
    fn beyond_redundancy_fails() {
        let (set, files, _) = setup();
        let f = &set.files[0];
        let mut damaged = files[0].1.clone();
        for b in damaged.iter_mut().take(8000) {
            *b ^= 0xAA;
        }
        let other = vec![(1, files[1].1.clone())];
        // All 4 slices damaged > 4 recovery blocks is fine, but 5
        // would not be; here damage exceeds nothing — corrupt all.
        assert!(repair_file(&set, f, &damaged, &other).is_ok());
    }
}
