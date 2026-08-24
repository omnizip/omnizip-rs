//! MSI read layer — string pool (`\x01!` / `\x05!` streams), the
//! `_Tables` / `_Columns` catalogs, and typed row decoding. Port of
//! the Ruby `msi/string_pool.rb` + `table_parser.rb`. Embedded
//! cabinets (compressed MSIs) need a CAB reader — the task-12
//! decision records that as a sibling cabriolet-rs concern; this
//! layer extracts UNCOMPRESSED embedded file streams.
#![forbid(unsafe_code)]

use crate::reader::OleReader;
use omnizip_archive_core::{ArchiveEntry, ArchiveError, ArchiveReader};

/// MSI table column.
#[derive(Clone, Debug)]
pub struct Column {
    pub name: String,
    pub number: u32,
    /// "i2" | "i4" | "string".
    pub kind: String,
    pub width: usize,
    pub primary_key: bool,
}

/// One parsed table.
#[derive(Clone, Debug, Default)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    /// Row-major cell values (strings as `String`, ints as `i64`).
    pub rows: Vec<Vec<MsiValue>>,
}

/// A decoded cell.
#[derive(Clone, Debug, PartialEq)]
pub enum MsiValue {
    Str(String),
    Int(i64),
    Null,
}

/// Reads an MSI database over an [`OleReader`].
pub struct MsiReader {
    ole: OleReader,
    strings: Vec<String>,
    pub tables: Vec<Table>,
}

impl MsiReader {
    /// Open an MSI (or any OLE file; non-MSI files yield no tables).
    ///
    /// # Errors
    ///
    /// As [`OleReader::from_bytes`].
    pub fn from_bytes(data: &[u8]) -> Result<Self, ArchiveError> {
        let ole = OleReader::from_bytes(data)?;
        Self::with_ole(ole)
    }

    /// Wrap an already-opened OLE file.
    ///
    /// # Errors
    ///
    /// Pool/table structure errors.
    pub fn with_ole(ole: OleReader) -> Result<Self, ArchiveError> {
        let mut reader = Self {
            ole,
            strings: Vec::new(),
            tables: Vec::new(),
        };
        reader.load_strings()?;
        reader.load_tables()?;
        Ok(reader)
    }

    /// Open from disk.
    ///
    /// # Errors
    ///
    /// IO or structure errors.
    pub fn open(path: &std::path::Path) -> Result<Self, ArchiveError> {
        let ole = OleReader::open(path)?;
        Self::with_ole(ole)
    }

    /// Expose the underlying OLE stream paths (diagnostics/tests).
    #[must_use]
    pub fn ole_stream_paths(&self) -> Vec<(String, bool, u64)> {
        self.ole.stream_paths()
    }

    fn read_named(&self, base: &str) -> Option<Vec<u8>> {
        // Decode every stream name (the MSI MIME-style base64-ish
        // encoding) and match plain names against them — the Ruby's
        // stream-name map, inverted.
        for (path, is_dir, _) in self.ole.stream_paths() {
            if is_dir {
                continue;
            }
            let encoded: Vec<u16> = path.encode_utf16().collect();
            if decode_stream_name(&encoded) == base {
                if let Ok(data) = self.ole.read_stream(&path) {
                    return Some(data);
                }
            }
        }
        // Prefix-byte fallbacks (some writers).
        for prefix in [0x01u8, 0x05] {
            let name = stream_name(prefix, base);
            if let Ok(data) = self.ole.read_stream(&name) {
                return Some(data);
            }
        }
        None
    }

    fn load_strings(&mut self) -> Result<(), ArchiveError> {
        // Several same-named pool/data streams can coexist (storage
        // sub-views); pick the pair whose lengths sum to the data
        // size — the self-consistent one is the database's.
        let pools = self.all_named("_StringPool");
        let datas = self.all_named("_StringData");
        let u16le = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let parse = |pool: &[u8], data: &[u8]| -> Option<Vec<String>> {
            if pool.len() < 4 {
                return None;
            }
            let mut strings = Vec::new();
            let mut data_offset = 0usize;
            let mut i = 4usize; // skip codepage + fill
            while i + 4 <= pool.len() {
                let length = u16le(pool, i) as usize;
                let _ref_count = u16le(pool, i + 2);
                if length > 0 && data_offset + length <= data.len() {
                    strings.push(
                        String::from_utf8_lossy(&data[data_offset..data_offset + length])
                            .into_owned(),
                    );
                    data_offset += length;
                } else {
                    strings.push(String::new());
                }
                i += 4;
            }
            Some(strings)
        };

        for pool in &pools {
            for data in &datas {
                if let Some(strings) = parse(pool, data) {
                    let total: usize = strings.iter().map(String::len).sum();
                    if total == data.len() && !strings.is_empty() {
                        self.strings = strings;
                        return Ok(());
                    }
                }
            }
        }
        // Fallback: the first pair, unverified.
        if let (Some(pool), Some(data)) = (pools.first(), datas.first()) {
            if let Some(strings) = parse(pool, data) {
                self.strings = strings;
            }
        }
        Ok(())
    }

    /// Every stream whose decoded name equals `base`.
    fn all_named(&self, base: &str) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for (path, is_dir, _) in self.ole.stream_paths() {
            if is_dir {
                continue;
            }
            let units: Vec<u16> = path.encode_utf16().collect();
            if decode_stream_name(&units) == base {
                if let Ok(data) = self.ole.read_stream(&path) {
                    out.push(data);
                }
            }
        }
        out
    }

    fn string_at(&self, idx: usize) -> String {
        // MSI string references are 1-based (0 = null).
        if idx == 0 {
            return String::new();
        }
        self.strings.get(idx - 1).cloned().unwrap_or_default()
    }

    fn load_tables(&mut self) -> Result<(), ArchiveError> {
        let Some(tables_data) = self.read_named("_Tables") else {
            return Ok(());
        };
        let mut names = Vec::new();
        let mut off = 2usize;
        while off + 2 <= tables_data.len() {
            let idx = u16::from_le_bytes([tables_data[off], tables_data[off + 1]]) as usize;
            let name = self.string_at(idx);
            if !name.is_empty() {
                names.push(name);
            }
            off += 2;
        }

        // Column catalog.
        let mut columns: Vec<(String, Vec<Column>)> = Vec::new();
        if let Some(cols) = self.read_named("_Columns") {
            let rows = cols.len() / 8;
            let half = rows * 2;
            let u16col =
                |slice: &[u8], i: usize| u16::from_le_bytes([slice[i * 2], slice[i * 2 + 1]]);
            for i in 0..rows {
                let table = self.string_at(u16col(&cols[..half], i) as usize);
                if table.is_empty() {
                    continue;
                }
                let raw_number = u16col(&cols[half..half * 2], i);
                let name = self.string_at(u16col(&cols[half * 2..half * 3], i) as usize);
                if name.is_empty() {
                    continue;
                }
                let type_raw = u16col(&cols[half * 3..], i);
                let (kind, width) = column_type(type_raw);
                columns.push((
                    table.clone(),
                    vec![Column {
                        name,
                        number: u32::from(raw_number & 0x7FFF),
                        kind: kind.to_string(),
                        width,
                        primary_key: raw_number & 0x8000 != 0,
                    }],
                ));
            }
        }
        // Merge per-table.
        let mut by_table: Vec<(String, Vec<Column>)> = Vec::new();
        for (table, col) in columns {
            if let Some((_, cols)) = by_table.iter_mut().find(|(t, _)| *t == table) {
                cols.push(col[0].clone());
            } else {
                by_table.push((table, vec![col[0].clone()]));
            }
        }
        for (_, cols) in &mut by_table {
            cols.sort_by_key(|c| c.number);
        }

        // Parse each table's rows.
        for name in names {
            let Some(cols) = by_table
                .iter()
                .find(|(t, _)| *t == name)
                .map(|(_, c)| c.clone())
            else {
                continue;
            };
            let rows = self
                .read_named(&name)
                .map(|data| parse_rows(&data, &cols, |i| self.string_at(i)))
                .unwrap_or_default();
            self.tables.push(Table {
                name: name.clone(),
                columns: cols,
                rows,
            });
        }
        Ok(())
    }

    /// The File table, if present: (file key, component, size,
    /// file name).
    #[must_use]
    pub fn file_rows(&self) -> Vec<(String, String, u64, String)> {
        let Some(t) = self.tables.iter().find(|t| t.name == "File") else {
            return Vec::new();
        };
        let col = |want: &str| {
            t.columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(want))
        };
        let (i_key, i_comp, i_size, i_name) = (
            col("File"),
            col("Component_"),
            col("FileSize"),
            col("FileName"),
        );
        t.rows
            .iter()
            .map(|row| {
                let get = |i: Option<usize>| {
                    i.and_then(|i| row.get(i))
                        .cloned()
                        .unwrap_or(MsiValue::Null)
                };
                let key = to_string(&get(i_key));
                let component = to_string(&get(i_comp));
                let size = match get(i_size) {
                    MsiValue::Int(i) => u64::try_from(i).unwrap_or(0),
                    _ => 0,
                };
                let file_name = match get(i_name) {
                    MsiValue::Str(s) => s.split('|').next_back().unwrap_or(&s).to_string(),
                    _ => String::new(),
                };
                (key, component, size, file_name)
            })
            .collect()
    }
}

fn to_string(v: &MsiValue) -> String {
    match v {
        MsiValue::Str(s) => s.clone(),
        MsiValue::Int(i) => i.to_string(),
        MsiValue::Null => String::new(),
    }
}

/// Decode an MSI-encoded stream name (port of the Ruby
/// `Constants.decode_stream_name`): unit 0x4840 is a prefix marker;
/// 0x4800..0x4840 encodes one MIME char; 0x3800..0x4800 two.
/// Decode an MSI-encoded stream name; public for diagnostics.
#[must_use]
pub fn decode_stream_name(units: &[u16]) -> String {
    fn mime_char(v: u16) -> char {
        match v {
            0..=9 => char::from(b'0' + v as u8),
            10..=35 => char::from(b'A' + (v - 10) as u8),
            36..=61 => char::from(b'a' + (v - 36) as u8),
            62 => '.',
            _ => '_',
        }
    }
    let mut out = String::new();
    for (i, &ch) in units.iter().enumerate() {
        if (0x3800..0x4800).contains(&ch) {
            let c = ch - 0x3800;
            out.push(mime_char(c & 0x3F));
            out.push(mime_char((c >> 6) & 0x3F));
        } else if (0x4800..0x4840).contains(&ch) {
            out.push(mime_char(ch - 0x4800));
        } else if i == 0 && (ch == 0x4840 || ch == 0x0005) {
            // Prefix marker.
        } else {
            out.push(char::from_u32(u32::from(ch)).unwrap_or('\u{FFFD}'));
        }
    }
    out
}

fn stream_name(prefix: u8, base: &str) -> String {
    // OLE names are UTF-16; the MSI prefix byte precedes the ASCII
    // name, forming a 2-char-ish name like "\x01!".
    let mut units: Vec<u16> = vec![u16::from(prefix)];
    units.extend(
        base.chars()
            .map(|c| u16::try_from(c as u32).unwrap_or(u16::MAX)),
    );
    String::from_utf16_lossy(&units)
}

fn column_type(raw: u16) -> (&'static str, usize) {
    let low = raw & 0xFF;
    let high = (raw >> 8) & 0xFF;
    let type_id = high & 0x7F;
    match type_id {
        1 if low == 2 => ("i2", 2),
        1 => ("i4", 4),
        2 => ("i4", 4),
        _ => ("string", 2),
    }
}

fn parse_rows(
    data: &[u8],
    columns: &[Column],
    string_at: impl Fn(usize) -> String,
) -> Vec<Vec<MsiValue>> {
    let total: usize = columns.iter().map(|c| c.width).sum();
    if total == 0 {
        return Vec::new();
    }
    let mut rows = Vec::new();
    let mut off = 0usize;
    while off + total <= data.len() {
        let mut row = Vec::with_capacity(columns.len());
        for col in columns {
            let cell = match col.kind.as_str() {
                "i2" => {
                    let v = i16::from_le_bytes([data[off], data[off + 1]]);
                    MsiValue::Int(i64::from(v))
                }
                "i4" => {
                    let b: [u8; 4] = data[off..off + 4].try_into().unwrap_or([0; 4]);
                    MsiValue::Int(i64::from(i32::from_le_bytes(b)))
                }
                _ => {
                    let idx = u16::from_le_bytes([data[off], data[off + 1]]) as usize;
                    if idx == 0 {
                        MsiValue::Null
                    } else {
                        MsiValue::Str(string_at(idx))
                    }
                }
            };
            row.push(cell);
            off += col.width;
        }
        rows.push(row);
    }
    rows
}

impl ArchiveReader for MsiReader {
    fn entries(&mut self) -> Result<Vec<ArchiveEntry>, ArchiveError> {
        self.ole.entries()
    }

    fn read_entry(&mut self, index: usize) -> Result<Vec<u8>, ArchiveError> {
        self.ole.read_entry(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_types() {
        assert_eq!(column_type(0x0102), ("i2", 2));
        assert_eq!(column_type(0x0104), ("i4", 4));
        assert_eq!(column_type(0x25FF), ("string", 2));
    }
}
