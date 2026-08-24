//! ISO 9660 writer — a deterministic plain level-1 image (task 17
//! rules): PVD + terminator, sorted directory records, little-endian
//! path tables, ISO-mangled uppercase names with `;1` versions, fixed
//! recording dates from `WriteOptions`.
#![forbid(unsafe_code)]

use omnizip_archive_core::write_options::WriteOptions;
use omnizip_archive_core::{ArchiveError, ArchiveWriter, NewEntry};
use std::collections::BTreeMap;

#[derive(Clone)]
struct Node {
    iso_name: String,
    is_dir: bool,
    data: Vec<u8>,
    extent: u32,
    /// Total serialized directory size (dirs only).
    dir_size: u32,
    children: Vec<String>,
}

/// Builds a deterministic ISO image in memory.
pub struct IsoWriter {
    files: BTreeMap<String, (NewEntry, Vec<u8>)>,
    dirs: BTreeMap<String, NewEntry>,
    volume_id: String,
}

impl IsoWriter {
    #[must_use]
    pub fn new(volume_id: &str) -> Self {
        Self {
            files: BTreeMap::new(),
            dirs: BTreeMap::new(),
            volume_id: iso_field(volume_id, 32),
        }
    }

    /// Serialize the image.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::InvalidArchive`] on missing parent dirs.
    pub fn finish_bytes(&mut self, options: &WriteOptions) -> Result<Vec<u8>, ArchiveError> {
        let parent_of = |path: &str| -> String {
            match path.rfind('/') {
                Some(i) => path[..i].to_string(),
                None => String::new(),
            }
        };

        // Collect every directory path (explicit + implied parents).
        let mut dir_paths: Vec<String> = self.dirs.keys().cloned().collect();
        for path in self.files.keys() {
            let mut p = parent_of(path);
            while !p.is_empty() && !dir_paths.contains(&p) {
                dir_paths.push(p.clone());
                p = parent_of(&p);
            }
        }
        dir_paths.sort();
        dir_paths.dedup();

        let mut nodes: BTreeMap<String, Node> = BTreeMap::new();
        nodes.insert(
            String::new(),
            Node {
                iso_name: String::new(),
                is_dir: true,
                data: Vec::new(),
                extent: 0,
                dir_size: 0,
                children: Vec::new(),
            },
        );
        for d in &dir_paths {
            nodes.get_mut("").expect("root").children.push(d.clone());
            nodes.insert(
                d.clone(),
                Node {
                    iso_name: format!("{}/", iso_mangle(&base_name(d))),
                    is_dir: true,
                    data: Vec::new(),
                    extent: 0,
                    dir_size: 0,
                    children: Vec::new(),
                },
            );
        }
        for (path, (_, data)) in &self.files {
            nodes
                .get_mut(&parent_of(path))
                .ok_or_else(|| {
                    ArchiveError::InvalidArchive(format!("iso: no parent dir for {path}"))
                })?
                .children
                .push(path.clone());
            nodes.insert(
                path.clone(),
                Node {
                    iso_name: format!("{};1", iso_mangle(&base_name(path))),
                    is_dir: false,
                    data: data.clone(),
                    extent: 0,
                    dir_size: 0,
                    children: Vec::new(),
                },
            );
        }
        for node in nodes.values_mut() {
            node.children.sort();
        }

        // Directory sizes (needed for records before extents exist).
        let sizes: Vec<(String, u32)> = nodes
            .iter()
            .filter(|(_, n)| n.is_dir)
            .map(|(path, node)| {
                let mut size = 34 + 34; // "." and ".."
                for c in &node.children {
                    let cn = nodes.get(c.as_str()).expect("child");
                    let name_len = cn.iso_name.len();
                    // Pad so each record length stays even.
                    size += 33 + name_len + (name_len + 1) % 2;
                }
                (path.clone(), size as u32)
            })
            .collect();
        for (path, size) in sizes {
            nodes.get_mut(&path).expect("dir").dir_size = size;
        }

        // Layout: [16] PVD, [17] terminator, [18] L path table,
        // [19] M path table, [20..] directories, then files.
        let mut next_extent = 20u32;
        for (path, node) in nodes.iter_mut() {
            if node.is_dir && !node.children.is_empty() {
                node.extent = next_extent;
                next_extent += node.dir_size.div_ceil(2048);
                let _ = path;
            }
        }
        for (path, node) in nodes.iter_mut() {
            if !node.is_dir {
                node.extent = next_extent;
                next_extent += node.data.len().div_ceil(2048) as u32;
                let _ = path;
            }
        }
        let volume_space = next_extent;

        let mut out: Vec<u8> = vec![0u8; 16 * 2048];

        // PVD.
        let mut pvd = vec![0u8; 2048];
        pvd[0] = 1;
        pvd[1..6].copy_from_slice(b"CD001");
        pvd[6] = 1;
        pvd[8..40].copy_from_slice(iso_field("", 32).as_bytes());
        pvd[40..72].copy_from_slice(self.volume_id.as_bytes());
        pvd[80..84].copy_from_slice(&volume_space.to_le_bytes());
        pvd[84..88].copy_from_slice(&volume_space.to_be_bytes());
        pvd[120..122].copy_from_slice(&1u16.to_le_bytes());
        pvd[122..124].copy_from_slice(&1u16.to_be_bytes());
        pvd[124..126].copy_from_slice(&1u16.to_le_bytes());
        pvd[126..128].copy_from_slice(&1u16.to_be_bytes());
        pvd[128..130].copy_from_slice(&2048u16.to_le_bytes());
        pvd[130..132].copy_from_slice(&2048u16.to_be_bytes());
        let pt_l = build_path_table(&nodes, false);
        let pt_m = build_path_table(&nodes, true);
        pvd[132..136].copy_from_slice(&(pt_l.len() as u32).to_le_bytes());
        pvd[140..144].copy_from_slice(&18u32.to_le_bytes());
        pvd[148..152].copy_from_slice(&19u32.to_be_bytes());
        let root = nodes.get("").expect("root");
        let mut root_rec = record_bytes(0x02, root.extent, root.dir_size, options, "\x00");
        root_rec[32] = 1;
        pvd[156..190].copy_from_slice(&root_rec[..34]);
        pvd[881] = 1; // file structure version
        pvd[190..318].copy_from_slice(iso_field("", 128).as_bytes());
        pvd[318..446].copy_from_slice(iso_field("", 128).as_bytes());
        pvd[446..574].copy_from_slice(iso_field(&options.host_tool, 128).as_bytes());
        out.extend_from_slice(&pvd);

        // Terminator.
        let mut term = vec![0u8; 2048];
        term[0] = 255;
        term[1..6].copy_from_slice(b"CD001");
        term[6] = 1;
        out.extend_from_slice(&term);

        // Path table sectors (L then M).
        let mut l_sector = pt_l;
        l_sector.resize(2048, 0);
        out.extend_from_slice(&l_sector);
        let mut m_sector = pt_m;
        m_sector.resize(2048, 0);
        out.extend_from_slice(&m_sector);

        // Directory extents then file extents, in BTreeMap order.
        for (path, node) in &nodes {
            if node.is_dir && !node.children.is_empty() {
                let bytes = directory_bytes(&nodes, path, options);
                let mut aligned = bytes;
                aligned.resize(aligned.len().div_ceil(2048) * 2048, 0);
                out.extend_from_slice(&aligned);
            }
        }
        for node in nodes.values() {
            if !node.is_dir {
                let mut aligned = node.data.clone();
                aligned.resize(aligned.len().div_ceil(2048) * 2048, 0);
                out.extend_from_slice(&aligned);
            }
        }
        Ok(out)
    }
}

fn base_name(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[i + 1..].to_string(),
        None => path.to_string(),
    }
}

/// ISO level-1 name mangling: uppercase A-Z/0-9/`_`, 8.3 shape.
fn iso_mangle(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    let (stem, ext) = match clean.split_once('.') {
        Some((s, e)) => (s, e),
        None => (clean.as_str(), ""),
    };
    let stem: String = stem.chars().take(8).collect();
    let ext: String = ext.chars().take(3).collect();
    if ext.is_empty() {
        stem
    } else {
        format!("{stem}.{ext}")
    }
}

fn iso_field(s: &str, width: usize) -> String {
    let mut out: String = s.chars().take(width).collect();
    while out.len() < width {
        out.push(' ');
    }
    out
}

/// Recording date (7 bytes) from unix seconds.
fn iso_date(unix: u64) -> [u8; 7] {
    let days = unix / 86_400;
    let rem = unix % 86_400;
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    [
        (y - 1900).clamp(0, 255) as u8,
        m as u8,
        d as u8,
        (rem / 3600) as u8,
        ((rem % 3600) / 60) as u8,
        (rem % 60) as u8,
        0, // UTC
    ]
}

/// One directory record (34-byte header + name + pad).
fn record_bytes(flags: u8, extent: u32, size: u32, options: &WriteOptions, name: &str) -> Vec<u8> {
    let date = iso_date(options.mtime);
    let name_len = name.len();
    let mut rec = Vec::with_capacity(34 + name_len);
    rec.push(0); // length, fixed below
    rec.push(0); // extended attr length
    rec.extend_from_slice(&extent.to_le_bytes());
    rec.extend_from_slice(&extent.to_be_bytes());
    rec.extend_from_slice(&size.to_le_bytes());
    rec.extend_from_slice(&size.to_be_bytes());
    rec.extend_from_slice(&date);
    rec.push(flags);
    rec.push(0); // unit size
    rec.push(0); // interleave gap
    rec.extend_from_slice(&1u16.to_le_bytes());
    rec.extend_from_slice(&1u16.to_be_bytes());
    rec.push(name_len as u8);
    rec.extend_from_slice(name.as_bytes());
    if name_len % 2 == 0 {
        rec.push(0);
    }
    rec[0] = rec.len() as u8;
    rec
}

/// Directory extent bytes for the directory at `path`.
fn directory_bytes(nodes: &BTreeMap<String, Node>, path: &str, options: &WriteOptions) -> Vec<u8> {
    let node = nodes.get(path).expect("dir");
    let own = node.extent;
    let parent_path = match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    };
    let parent = nodes
        .get(&parent_path)
        .filter(|n| n.extent != 0)
        .map_or(own, |n| n.extent);

    let mut out = Vec::new();
    out.extend_from_slice(&record_bytes(0x02, own, node.dir_size, options, "\x00"));
    out.extend_from_slice(&record_bytes(0x02, parent, node.dir_size, options, "\x01"));
    for child in &node.children {
        let cn = &nodes[child.as_str()];
        if cn.is_dir {
            out.extend_from_slice(&record_bytes(
                0x02,
                cn.extent,
                cn.dir_size,
                options,
                &cn.iso_name,
            ));
        } else {
            out.extend_from_slice(&record_bytes(
                0x00,
                cn.extent,
                cn.data.len() as u32,
                options,
                &cn.iso_name,
            ));
        }
    }
    out
}

/// Path table (root + every non-empty directory); `be` selects the
/// big-endian (M) encoding.
fn build_path_table(nodes: &BTreeMap<String, Node>, be: bool) -> Vec<u8> {
    let dirs: Vec<&String> = nodes
        .keys()
        .filter(|k| !k.is_empty())
        .filter(|k| {
            nodes
                .get(k.as_str())
                .is_some_and(|n| n.is_dir && !n.children.is_empty())
        })
        .collect();
    let mut number: BTreeMap<&str, u16> = BTreeMap::new();
    number.insert("", 1);
    for (i, d) in dirs.iter().enumerate() {
        number.insert(d.as_str(), (i + 2) as u16);
    }

    let ext = |v: u32| {
        if be {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    let num16 = |v: u16| {
        if be {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    let mut out = Vec::new();
    let root_ext = nodes.get("").expect("root").extent;
    // Root record: name len 1, extent, parent number 1, name "\0".
    out.push(1);
    out.extend_from_slice(&ext(root_ext));
    out.extend_from_slice(&num16(1));
    out.push(0);
    for d in dirs {
        let parent = match d.rfind('/') {
            Some(i) => &d[..i],
            None => "",
        };
        let node = nodes.get(d.as_str()).expect("dir");
        let name = &node.iso_name;
        out.push(name.len() as u8);
        out.extend_from_slice(&ext(node.extent));
        out.extend_from_slice(&num16(number.get(parent).copied().unwrap_or(1)));
        out.extend_from_slice(name.as_bytes());
        if name.len() % 2 != 0 {
            out.push(0);
        }
    }
    out
}

impl ArchiveWriter for IsoWriter {
    fn add_file(
        &mut self,
        entry: &NewEntry,
        data: &[u8],
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        self.files
            .insert(entry.name.clone(), (entry.clone(), data.to_vec()));
        Ok(())
    }

    fn add_directory(
        &mut self,
        entry: &NewEntry,
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        self.dirs.insert(entry.name.clone(), entry.clone());
        Ok(())
    }

    fn add_symlink(
        &mut self,
        _entry: &NewEntry,
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        Err(ArchiveError::UnsupportedFeature {
            reason: "iso: symlinks need Rock Ridge; not supported by the level-1 writer".into(),
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
    use crate::reader::IsoReader;
    use omnizip_archive_core::ArchiveReader;

    fn build() -> Vec<u8> {
        let opts = WriteOptions::deterministic().with_mtime(1_700_000_000);
        let mut w = IsoWriter::new("TESTVOL");
        w.add_directory(&NewEntry::directory("docs", &opts), &opts)
            .unwrap();
        w.add_file(
            &NewEntry::file("docs/readme.txt", &opts),
            b"iso round trip\n".repeat(40).as_slice(),
            &opts,
        )
        .unwrap();
        w.add_file(&NewEntry::file("hello.dat", &opts), &[0x42; 4096], &opts)
            .unwrap();
        w.finish_bytes(&opts).unwrap()
    }

    #[test]
    fn round_trip() {
        let bytes = build();
        let mut r = IsoReader::from_bytes(&bytes).unwrap();
        assert_eq!(r.volume_identifier(), "TESTVOL");
        let entries = r.entries().unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["DOCS", "DOCS/README.TXT", "HELLO.DAT"]);
        let readme = names.iter().position(|n| n.contains("README")).unwrap();
        assert_eq!(
            r.read_entry(readme).unwrap(),
            b"iso round trip\n".repeat(40)
        );
        let hello = names.iter().position(|n| *n == "HELLO.DAT").unwrap();
        assert_eq!(r.read_entry(hello).unwrap(), vec![0x42; 4096]);
    }

    #[test]
    fn deterministic() {
        assert_eq!(build(), build());
    }
}
