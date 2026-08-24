//! PAR2 packet framing — write + parse the 64-byte header and typed
//! bodies (main, file description, IFSC, recovery slice).
#![forbid(unsafe_code)]

use crate::{invalid, packet_type, RecoverySet, TrackedFile, PACKET_HEADER_SIZE, PACKET_MAGIC};
use omnizip_archive_core::ArchiveError;

/// Serialize one packet: magic, length, MD5 over (set id ‖ type ‖
/// body), set id, type, body, 4-byte-aligned zero padding.
#[must_use]
pub fn write_packet(set_id: &[u8; 16], type_: &[u8; 16], body: &[u8]) -> Vec<u8> {
    // The length field and the MD5 both cover the 4-byte-aligned
    // padded body (self-consistent; the spec's alignment rule).
    let padded_len = (PACKET_HEADER_SIZE + body.len()).next_multiple_of(4);
    let padded_body_len = padded_len - PACKET_HEADER_SIZE;
    let mut padded_body = body.to_vec();
    padded_body.resize(padded_body_len, 0);

    let mut hashed = Vec::with_capacity(32 + padded_body_len);
    hashed.extend_from_slice(set_id);
    hashed.extend_from_slice(type_);
    hashed.extend_from_slice(&padded_body);
    let md5 = omnizip_crypto::md5(&hashed);

    let mut out = Vec::with_capacity(padded_len);
    out.extend_from_slice(PACKET_MAGIC);
    out.extend_from_slice(&(padded_len as u64).to_le_bytes());
    out.extend_from_slice(&md5);
    out.extend_from_slice(set_id);
    out.extend_from_slice(type_);
    out.extend_from_slice(&padded_body);
    out
}

/// One parsed packet.
pub struct Packet<'a> {
    pub set_id: [u8; 16],
    pub type_: [u8; 16],
    pub body: &'a [u8],
}

/// Parse all packets in a PAR2 volume.
///
/// # Errors
///
/// [`ArchiveError`] on bad magic/length/checksum.
pub fn parse_packets(data: &[u8]) -> Result<Vec<Packet<'_>>, ArchiveError> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + PACKET_HEADER_SIZE <= data.len() {
        if &data[pos..pos + 8] != PACKET_MAGIC {
            return Err(invalid("par2: bad packet magic"));
        }
        let len = u64::from_le_bytes(
            data[pos + 8..pos + 16]
                .try_into()
                .map_err(|_| invalid("par2: bad length"))?,
        ) as usize;
        if len < PACKET_HEADER_SIZE || pos + len > data.len() {
            return Err(invalid("par2: packet length out of bounds"));
        }
        let md5: [u8; 16] = data[pos + 16..pos + 32]
            .try_into()
            .map_err(|_| invalid("par2: bad md5"))?;
        let set_id: [u8; 16] = data[pos + 32..pos + 48]
            .try_into()
            .map_err(|_| invalid("par2: bad set id"))?;
        let type_: [u8; 16] = data[pos + 48..pos + 64]
            .try_into()
            .map_err(|_| invalid("par2: bad type"))?;
        let body = &data[pos + PACKET_HEADER_SIZE..pos + len];

        let mut hashed = Vec::with_capacity(32 + body.len());
        hashed.extend_from_slice(&set_id);
        hashed.extend_from_slice(&type_);
        hashed.extend_from_slice(body);
        if omnizip_crypto::md5(&hashed) != md5 {
            return Err(ArchiveError::Checksum("par2: packet md5 mismatch".into()));
        }

        out.push(Packet {
            set_id,
            type_,
            body,
        });
        pos += len;
    }
    Ok(out)
}

/// Assemble a [`RecoverySet`] from parsed packets.
///
/// # Errors
///
/// [`ArchiveError`] on structure problems.
pub fn assemble(packets: &[Packet<'_>]) -> Result<RecoverySet, ArchiveError> {
    let mut set = RecoverySet::default();
    let mut main_seen = false;
    for p in packets {
        if set.set_id == [0; 16] {
            set.set_id = p.set_id;
        } else if set.set_id != p.set_id {
            return Err(invalid("par2: mixed recovery set ids"));
        }
        match p.type_ {
            t if &t == packet_type::MAIN => {
                if p.body.len() < 16 {
                    return Err(invalid("par2: short main packet"));
                }
                set.block_size = u64::from_le_bytes(p.body[0..8].try_into().expect("8"));
                main_seen = true;
            }
            t if &t == packet_type::FILE_DESCRIPTION => {
                if p.body.len() < 16 + 16 + 16 + 8 {
                    return Err(invalid("par2: short file description"));
                }
                let mut file_id = [0u8; 16];
                file_id.copy_from_slice(&p.body[0..16]);
                let name = p.body[56..]
                    .split(|&b| b == 0)
                    .next()
                    .unwrap_or(&[])
                    .to_vec();
                set.files.push(TrackedFile {
                    file_id,
                    name: String::from_utf8_lossy(&name).into_owned(),
                    length: u64::from_le_bytes(p.body[48..56].try_into().expect("8")),
                    slices: Vec::new(),
                });
            }
            t if &t == packet_type::IFSC => {
                if p.body.len() < 16 {
                    return Err(invalid("par2: short ifsc packet"));
                }
                let mut file_id = [0u8; 16];
                file_id.copy_from_slice(&p.body[0..16]);
                let mut slices = Vec::new();
                let mut off = 16usize;
                while off + 24 <= p.body.len() {
                    let crc = u64::from_le_bytes(p.body[off..off + 8].try_into().expect("8"));
                    let mut md5 = [0u8; 16];
                    md5.copy_from_slice(&p.body[off + 8..off + 24]);
                    slices.push((crc, md5));
                    off += 24;
                }
                if let Some(f) = set.files.iter_mut().find(|f| f.file_id == file_id) {
                    f.slices = slices;
                }
            }
            t if &t == packet_type::RECOVERY => {
                if p.body.len() < 4 {
                    return Err(invalid("par2: short recovery packet"));
                }
                let exponent = u32::from_le_bytes(p.body[0..4].try_into().expect("4"));
                set.recovery.push((exponent, p.body[4..].to_vec()));
            }
            _ => {}
        }
    }
    if !main_seen {
        return Err(invalid("par2: no main packet"));
    }
    Ok(set)
}

/// Serialize the main packet body.
#[must_use]
pub fn main_body(block_size: u64, file_ids: &[[u8; 16]]) -> Vec<u8> {
    let mut b = Vec::with_capacity(16 + file_ids.len() * 16);
    b.extend_from_slice(&block_size.to_le_bytes());
    b.extend_from_slice(&(file_ids.len() as u64).to_le_bytes());
    for id in file_ids {
        b.extend_from_slice(id);
    }
    b
}

/// Serialize a file-description packet body.
#[must_use]
pub fn file_description_body(
    file_id: &[u8; 16],
    hash16: &[u8; 16],
    hash: &[u8; 16],
    length: u64,
    name: &str,
) -> Vec<u8> {
    let mut b = Vec::with_capacity(64 + name.len());
    b.extend_from_slice(file_id);
    b.extend_from_slice(hash16);
    b.extend_from_slice(hash);
    b.extend_from_slice(&length.to_le_bytes());
    b.extend_from_slice(name.as_bytes());
    b.push(0);
    b
}

/// Serialize an IFSC packet body.
#[must_use]
pub fn ifsc_body(file_id: &[u8; 16], slices: &[(u64, [u8; 16])]) -> Vec<u8> {
    let mut b = Vec::with_capacity(16 + slices.len() * 24);
    b.extend_from_slice(file_id);
    for (crc, md5) in slices {
        b.extend_from_slice(&crc.to_le_bytes());
        b.extend_from_slice(md5);
    }
    b
}

/// Serialize a recovery packet body.
#[must_use]
pub fn recovery_body(exponent: u32, data: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(4 + data.len());
    b.extend_from_slice(&exponent.to_le_bytes());
    b.extend_from_slice(data);
    b
}
