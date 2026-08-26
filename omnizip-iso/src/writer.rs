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
    /// Full-fidelity name for Rock Ridge NM / Joliet UCS-2.
    full_name: String,
    is_dir: bool,
    is_link: bool,
    link_target: String,
    mode: u32,
    data: Vec<u8>,
    extent: u32,
    /// Total serialized directory size (dirs only).
    dir_size: u32,
    /// Joliet tree directory extent (dirs only).
    j_extent: u32,
    /// Joliet tree directory size (dirs only).
    j_dir_size: u32,
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
                full_name: String::new(),
                is_dir: true,
                is_link: false,
                link_target: String::new(),
                mode: 0o555,
                data: Vec::new(),
                extent: 0,
                dir_size: 0,
                j_extent: 0,
                j_dir_size: 0,
                children: Vec::new(),
            },
        );
        for d in &dir_paths {
            let entry = self.dirs.get(d);
            nodes.get_mut("").expect("root").children.push(d.clone());
            nodes.insert(
                d.clone(),
                Node {
                    iso_name: format!("{}/", iso_mangle(&base_name(d))),
                    full_name: base_name(d),
                    is_dir: true,
                    is_link: false,
                    link_target: String::new(),
                    mode: entry.map_or(0o555, |e| e.mode),
                    data: Vec::new(),
                    extent: 0,
                    dir_size: 0,
                    j_extent: 0,
                    j_dir_size: 0,
                    children: Vec::new(),
                },
            );
        }
        for (path, (entry, data)) in &self.files {
            nodes
                .get_mut(&parent_of(path))
                .ok_or_else(|| {
                    ArchiveError::InvalidArchive(format!("iso: no parent dir for {path}"))
                })?
                .children
                .push(path.clone());
            let (is_link, link_target) = match &entry.kind {
                omnizip_archive_core::EntryKind::Symlink(t) => (true, t.clone()),
                _ => (false, String::new()),
            };
            nodes.insert(
                path.clone(),
                Node {
                    iso_name: format!("{};1", iso_mangle(&base_name(path))),
                    full_name: base_name(path),
                    is_dir: false,
                    is_link,
                    link_target,
                    mode: entry.mode,
                    data: data.clone(),
                    extent: 0,
                    dir_size: 0,
                    j_extent: 0,
                    j_dir_size: 0,
                    children: Vec::new(),
                },
            );
        }
        for node in nodes.values_mut() {
            node.children.sort();
        }

        // Directory sizes for both trees (needed before extents exist).
        let sizes: Vec<(String, u32, u32)> = nodes
            .iter()
            .filter(|(_, n)| n.is_dir)
            .map(|(path, node)| {
                // "." carries SP; ".." is bare. Both trees mirror the
                // Rock Ridge area so either tree yields the metadata.
                let mut size = record_len(1, dot_su().len()) + 34;
                let mut jsize = record_len(1, dot_su().len()) + 34;
                for c in &node.children {
                    let cn = nodes.get(c.as_str()).expect("child");
                    let su = rr_area(
                        &cn.full_name,
                        cn.mode,
                        cn.is_dir,
                        cn.is_link.then_some(cn.link_target.as_str()),
                    );
                    size += record_len(cn.iso_name.len(), su.len());
                    jsize += record_len(ucs2(&cn.full_name).len(), su.len());
                }
                (path.clone(), size as u32, jsize as u32)
            })
            .collect();
        for (path, size, jsize) in sizes {
            let n = nodes.get_mut(&path).expect("dir");
            n.dir_size = size;
            n.j_dir_size = jsize;
        }

        // Layout: [16] PVD, [17] SVD (Joliet), [18] terminator,
        // [19] L path table, [20] M path table, [21] Joliet L,
        // [22] Joliet M, [23..] PVD dirs, Joliet dirs, then files.
        let mut next_extent = 23u32;
        for (path, node) in nodes.iter_mut() {
            if node.is_dir && !node.children.is_empty() {
                node.extent = next_extent;
                next_extent += node.dir_size.div_ceil(2048);
                let _ = path;
            }
        }
        for (path, node) in nodes.iter_mut() {
            if node.is_dir && !node.children.is_empty() {
                node.j_extent = next_extent;
                next_extent += node.j_dir_size.div_ceil(2048);
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
        let pt_l = build_path_table(&nodes, false, false);
        let pt_m = build_path_table(&nodes, true, false);
        pvd[132..136].copy_from_slice(&(pt_l.len() as u32).to_le_bytes());
        pvd[136..140].copy_from_slice(&(pt_l.len() as u32).to_be_bytes());
        pvd[140..144].copy_from_slice(&19u32.to_le_bytes());
        pvd[148..152].copy_from_slice(&20u32.to_be_bytes());
        pvd[881] = 1; // file structure version
        pvd[190..318].copy_from_slice(iso_field("", 128).as_bytes());
        pvd[318..446].copy_from_slice(iso_field("", 128).as_bytes());
        pvd[446..574].copy_from_slice(iso_field(&options.host_tool, 128).as_bytes());
        let root = nodes.get("").expect("root");
        let mut root_rec = record_bytes(0x02, root.extent, root.dir_size, options, b"\x00", &[]);
        root_rec[32] = 1;
        pvd[156..156 + root_rec.len()].copy_from_slice(&root_rec);
        out.extend_from_slice(&pvd);

        // Sector 17: Joliet supplementary descriptor (escape bytes
        // 88..91 = "%/E" mark the UCS-2 level-3 tree).
        let jpt_l = build_path_table(&nodes, false, true);
        let jpt_m = build_path_table(&nodes, true, true);
        let mut svd = vec![0u8; 2048];
        svd[0] = 2;
        svd[1..6].copy_from_slice(b"CD001");
        svd[6] = 1;
        svd[88] = 0x25;
        svd[89] = 0x2F;
        svd[90] = 0x45;
        svd[8..40].copy_from_slice(&ucs2(&iso_field("", 16)));
        svd[40..72].copy_from_slice(&ucs2(&iso_field(&self.volume_id, 16)));
        svd[80..84].copy_from_slice(&volume_space.to_le_bytes());
        svd[84..88].copy_from_slice(&volume_space.to_be_bytes());
        svd[120..122].copy_from_slice(&1u16.to_le_bytes());
        svd[122..124].copy_from_slice(&1u16.to_be_bytes());
        svd[124..126].copy_from_slice(&1u16.to_le_bytes());
        svd[126..128].copy_from_slice(&1u16.to_be_bytes());
        svd[128..130].copy_from_slice(&2048u16.to_le_bytes());
        svd[130..132].copy_from_slice(&2048u16.to_be_bytes());
        svd[132..136].copy_from_slice(&(jpt_l.len() as u32).to_le_bytes());
        svd[136..140].copy_from_slice(&(jpt_l.len() as u32).to_be_bytes());
        svd[140..144].copy_from_slice(&21u32.to_le_bytes());
        svd[148..152].copy_from_slice(&22u32.to_be_bytes());
        let jroot = nodes.get("").expect("root");
        let mut jroot_rec = record_bytes(
            0x02,
            jroot.j_extent,
            jroot.j_dir_size,
            options,
            b"\x00",
            &[],
        );
        jroot_rec[32] = 1;
        svd[156..156 + jroot_rec.len()].copy_from_slice(&jroot_rec);
        svd[881] = 1;
        out.extend_from_slice(&svd);

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
        let mut jl_sector = jpt_l;
        jl_sector.resize(2048, 0);
        out.extend_from_slice(&jl_sector);
        let mut jm_sector = jpt_m;
        jm_sector.resize(2048, 0);
        out.extend_from_slice(&jm_sector);

        // Directory extents then file extents, in BTreeMap order.
        for (path, node) in &nodes {
            if node.is_dir && !node.children.is_empty() {
                let bytes = directory_bytes(&nodes, path, options);
                let mut aligned = bytes;
                aligned.resize(aligned.len().div_ceil(2048) * 2048, 0);
                out.extend_from_slice(&aligned);
            }
        }
        for (path, node) in &nodes {
            if node.is_dir && !node.children.is_empty() {
                let bytes = joliet_directory_bytes(&nodes, path, options);
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

/// Serialized record length: 33 + name (+pad) + su (+pad), even.
fn record_len(name_len: usize, su_len: usize) -> usize {
    let mut l = 33 + name_len;
    if name_len % 2 == 0 {
        l += 1;
    }
    l += su_len;
    if l % 2 != 0 {
        l += 1;
    }
    l
}

/// One directory record (33-byte header + name + pad + system use).
fn record_bytes(
    flags: u8,
    extent: u32,
    size: u32,
    options: &WriteOptions,
    name: &[u8],
    su: &[u8],
) -> Vec<u8> {
    let date = iso_date(options.mtime);
    let name_len = name.len();
    let mut rec = Vec::with_capacity(record_len(name_len, su.len()));
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
    rec.extend_from_slice(name);
    if name_len % 2 == 0 {
        rec.push(0);
    }
    rec.extend_from_slice(su);
    if rec.len() % 2 != 0 {
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
    out.extend_from_slice(&record_bytes(
        0x02,
        own,
        node.dir_size,
        options,
        b"\x00",
        &dot_su(),
    ));
    out.extend_from_slice(&record_bytes(
        0x02,
        parent,
        node.dir_size,
        options,
        b"\x01",
        &[],
    ));
    for child in &node.children {
        let cn = &nodes[child.as_str()];
        let su = rr_area(
            &cn.full_name,
            cn.mode,
            cn.is_dir,
            cn.is_link.then_some(cn.link_target.as_str()),
        );
        if cn.is_dir {
            out.extend_from_slice(&record_bytes(
                0x02,
                cn.extent,
                cn.dir_size,
                options,
                cn.iso_name.as_bytes(),
                &su,
            ));
        } else {
            out.extend_from_slice(&record_bytes(
                0x00,
                cn.extent,
                cn.data.len() as u32,
                options,
                cn.iso_name.as_bytes(),
                &su,
            ));
        }
    }
    out
}

/// Joliet directory extent bytes: UCS-2BE names, `j_extent` tree,
/// same Rock Ridge system-use area as the primary tree.
fn joliet_directory_bytes(
    nodes: &BTreeMap<String, Node>,
    path: &str,
    options: &WriteOptions,
) -> Vec<u8> {
    let node = nodes.get(path).expect("dir");
    let own = node.j_extent;
    let parent_path = match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    };
    let parent = nodes
        .get(&parent_path)
        .filter(|n| n.j_extent != 0)
        .map_or(own, |n| n.j_extent);

    let mut out = Vec::new();
    out.extend_from_slice(&record_bytes(
        0x02,
        own,
        node.j_dir_size,
        options,
        b"\x00",
        &dot_su(),
    ));
    out.extend_from_slice(&record_bytes(
        0x02,
        parent,
        node.j_dir_size,
        options,
        b"\x01",
        &[],
    ));
    for child in &node.children {
        let cn = &nodes[child.as_str()];
        let su = rr_area(
            &cn.full_name,
            cn.mode,
            cn.is_dir,
            cn.is_link.then_some(cn.link_target.as_str()),
        );
        if cn.is_dir {
            out.extend_from_slice(&record_bytes(
                0x02,
                cn.j_extent,
                cn.j_dir_size,
                options,
                &ucs2(&cn.full_name),
                &su,
            ));
        } else {
            out.extend_from_slice(&record_bytes(
                0x00,
                cn.extent,
                cn.data.len() as u32,
                options,
                &ucs2(&cn.full_name),
                &su,
            ));
        }
    }
    out
}

/// Path table (root + every non-empty directory); `be` selects the
/// big-endian (M) encoding.
fn build_path_table(nodes: &BTreeMap<String, Node>, be: bool, joliet: bool) -> Vec<u8> {
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
        let name = if joliet {
            ucs2(&node.full_name)
        } else {
            node.iso_name.as_bytes().to_vec()
        };
        out.push(name.len() as u8);
        out.extend_from_slice(&ext(if joliet { node.j_extent } else { node.extent }));
        out.extend_from_slice(&num16(number.get(parent).copied().unwrap_or(1)));
        out.extend_from_slice(&name);
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
        entry: &NewEntry,
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        self.files
            .insert(entry.name.clone(), (entry.clone(), Vec::new()));
        Ok(())
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
    use omnizip_archive_core::{ArchiveReader, EntryKind};

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
        assert_eq!(names, vec!["docs", "docs/readme.txt", "hello.dat"]);
        let readme = names.iter().position(|n| n.contains("readme")).unwrap();
        assert_eq!(
            r.read_entry(readme).unwrap(),
            b"iso round trip\n".repeat(40)
        );
        let hello = names.iter().position(|n| *n == "hello.dat").unwrap();
        assert_eq!(r.read_entry(hello).unwrap(), vec![0x42; 4096]);
    }

    #[test]
    fn round_trip_rock_ridge_joliet() {
        let opts = WriteOptions::deterministic().with_mtime(1_700_000_000);
        let mut w = IsoWriter::new("MixedCaseVol");
        let long_name = "a-very-long-mixed-case-name-0123456789.txt";
        w.add_file(
            &NewEntry::file(format!("Deep/Sub {long_name}"), &opts),
            b"deep data".as_slice(),
            &opts,
        )
        .unwrap();
        let mut link_opts = WriteOptions::deterministic().with_mtime(1_700_000_000);
        link_opts.file_mode = 0o755;
        w.add_symlink(
            &NewEntry::symlink(
                "Deep/alias",
                "../a-very-long-mixed-case-name-0123456789.txt",
                &link_opts,
            ),
            &link_opts,
        )
        .unwrap();
        let bytes = w.finish_bytes(&opts).unwrap();

        let mut r = IsoReader::from_bytes(&bytes).unwrap();
        assert_eq!(r.volume_identifier(), "MixedCaseVol");
        let entries = r.entries().unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Deep", &format!("Deep/Sub {long_name}"), "Deep/alias",]
        );
        let file_idx = names
            .iter()
            .position(|n| n.starts_with("Deep/Sub"))
            .expect("file");
        assert_eq!(r.read_entry(file_idx).unwrap(), b"deep data".to_vec());
        assert_eq!(entries[file_idx].mode, Some(0o644), "PX mode from RR area");
        let link_idx = names.iter().position(|n| *n == "Deep/alias").expect("link");
        assert_eq!(
            entries[link_idx].kind,
            EntryKind::Symlink(format!("../{long_name}")),
            "SL target recovered"
        );
        assert_eq!(entries[link_idx].mode, Some(0o755));
    }

    #[test]
    fn deterministic() {
        assert_eq!(build(), build());
    }
}

// --- Rock Ridge SUSP + Joliet emission (task: RR/Joliet round-trip) ---

/// SUSP system-use entry: signature, length, version, payload.
fn susp(sig: &[u8; 2], payload: &[u8]) -> Vec<u8> {
    let mut e = Vec::with_capacity(4 + payload.len());
    e.extend_from_slice(sig);
    e.push((4 + payload.len()) as u8);
    e.push(1); // SUSP version
    e.extend_from_slice(payload);
    e
}

/// NM entries (name, split with CONTINUE when long).
fn susp_nm(name: &str) -> Vec<u8> {
    let bytes = name.as_bytes();
    let parts: Vec<&[u8]> = if bytes.is_empty() {
        vec![&[]]
    } else {
        bytes.chunks(250).collect()
    };
    let mut out = Vec::new();
    for (i, chunk) in parts.iter().enumerate() {
        let flags = if i + 1 < parts.len() { 0x01u8 } else { 0x00 };
        let mut payload = vec![flags];
        payload.extend_from_slice(chunk);
        out.extend_from_slice(&susp(b"NM", &payload));
    }
    out
}

/// PX: full st_mode (type + permission bits), links, uid, gid,
/// serial (RRIP 1.12 layout). Consumers such as libarchive replace
/// the mode wholesale, so the POSIX type bits must be present.
fn susp_px(mode: u32) -> Vec<u8> {
    let mut p = Vec::with_capacity(32);
    p.extend_from_slice(&mode.to_le_bytes());
    p.extend_from_slice(&1u32.to_le_bytes()); // links
    p.extend_from_slice(&0u32.to_le_bytes()); // uid
    p.extend_from_slice(&0u32.to_le_bytes()); // gid
    p.extend_from_slice(&0u64.to_le_bytes()); // serial
    p.extend_from_slice(&0u64.to_le_bytes()); // serial hi
    susp(b"PX", &p)
}

/// SL: symlink target as RRIP components (flags byte, then
/// per-component cflags + length + data; empty first component marks
/// an absolute path).
fn susp_sl(target: &str) -> Vec<u8> {
    let mut p = vec![0u8]; // SL flags: no CONTINUE
    if target.starts_with('/') {
        p.extend_from_slice(&[0u8, 0]); // root
    }
    let parts: Vec<&str> = target.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        p.extend_from_slice(&[0u8, 0]);
    }
    for part in parts {
        match part {
            "." => p.extend_from_slice(&[0x02, 0]),
            ".." => p.extend_from_slice(&[0x04, 0]),
            _ => {
                p.push(0);
                p.push(part.len().clamp(1, 250) as u8);
                p.extend_from_slice(part.as_bytes());
            }
        }
    }
    susp(b"SL", &p)
}

/// SP entry: marks the system-use area as SUSP (root "." records).
fn susp_sp() -> Vec<u8> {
    susp(b"SP", &[0xBE, 0xEF, 0])
}

/// "."-record system use: SP followed by the RR marker, so SUSP
/// consumers find a well-formed entry after SP (libarchive requires
/// one; a lone pad byte there disables Rock Ridge entirely).
fn dot_su() -> Vec<u8> {
    let mut su = susp_sp();
    su.extend_from_slice(&susp(b"RR", &[0]));
    su
}

/// RR-area for a file/dir record: NM + PX (+ SL for links).
fn rr_area(name: &str, mode: u32, is_dir: bool, link_target: Option<&str>) -> Vec<u8> {
    let type_bits = if link_target.is_some() {
        0o120_000 // S_IFLNK
    } else if is_dir {
        0o040_000 // S_IFDIR
    } else {
        0o100_000 // S_IFREG
    };
    let mut su = Vec::new();
    su.extend_from_slice(&susp_nm(name));
    su.extend_from_slice(&susp_px(mode | type_bits));
    if let Some(t) = link_target {
        su.extend_from_slice(&susp_sl(t));
    }
    su
}

fn ucs2(name: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(name.len() * 2);
    for unit in name.encode_utf16() {
        out.extend_from_slice(&unit.to_be_bytes());
    }
    out
}
