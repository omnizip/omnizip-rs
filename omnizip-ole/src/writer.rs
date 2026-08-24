//! CFB writer — a valid version-3 compound file: header with 109 DIFAT
//! slots, FAT sectors, directory entries sorted per the CFB name order
//! ((name length, uppercase name)) arranged as balanced sibling trees,
//! and a mini stream + mini FAT for sub-cutoff streams. Root storage
//! carries a fixed name ("Root Entry"); all names are deterministic.
#![forbid(unsafe_code)]

use crate::{fat, obj_type, DirEntry, MINI_CUTOFF, MINI_SECTOR_SIZE, SECTOR_SIZE};
use omnizip_archive_core::write_options::WriteOptions;
use omnizip_archive_core::{ArchiveError, ArchiveWriter, NewEntry};
use std::collections::BTreeMap;

/// Builds a deterministic compound file in memory.
pub struct OleWriter {
    /// path -> data (storages implied from path prefixes).
    streams: BTreeMap<String, Vec<u8>>,
}

impl OleWriter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            streams: BTreeMap::new(),
        }
    }

    /// Serialize the compound file.
    ///
    /// # Errors
    ///
    /// Never in practice (bounded names).
    pub fn finish_bytes(&mut self) -> Result<Vec<u8>, ArchiveError> {
        // ---- Build the storage tree (storages implied from paths).
        let mut storages: Vec<String> = Vec::new();
        for path in self.streams.keys() {
            let mut p = path.as_str();
            while let Some(i) = p.rfind('/') {
                p = &p[..i];
                if !storages.iter().any(|s| s == p) {
                    storages.push(p.to_string());
                }
            }
        }
        storages.sort();

        // Children per storage ("" = root).
        let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for s in &storages {
            children.insert(s.clone(), Vec::new());
        }
        let add_child =
            |path: &str, storages: &[String], children: &mut BTreeMap<String, Vec<String>>| {
                let parent = match path.rfind('/') {
                    Some(i) => path[..i].to_string(),
                    None => String::new(),
                };
                if storages.iter().any(|s| s == path) {
                    // It is a storage: its parent owns it.
                    let sp = match path.rfind('/') {
                        Some(i) => path[..i].to_string(),
                        None => String::new(),
                    };
                    children.entry(sp).or_default().push(path.to_string());
                } else {
                    children.entry(parent).or_default().push(path.to_string());
                }
            };
        for s in &storages {
            add_child(s, &storages, &mut children);
        }
        for path in self.streams.keys() {
            add_child(path, &storages, &mut children);
        }

        // ---- Layout: header | FAT sectors | directory | minifat |
        //        mini stream | regular streams.
        // Plan content sectors first to size the FAT.
        let mut regular: Vec<(String, Vec<u8>)> = Vec::new();
        let mut mini: Vec<(String, Vec<u8>)> = Vec::new();
        for (path, data) in &self.streams {
            if data.len() < MINI_CUTOFF as usize {
                mini.push((path.clone(), data.clone()));
            } else {
                regular.push((path.clone(), data.clone()));
            }
        }

        let dir_entry_count = 1 + storages.len() + self.streams.len();
        let dir_sectors = (dir_entry_count * 128).div_ceil(SECTOR_SIZE);
        let minifat_entries: usize = mini
            .iter()
            .map(|(_, d)| d.len().div_ceil(MINI_SECTOR_SIZE))
            .sum();
        let minifat_sectors = (minifat_entries * 4).div_ceil(SECTOR_SIZE);
        let mini_stream_len: usize = mini.iter().map(|(_, d)| d.len()).sum();
        let mini_stream_sectors = mini_stream_len.div_ceil(SECTOR_SIZE);
        let regular_sectors: usize = regular
            .iter()
            .map(|(_, d)| d.len().div_ceil(SECTOR_SIZE))
            .sum();

        // Iterate: guess fat_sectors, compute, repeat until stable.
        let mut fat_sectors = 1usize;
        for _ in 0..8 {
            let total_data_sectors =
                fat_sectors + dir_sectors + minifat_sectors + mini_stream_sectors + regular_sectors;
            let needed = (total_data_sectors * 4).div_ceil(SECTOR_SIZE);
            if needed == fat_sectors {
                break;
            }
            fat_sectors = needed;
        }

        // Sector numbering: [fat...] [dir...] [minifat...] [mini
        // stream...] [regular streams...]
        let mut next = 0u32;
        let fat_start = next;
        next += fat_sectors as u32;
        let dir_start = next;
        next += dir_sectors as u32;
        let minifat_start = next;
        next += minifat_sectors as u32;
        let mini_stream_start = next;
        next += mini_stream_sectors as u32;
        let _ = next;

        // ---- Assign chains.
        let mut fat = vec![fat::FREESECT; fat_sectors * (SECTOR_SIZE / 4)];
        for i in 0..fat_sectors {
            fat[fat_start as usize + i] = fat::FATSECT;
        }

        // Directory + mini-FAT chains.
        for k in 0..dir_sectors as u32 {
            let idx = (dir_start + k) as usize;
            fat[idx] = if k + 1 == dir_sectors as u32 {
                fat::ENDOFCHAIN
            } else {
                dir_start + k + 1
            };
        }
        for k in 0..minifat_sectors as u32 {
            let idx = (minifat_start + k) as usize;
            fat[idx] = if k + 1 == minifat_sectors as u32 {
                fat::ENDOFCHAIN
            } else {
                minifat_start + k + 1
            };
        }

        let mut chain_map: BTreeMap<String, (u32, usize)> = BTreeMap::new(); // path -> (start, len)
        let mut minifat = vec![fat::FREESECT; minifat_entries.max(1)];

        // Mini stream: concatenate, build minifat chains.
        let mut mini_bytes: Vec<u8> = Vec::with_capacity(mini_stream_len);
        let mut mini_sector_cursor = 0u32;
        for (path, data) in &mini {
            let start = if data.is_empty() {
                fat::ENDOFCHAIN
            } else {
                let start = mini_sector_cursor;
                let n = data.len().div_ceil(MINI_SECTOR_SIZE) as u32;
                for k in 0..n {
                    let cur = (start + k) as usize;
                    let next_mini = if k + 1 == n {
                        fat::ENDOFCHAIN
                    } else {
                        start + k + 1
                    };
                    if cur < minifat.len() {
                        minifat[cur] = next_mini;
                    }
                }
                mini_sector_cursor += n;
                mini_bytes.extend_from_slice(data);
                let pad = data.len().next_multiple_of(MINI_SECTOR_SIZE) - data.len();
                mini_bytes.resize(mini_bytes.len() + pad, 0);
                start
            };
            chain_map.insert(path.clone(), (start, data.len()));
        }

        // Mini FAT + mini stream occupy regular sectors.
        let mut next_sector = mini_stream_start;
        let assign_chain = |len: usize, fat: &mut Vec<u32>, next_sector: &mut u32| -> u32 {
            if len == 0 {
                return fat::ENDOFCHAIN;
            }
            let n = len.div_ceil(SECTOR_SIZE) as u32;
            let start = *next_sector;
            for k in 0..n {
                let idx = (start + k) as usize;
                let nxt = if k + 1 == n {
                    fat::ENDOFCHAIN
                } else {
                    start + k + 1
                };
                if idx < fat.len() {
                    fat[idx] = nxt;
                }
            }
            *next_sector += n;
            start
        };
        // Mini stream chain (owned by root).
        let mini_stream_chain_start = assign_chain(mini_stream_len, &mut fat, &mut next_sector);
        for (path, data) in &regular {
            let start = assign_chain(data.len(), &mut fat, &mut next_sector);
            chain_map.insert(path.clone(), (start, data.len()));
        }

        // ---- Directory entries: root + storages + streams, children
        //        as balanced BSTs over CFB-sorted names.
        let cfb_sort = |a: &str, b: &str| {
            let key = |s: &str| (s.len(), s.to_uppercase());
            key(a).cmp(&key(b))
        };
        let mut names: Vec<String> = storages.clone();
        names.extend(self.streams.keys().cloned());
        let mut index_of: BTreeMap<String, usize> = BTreeMap::new();
        // Entry 0: root; storages then streams in a stable order.
        let mut dir: Vec<DirEntry> = vec![DirEntry {
            name: "Root Entry".into(),
            object_type: obj_type::ROOT,
            left: fat::NOSTREAM,
            right: fat::NOSTREAM,
            child: fat::NOSTREAM,
            start_sector: mini_stream_chain_start,
            size: mini_stream_len as u64,
        }];
        for n in &names {
            let is_storage = storages.iter().any(|s| s == n);
            let (start, len) = chain_map.get(n).cloned().unwrap_or((fat::ENDOFCHAIN, 0));
            let entry = if is_storage {
                DirEntry {
                    name: base_name(n).to_string(),
                    object_type: obj_type::STORAGE,
                    left: fat::NOSTREAM,
                    right: fat::NOSTREAM,
                    child: fat::NOSTREAM,
                    start_sector: fat::ENDOFCHAIN,
                    size: 0,
                }
            } else {
                DirEntry {
                    name: base_name(n).to_string(),
                    object_type: obj_type::STREAM,
                    left: fat::NOSTREAM,
                    right: fat::NOSTREAM,
                    child: fat::NOSTREAM,
                    start_sector: start,
                    size: len as u64,
                }
            };
            index_of.insert(n.clone(), dir.len());
            dir.push(entry);
        }

        // Balanced BST per storage.
        fn build_bst(
            sorted: &[String],
            index_of: &BTreeMap<String, usize>,
            dir: &mut [DirEntry],
        ) -> u32 {
            fn rec(
                slice: &[String],
                index_of: &BTreeMap<String, usize>,
                dir: &mut [DirEntry],
            ) -> u32 {
                if slice.is_empty() {
                    return fat::NOSTREAM;
                }
                let mid = slice.len() / 2;
                let idx = index_of[&slice[mid]] as u32;
                dir[idx as usize].left = rec(&slice[..mid], index_of, dir);
                dir[idx as usize].right = rec(&slice[mid + 1..], index_of, dir);
                idx
            }
            rec(sorted, index_of, dir)
        }

        for (parent, kids) in &children {
            let mut sorted = kids.clone();
            sorted.sort_by(|a, b| cfb_sort(base_name(a), base_name(b)));
            let root_child = build_bst(&sorted, &index_of, &mut dir);
            let parent_idx = if parent.is_empty() {
                0
            } else {
                index_of[parent]
            };
            dir[parent_idx].child = root_child;
        }

        // ---- Serialize.
        let mut out: Vec<u8> = vec![0u8; 512];
        out[0..8].copy_from_slice(&crate::MAGIC);
        out[0x18] = 0x3E; // minor version
        out[0x1A] = 0x03; // major version (512-byte sectors)
        out[0x1C] = 0xFE;
        out[0x1D] = 0xFF; // little-endian
        out[0x1E] = 9; // sector shift
        out[0x20] = 6; // mini sector shift
        out[0x2C..0x30].copy_from_slice(&(fat_sectors as u32).to_le_bytes());
        out[0x30..0x34].copy_from_slice(&dir_start.to_le_bytes());
        out[0x38..0x3C].copy_from_slice(&MINI_CUTOFF.to_le_bytes());
        out[0x3C..0x40].copy_from_slice(&minifat_start.to_le_bytes());
        out[0x40..0x44].copy_from_slice(&(minifat_sectors as u32).to_le_bytes());
        // DIFAT: fat sector ids, then free.
        for (i, slot) in fat[0..fat_sectors * (SECTOR_SIZE / 4)]
            .chunks(SECTOR_SIZE / 4)
            .enumerate()
        {
            let _ = slot;
            let id = fat_start + i as u32;
            out[0x4C + i * 4..0x50 + i * 4].copy_from_slice(&id.to_le_bytes());
        }
        for i in fat_sectors..109 {
            out[0x4C + i * 4..0x50 + i * 4].copy_from_slice(&fat::FREESECT.to_le_bytes());
        }

        let pad_sector = |v: &mut Vec<u8>| {
            let rem = v.len().next_multiple_of(SECTOR_SIZE) - v.len();
            v.resize(v.len() + rem, 0);
        };

        let mut fat_bytes: Vec<u8> = Vec::new();
        for f in &fat {
            fat_bytes.extend_from_slice(&f.to_le_bytes());
        }
        pad_sector(&mut fat_bytes);
        out.extend_from_slice(&fat_bytes);

        let mut dir_bytes: Vec<u8> = Vec::new();
        for e in &dir {
            dir_bytes.extend_from_slice(&dir_entry_bytes(e));
        }
        pad_sector(&mut dir_bytes);
        out.extend_from_slice(&dir_bytes);

        let mut minifat_bytes: Vec<u8> = Vec::new();
        for m in &minifat {
            minifat_bytes.extend_from_slice(&m.to_le_bytes());
        }
        pad_sector(&mut minifat_bytes);
        out.extend_from_slice(&minifat_bytes);

        let mut mini_out = mini_bytes.clone();
        pad_sector(&mut mini_out);
        out.extend_from_slice(&mini_out);

        for (_, data) in &regular {
            let mut d = data.clone();
            pad_sector(&mut d);
            out.extend_from_slice(&d);
        }
        Ok(out)
    }
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Serialize one 128-byte directory entry (all-black tree colors).
fn dir_entry_bytes(e: &DirEntry) -> Vec<u8> {
    let mut b = vec![0u8; 128];
    let units: Vec<u16> = e.name.encode_utf16().collect();
    for (i, u) in units.iter().take(31).enumerate() {
        b[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
    }
    let name_len = (units.len().min(31) * 2 + 2) as u16;
    b[64..66].copy_from_slice(&name_len.to_le_bytes());
    b[66] = e.object_type;
    b[67] = 1; // black
    b[68..72].copy_from_slice(&e.left.to_le_bytes());
    b[72..76].copy_from_slice(&e.right.to_le_bytes());
    b[76..80].copy_from_slice(&e.child.to_le_bytes());
    b[116..120].copy_from_slice(&e.start_sector.to_le_bytes());
    b[120..128].copy_from_slice(&e.size.to_le_bytes());
    b
}

impl Default for OleWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl ArchiveWriter for OleWriter {
    fn add_file(
        &mut self,
        entry: &NewEntry,
        data: &[u8],
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        self.streams.insert(entry.name.clone(), data.to_vec());
        Ok(())
    }

    fn add_directory(
        &mut self,
        entry: &NewEntry,
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        // Storages are implied by path prefixes; an empty dir entry
        // only matters when nothing lives under it — record it as a
        // zero-length stream marker.
        if !self
            .streams
            .keys()
            .any(|k| k.starts_with(&entry.name) && k.len() > entry.name.len())
        {
            self.streams.insert(
                format!("{}/.", entry.name.trim_end_matches('/')),
                Vec::new(),
            );
        }
        Ok(())
    }

    fn add_symlink(
        &mut self,
        _entry: &NewEntry,
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        Err(ArchiveError::UnsupportedFeature {
            reason: "ole: symlinks are not representable in compound files".into(),
        })
    }

    fn finish(&mut self) -> Result<(), ArchiveError> {
        Ok(())
    }
}

#[allow(clippy::too_many_lines)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::OleReader;

    fn build() -> Vec<u8> {
        let opts = WriteOptions::deterministic();
        let mut w = OleWriter::new();
        w.add_file(
            &NewEntry::file("storage/big.bin", &opts),
            [0xAB; 8192].as_slice(),
            &opts,
        )
        .unwrap();
        w.add_file(
            &NewEntry::file("storage/small.txt", &opts),
            b"tiny ole stream".as_slice(),
            &opts,
        )
        .unwrap();
        w.add_file(
            &NewEntry::file("root.txt", &opts),
            b"at root".as_slice(),
            &opts,
        )
        .unwrap();
        w.finish_bytes().unwrap()
    }

    #[test]
    fn round_trip() {
        let bytes = build();
        let r = OleReader::from_bytes(&bytes).unwrap();
        assert_eq!(r.read_stream("storage/big.bin").unwrap(), vec![0xAB; 8192]);
        assert_eq!(
            r.read_stream("storage/small.txt").unwrap(),
            b"tiny ole stream"
        );
        assert_eq!(r.read_stream("root.txt").unwrap(), b"at root");
        let paths = r.stream_paths();
        assert!(paths.iter().any(|(p, _, _)| p == "storage/big.bin"));
    }

    #[test]
    fn deterministic() {
        assert_eq!(build(), build());
    }
}
