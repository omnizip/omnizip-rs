//! omnizip-ffi — the C ABI over the omnizip codecs: the Rust side of
//! the Ruby gem's Rust-accelerated path (`TODO.ref-parity/62`).
//!
//! The surface is deliberately tiny and stable: three codecs
//! (zstd, bzip2, lzma/xz), explicit lengths, one call each way, a
//! thread-local last-error, and one deallocator. Ruby binds it with
//! stdlib `Fiddle` — no compiled extension, no rake-compiler:
//!
//! ```ruby
//! require 'fiddle'
//! lib = Fiddle.dlopen('libomnizip_ffi.dylib')
//! compress = Fiddle::Function.new(lib['ozip_compress'],
//!   [Fiddle::TYPE_VOIDP, Fiddle::TYPE_VOIDP, Fiddle::TYPE_SIZE_T,
//!    Fiddle::TYPE_INT, Fiddle::TYPE_VOIDP], Fiddle::TYPE_VOIDP)
//! ```
//!
//! ## Safety model
//!
//! Every entry point is `pub extern "C"` with a documented SAFETY
//! contract; panics never cross the boundary (`catch_unwind` at
//! every entry); errors surface as `NULL` returns plus
//! [`ozip_last_error`]. Buffer ownership is explicit: returned
//! buffers are Rust-allocated and MUST be freed by [`ozip_free`] —
//! nothing else may touch them.
//!
//! ## Determinism
//!
//! The codecs' contracts carry through: same input + level ⇒ same
//! bytes, on every machine — the tier swap in the gem cannot
//! disturb downstream content addressing.

// The workspace `forbid` cannot be scoped; this crate keeps the
// strictest enforceable form and allows it ONLY in the `shim`
// module, where each unsafe block carries its contract.
#![deny(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use std::cell::RefCell;
use std::ffi::c_char;
use std::ffi::CString;

use omnizip_codecs::level::CompressionLevel;
use omnizip_codecs::{Codec, OmnizipError};

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::new("no error").expect("static"));
}

fn set_last_error(msg: String) {
    let sanitized = msg.replace('\0', " ");
    LAST_ERROR.with(|e| {
        *e.borrow_mut() =
            CString::new(sanitized).unwrap_or_else(|_| CString::new("error").expect("static"));
    });
}

fn codec_by_name(name: &str) -> Result<Box<dyn Codec>, String> {
    match name {
        "zstd" => Ok(Box::new(omnizip_zstd::ZstdCodec)),
        "bzip2" => Ok(Box::new(omnizip_bzip2::Bzip2Codec)),
        "lzma" | "xz" => Ok(Box::new(omnizip_lzma::LzmaCodec)),
        "deflate" => Ok(Box::new(RawDeflateAdapter)),
        "deflate64" => Ok(Box::new(omnizip_deflate64::Deflate64Codec)),
        "lzma-alone" => Ok(Box::new(LzmaAloneAdapter)),
        "lzip" => Ok(Box::new(LzipAdapter)),
        "zlib" => Ok(Box::new(omnizip_libdeflate::LibdeflateCodec)),
        "gzip" => Ok(Box::new(GzipAdapter)),
        name if name.starts_with("ppmd7:") || name.starts_with("ppmd8:") => {
            let (variant, order, mem) = parse_ppmd_name(name)?;
            match variant {
                7 => Ok(Box::new(PpmdAdapter7 { order, mem })),
                _ => Ok(Box::new(PpmdAdapter8 { order, mem })),
            }
        }
        other => Err(format!(
            "unknown codec '{other}' (available: zstd, bzip2, lzma, xz, deflate, deflate64, lzma-alone, lzip, zlib, gzip, ppmd7:*, ppmd8:*)"
        )),
    }
}

/// The error from the most recent FFI call on this thread, as a
/// NUL-terminated C string valid until the next call. Never NULL.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn ozip_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

/// The library's semver (`env!("CARGO_PKG_VERSION")`) as a
/// NUL-terminated C string. Static for the process lifetime — the
/// gem's platform-gem smoke gate asserts the vendored dylib matches
/// the gem's expected Rust release.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn ozip_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// Free a buffer returned by `ozip_compress`/`ozip_decompress`.
///
/// # Safety
///
/// `buf` must have been returned by this library on the same
/// thread-era allocation (Rust global allocator), `len` must be the
/// length reported at return time, and the buffer must not have
/// been freed already. Passing NULL is a no-op.
#[allow(unsafe_code)]
#[no_mangle]
pub unsafe extern "C" fn ozip_free(buf: *mut u8, len: usize) {
    if buf.is_null() {
        return;
    }
    // SAFETY: the caller satisfies the contract above — the pointer
    // came from Vec::into_raw_parts (below) with this exact length,
    // and ownership transfers back here exactly once.
    drop(unsafe { Vec::from_raw_parts(buf, len, len) });
}

/// Compress `input` (`input_len` bytes) with `codec` at `level`.
/// Returns a Rust-allocated buffer and writes its length through
/// `out_len`, or NULL on error (see `ozip_last_error`).
///
/// # Safety
///
/// `codec` must be a readable NUL-terminated string; `input` must
/// be readable for `input_len` bytes; `out_len` must be writable.
/// The returned buffer must be freed with `ozip_free`.
#[allow(unsafe_code)]
#[no_mangle]
pub unsafe extern "C" fn ozip_compress(
    codec: *const c_char,
    input: *const u8,
    input_len: usize,
    level: u8,
    out_len: *mut usize,
) -> *mut u8 {
    // SAFETY: caller contract above.
    #[allow(unsafe_code)]
    match unsafe { try_compress(codec, input, input_len, level, out_len) } {
        Ok(ptr) => ptr,
        Err(()) => std::ptr::null_mut(),
    }
}

#[allow(unsafe_code)]
unsafe fn try_compress(
    codec: *const c_char,
    input: *const u8,
    input_len: usize,
    level: u8,
    out_len: *mut usize,
) -> Result<*mut u8, ()> {
    let name = unsafe { std::ffi::CStr::from_ptr(codec) }
        .to_string_lossy()
        .into_owned();
    let data = if input_len == 0 {
        &[][..]
    } else {
        // SAFETY: caller guarantees readability for input_len.
        unsafe { std::slice::from_raw_parts(input, input_len) }
    };
    let result = std::panic::catch_unwind(|| {
        codec_by_name(&name).and_then(|c| {
            c.compress(data, CompressionLevel::new(level))
                .map_err(|e| e.to_string())
        })
    });
    match result {
        Ok(Ok(out)) => {
            unsafe { *out_len = out.len() };
            Ok(ozip_take(out))
        }
        Ok(Err(msg)) => {
            set_last_error(msg);
            Err(())
        }
        Err(_) => {
            set_last_error("codec panicked".into());
            Err(())
        }
    }
}

/// Decompress `input` (`input_len` bytes). `expected_len` is the
/// exact plaintext size (the codecs verify it); use the value
/// recorded at compress time. Returns a buffer + length via
/// `out_len`, or NULL on error.
///
/// # Safety
///
/// Same contract as `ozip_compress`.
#[allow(unsafe_code)]
#[no_mangle]
pub unsafe extern "C" fn ozip_decompress(
    codec: *const c_char,
    input: *const u8,
    input_len: usize,
    expected_len: usize,
    out_len: *mut usize,
) -> *mut u8 {
    #[allow(unsafe_code)]
    match unsafe { try_decompress(codec, input, input_len, expected_len, out_len) } {
        Ok(ptr) => ptr,
        Err(()) => std::ptr::null_mut(),
    }
}

#[allow(unsafe_code)]
unsafe fn try_decompress(
    codec: *const c_char,
    input: *const u8,
    input_len: usize,
    expected_len: usize,
    out_len: *mut usize,
) -> Result<*mut u8, ()> {
    let name = unsafe { std::ffi::CStr::from_ptr(codec) }
        .to_string_lossy()
        .into_owned();
    let data = if input_len == 0 {
        &[][..]
    } else {
        // SAFETY: caller guarantees readability for input_len.
        unsafe { std::slice::from_raw_parts(input, input_len) }
    };
    // expected_len == usize::MAX means "unknown": streaming
    // callers (the Ruby tier) cannot know the plaintext size. The
    // Codec trait enforces exact lengths, so this routes to each
    // codec's length-agnostic free function instead.
    let result = if expected_len == usize::MAX {
        std::panic::catch_unwind(|| {
            match name.as_str() {
            "zstd" => omnizip_zstd::decompress(data, u32::MAX).map_err(|e| e.to_string()),
            "bzip2" => omnizip_bzip2::decompress_framed(data).map_err(|e| e.to_string()),
            "lzma" | "xz" => omnizip_lzma::xz_decompress(data).map_err(|e| e.to_string()),
            "deflate" => omnizip_libdeflate::decompress_raw_unknown_len(data)
                .map_err(|e| e.to_string()),
            "deflate64" => omnizip_deflate64::Deflate64Codec
                .decompress(data, u32::MAX)
                .map_err(|e| e.to_string()),
            "lzma-alone" => omnizip_lzma::lzma_alone_decompress(data).map_err(|e| e.to_string()),
            "lzip" => omnizip_lzma::lzip_decompress(data).map_err(|e| e.to_string()),
            "zlib" => omnizip_libdeflate::decompress_zlib_unknown_len(data)
                .map_err(|e| e.to_string()),
            "gzip" => omnizip_archive_core::formats::gzip::decompress(data).map_err(|e| e.to_string()),
            name if name.starts_with("ppmd7:") || name.starts_with("ppmd8:") => {
                let (variant, _order, mem) = parse_ppmd_name(name)
                    .map_err(|e| e.to_string())?;
                // The container carries its own size (magic + order +
                // u32 LE at bytes 6..10); expected_len is a cross-check
                // there, so feed the container's value, not a sentinel.
                if data.len() < 10 {
                    return Err("ppmd container too short".to_string());
                }
                let expected =
                    u32::from_le_bytes([data[6], data[7], data[8], data[9]]) as usize;
                match variant {
                    7 => omnizip_ppmd::ppmd7::codec::decompress_with_budget(
                        data, expected, mem,
                    )
                    .map_err(|e| e.to_string()),
                    _ => omnizip_ppmd::ppmd8::codec::decompress_with_budget(
                        data, expected, mem,
                    )
                    .map_err(|e| e.to_string()),
                }
            }
            other => Err(format!(
                "unknown codec '{other}' (available: zstd, bzip2, lzma, xz, deflate, deflate64, lzma-alone, lzip, zlib, gzip, ppmd7:*, ppmd8:*)"
            )),
        }
        })
    } else {
        let Ok(expected) = u32::try_from(expected_len) else {
            set_last_error(format!("expected_len {expected_len} exceeds u32"));
            return Err(());
        };
        std::panic::catch_unwind(|| {
            codec_by_name(&name).and_then(|c| {
                c.decompress(data, expected)
                    .map_err(|e: OmnizipError| e.to_string())
            })
        })
    };
    match result {
        Ok(Ok(out)) => {
            unsafe { *out_len = out.len() };
            Ok(ozip_take(out))
        }
        Ok(Err(msg)) => {
            set_last_error(msg);
            Err(())
        }
        Err(_) => {
            set_last_error("codec panicked".into());
            Err(())
        }
    }
}

/// Hand a Vec's buffer to the caller: leaks capacity semantics are
/// fine — `ozip_free` reconstructs the Vec with len == capacity.
fn ozip_take(mut v: Vec<u8>) -> *mut u8 {
    let ptr = v.as_mut_ptr();
    let len = v.len();
    let cap = v.capacity();
    // Normalization: reallocating to cap == len guarantees
    // `Vec::from_raw_parts(buf, len, len)` in `ozip_free` is always
    // sound, whatever the growth history was.
    if cap != len {
        let exact = v.clone();
        drop(v);
        return ozip_take(exact);
    }
    std::mem::forget(v);
    ptr
}

/// An opened archive: one `ArchiveReader` (any supported format,
/// auto-detected) plus its materialized entry list. Opaque handle for
/// the archive-level FFI surface — the gem's whole-archive
/// operations (list entries, read entry) ride this instead of
/// decoding entry-by-entry through codec names.
pub struct ArchHandle {
    reader: Box<dyn omnizip_archive_core::ArchiveReader>,
    entries: Vec<omnizip_archive_core::ArchiveEntry>,
    last_name: CString,
}

fn open_archive_reader(
    data: &[u8],
    password: Option<&str>,
) -> Result<Box<dyn omnizip_archive_core::ArchiveReader>, String> {
    use omnizip_archive_core::detect::{detect_format, FormatKind};
    let pw = password;
    match detect_format(data) {
        FormatKind::Zip => Ok(Box::new(
            omnizip_zip::ZipReader::from_bytes(data).map_err(|e| e.to_string())?,
        )),
        FormatKind::Tar => Ok(Box::new(
            omnizip_tar::TarReader::from_bytes(data).map_err(|e| e.to_string())?,
        )),
        FormatKind::Cpio => Ok(Box::new(
            omnizip_cpio::CpioReader::from_bytes(data).map_err(|e| e.to_string())?,
        )),
        FormatKind::SevenZip => Ok(Box::new(
            omnizip_sevenzip::reader::SevenZipReader::from_bytes_with_password(data, pw)
                .map_err(|e| e.to_string())?,
        )),
        FormatKind::Rar5 => match pw {
            Some(p) => omnizip_rar::rar5::Rar5Reader::from_bytes_with_password(data, p)
                .map(|r| Box::new(r) as Box<dyn omnizip_archive_core::ArchiveReader>)
                .map_err(|e| e.to_string()),
            None => omnizip_rar::rar5::Rar5Reader::from_bytes(data)
                .map(|r| Box::new(r) as Box<dyn omnizip_archive_core::ArchiveReader>)
                .map_err(|e| e.to_string()),
        },
        FormatKind::Rar4 => match pw {
            Some(p) => omnizip_rar::rar3::Rar4Reader::from_bytes_with_password(data, p)
                .map(|r| Box::new(r) as Box<dyn omnizip_archive_core::ArchiveReader>)
                .map_err(|e| e.to_string()),
            None => omnizip_rar::rar3::Rar4Reader::from_bytes(data)
                .map(|r| Box::new(r) as Box<dyn omnizip_archive_core::ArchiveReader>)
                .map_err(|e| e.to_string()),
        },
        // ISO/RPM/XAR/OLE have magic sniffing in their own crates'
        // readers; detect_format's enum predates them. Try them in a
        // fixed order for kinds the enum cannot express.
        _ => {
            for probe in [
                omnizip_iso::reader::IsoReader::from_bytes(data)
                    .ok()
                    .map(|r| Box::new(r) as Box<dyn omnizip_archive_core::ArchiveReader>),
                omnizip_rpm::reader::RpmReader::from_bytes(data)
                    .ok()
                    .map(|r| Box::new(r) as Box<dyn omnizip_archive_core::ArchiveReader>),
                omnizip_xar::reader::XarReader::from_bytes(data)
                    .ok()
                    .map(|r| Box::new(r) as Box<dyn omnizip_archive_core::ArchiveReader>),
                omnizip_ole::reader::OleReader::from_bytes(data)
                    .ok()
                    .map(|r| Box::new(r) as Box<dyn omnizip_archive_core::ArchiveReader>),
            ] {
                if let Some(reader) = probe {
                    return Ok(reader);
                }
            }
            Err("unsupported archive format kind".into())
        }
    }
}

/// Open an archive from raw bytes (format auto-detected; optional
/// NUL-terminated password). Returns NULL on error
/// ([`ozip_last_error`]). Free with [`ozip_arch_close`].
#[allow(unsafe_code)]
#[no_mangle]
pub unsafe extern "C" fn ozip_arch_open(
    data: *const u8,
    len: usize,
    password: *const c_char,
) -> *mut ArchHandle {
    #[allow(unsafe_code)]
    match unsafe { try_arch_open(data, len, password) } {
        Ok(h) => Box::into_raw(h),
        Err(()) => std::ptr::null_mut(),
    }
}

#[allow(unsafe_code)]
unsafe fn try_arch_open(
    data: *const u8,
    len: usize,
    password: *const c_char,
) -> Result<Box<ArchHandle>, ()> {
    if data.is_null() {
        set_last_error("arch data pointer is null".into());
        return Err(());
    }
    // SAFETY: caller guarantees readability for `len` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    let pw = if password.is_null() {
        None
    } else {
        // SAFETY: caller guarantees NUL termination.
        Some(
            unsafe { std::ffi::CStr::from_ptr(password) }
                .to_string_lossy()
                .into_owned(),
        )
    };
    let result = std::panic::catch_unwind(|| {
        let mut reader = open_archive_reader(bytes, pw.as_deref())?;
        let entries = reader.entries().map_err(|e| format!("entries: {e}"))?;
        Ok::<_, String>(ArchHandle {
            reader,
            entries,
            last_name: CString::new("no entry").expect("static"),
        })
    });
    match result {
        Ok(Ok(mut h)) => {
            h.last_name = CString::new("ok").expect("static");
            Ok(Box::new(h))
        }
        Ok(Err(msg)) => {
            set_last_error(msg);
            Err(())
        }
        Err(_) => {
            set_last_error("archive open panicked".into());
            Err(())
        }
    }
}

/// Number of entries in the opened archive. 0 on NULL handle.
#[allow(unsafe_code)]
#[no_mangle]
pub unsafe extern "C" fn ozip_arch_count(handle: *const ArchHandle) -> usize {
    if handle.is_null() {
        return 0;
    }
    // SAFETY: handle came from ozip_arch_open and was not closed.
    (unsafe { &*handle }).entries.len()
}

/// Entry name (NUL-terminated, valid until the next call on this
/// handle). NULL on out-of-range index.
#[allow(unsafe_code)]
#[no_mangle]
pub unsafe extern "C" fn ozip_arch_entry_name(
    handle: *mut ArchHandle,
    index: usize,
) -> *const c_char {
    if handle.is_null() {
        return std::ptr::null();
    }
    // SAFETY: handle came from ozip_arch_open and was not closed.
    let h = unsafe { &mut *handle };
    match h.entries.get(index) {
        Some(e) => {
            h.last_name = CString::new(e.name.clone())
                .unwrap_or_else(|_| CString::new("bad name").expect("static"));
            h.last_name.as_ptr()
        }
        None => std::ptr::null(),
    }
}

/// Entry uncompressed size (u64). 0 on out-of-range.
#[allow(unsafe_code)]
#[no_mangle]
pub unsafe extern "C" fn ozip_arch_entry_size(handle: *const ArchHandle, index: usize) -> u64 {
    if handle.is_null() {
        return 0;
    }
    // SAFETY: handle from ozip_arch_open.
    (unsafe { &*handle })
        .entries
        .get(index)
        .and_then(|e| e.size)
        .unwrap_or(0)
}

/// Read one entry's uncompressed bytes; free the buffer with
/// [`ozip_free`]. NULL on error.
#[allow(unsafe_code)]
#[no_mangle]
pub unsafe extern "C" fn ozip_arch_read_entry(
    handle: *mut ArchHandle,
    index: usize,
    out_len: *mut usize,
) -> *mut u8 {
    #[allow(unsafe_code)]
    match unsafe { try_arch_read_entry(handle, index, out_len) } {
        Ok(ptr) => ptr,
        Err(()) => std::ptr::null_mut(),
    }
}

#[allow(unsafe_code)]
unsafe fn try_arch_read_entry(
    handle: *mut ArchHandle,
    index: usize,
    out_len: *mut usize,
) -> Result<*mut u8, ()> {
    if handle.is_null() || out_len.is_null() {
        set_last_error("arch read: null handle or out_len".into());
        return Err(());
    }
    // SAFETY: handle from ozip_arch_open.
    let h = unsafe { &mut *handle };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        h.reader.read_entry(index).map_err(|e| e.to_string())
    }));
    match result {
        Ok(Ok(out)) => {
            // SAFETY: caller-provided writable slot.
            unsafe { *out_len = out.len() };
            Ok(ozip_take(out))
        }
        Ok(Err(msg)) => {
            set_last_error(msg);
            Err(())
        }
        Err(_) => {
            set_last_error("entry read panicked".into());
            Err(())
        }
    }
}

/// Close an archive handle (freeing it). NULL is a no-op.
#[allow(unsafe_code)]
#[no_mangle]
pub unsafe extern "C" fn ozip_arch_close(handle: *mut ArchHandle) {
    if !handle.is_null() {
        // SAFETY: ownership transfers back exactly once.
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// `ppmd7:o{order}:m{mem}` / `ppmd8:o{order}:m{mem}` — the gem's
/// PPMd algorithms carry their parameters in the codec name (the FFI
/// ABI has no params argument); `mem` is bytes.
fn parse_ppmd_name(name: &str) -> Result<(u8, u8, usize), String> {
    let bad = || format!("invalid ppmd codec name '{name}'");
    let mut parts = name.split(':');
    let variant = match parts.next() {
        Some("ppmd7") => 7,
        Some("ppmd8") => 8,
        _ => return Err(bad()),
    };
    let order = parts
        .next()
        .and_then(|p| p.strip_prefix('o'))
        .ok_or_else(bad)?;
    let mem = parts
        .next()
        .and_then(|p| p.strip_prefix('m'))
        .ok_or_else(bad)?;
    if parts.next().is_some() {
        return Err(bad());
    }
    let order: u8 = order.parse().map_err(|_| bad())?;
    let mem: usize = mem.parse().map_err(|_| bad())?;
    Ok((variant, order, mem))
}

struct PpmdAdapter7 {
    order: u8,
    mem: usize,
}

impl Codec for PpmdAdapter7 {
    fn id(&self) -> omnizip_codecs::CodecId {
        omnizip_codecs::CodecId::PPMD7
    }
    fn name(&self) -> &'static str {
        "ppmd7"
    }
    fn compress(
        &self,
        plaintext: &[u8],
        _level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        omnizip_ppmd::ppmd7::codec::compress_with_budget(plaintext, self.order, self.mem)
            .map_err(map_ppmd_err)
    }
    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        omnizip_ppmd::ppmd7::codec::decompress_with_budget(
            compressed,
            expected_len as usize,
            self.mem,
        )
        .map_err(map_ppmd_err)
    }
}

struct PpmdAdapter8 {
    order: u8,
    mem: usize,
}

impl Codec for PpmdAdapter8 {
    fn id(&self) -> omnizip_codecs::CodecId {
        omnizip_codecs::CodecId::PPMD8
    }
    fn name(&self) -> &'static str {
        "ppmd8"
    }
    fn compress(
        &self,
        plaintext: &[u8],
        _level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        omnizip_ppmd::ppmd8::codec::compress_with_budget(plaintext, self.order, self.mem)
            .map_err(map_ppmd_err)
    }
    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        omnizip_ppmd::ppmd8::codec::decompress_with_budget(
            compressed,
            expected_len as usize,
            self.mem,
        )
        .map_err(map_ppmd_err)
    }
}

fn map_ppmd_err(e: impl std::fmt::Display) -> OmnizipError {
    OmnizipError::DecodeFailed {
        codec: omnizip_codecs::CodecId::PPMD8,
        reason: e.to_string(),
    }
}

/// The gzip container (RFC 1952): header + raw deflate + CRC/ISIZE
/// trailer — the Ruby gzip format wraps Zlib streams, and this name
/// accelerates that whole-container path.
struct GzipAdapter;

impl Codec for GzipAdapter {
    fn id(&self) -> omnizip_codecs::CodecId {
        omnizip_codecs::CodecId::DEFLATE
    }
    fn name(&self) -> &'static str {
        "gzip"
    }
    fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        let opts = omnizip_archive_core::formats::gzip::GzipOptions {
            level: level.as_u8().min(9),
            ..Default::default()
        };
        omnizip_archive_core::formats::gzip::compress(plaintext, &opts).map_err(map_gzip_err)
    }
    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let out =
            omnizip_archive_core::formats::gzip::decompress(compressed).map_err(map_gzip_err)?;
        let want = usize::try_from(expected_len).unwrap_or(usize::MAX);
        if out.len() != want {
            return Err(OmnizipError::LengthMismatch {
                codec: omnizip_codecs::CodecId::DEFLATE,
                expected: expected_len,
                actual: out.len(),
            });
        }
        Ok(out)
    }
}

fn map_gzip_err(e: omnizip_archive_core::ArchiveError) -> OmnizipError {
    OmnizipError::DecodeFailed {
        codec: omnizip_codecs::CodecId::DEFLATE,
        reason: e.to_string(),
    }
}

/// The Ruby gem's Deflate algorithm speaks RAW RFC 1951 (its gzip
/// format wraps the same raw stream in Ruby) — distinct from
/// `DeflateCodec`, which wraps in zlib.
struct RawDeflateAdapter;

impl Codec for RawDeflateAdapter {
    fn id(&self) -> omnizip_codecs::CodecId {
        omnizip_codecs::CodecId::DEFLATE
    }
    fn name(&self) -> &'static str {
        "deflate"
    }
    fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        omnizip_libdeflate::compress_raw(plaintext, level.as_u8())
    }
    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let out = omnizip_libdeflate::decompress_raw_unknown_len(compressed)?;
        let want = usize::try_from(expected_len).unwrap_or(usize::MAX);
        if out.len() != want {
            return Err(OmnizipError::LengthMismatch {
                codec: omnizip_codecs::CodecId::DEFLATE,
                expected: expected_len,
                actual: out.len(),
            });
        }
        Ok(out)
    }
}

/// The Ruby gem's LZMA algorithm emits LZMA1-alone streams (the
/// 13-byte header carries lc/lp/pb/dict) — distinct from the XZ
/// container that `LzmaCodec` speaks, hence a dedicated FFI name.
struct LzmaAloneAdapter;

impl Codec for LzmaAloneAdapter {
    fn id(&self) -> omnizip_codecs::CodecId {
        omnizip_codecs::CodecId::LZMA
    }
    fn name(&self) -> &'static str {
        "lzma-alone"
    }
    fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
        let lv = level.as_u8().min(9);
        let dict_size: u32 = [
            1 << 16,
            1 << 20,
            1 << 21,
            1 << 22,
            1 << 22,
            1 << 23,
            1 << 23,
            1 << 24,
            1 << 25,
            1 << 26,
        ][usize::from(lv)];
        let opts = omnizip_lzma::LzmaOptions {
            dict_size,
            ..Default::default()
        };
        omnizip_lzma::lzma_alone_compress_with_options(plaintext, &opts).map_err(map_lzma_err)
    }
    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let out = omnizip_lzma::lzma_alone_decompress(compressed).map_err(map_lzma_err)?;
        check_expected(out.len(), expected_len, "lzma-alone")?;
        Ok(out)
    }
}

/// lzip member decode (the trailer carries the plaintext size);
/// compression is not offered through the FFI.
struct LzipAdapter;

impl Codec for LzipAdapter {
    fn id(&self) -> omnizip_codecs::CodecId {
        omnizip_codecs::CodecId::LZMA
    }
    fn name(&self) -> &'static str {
        "lzip"
    }
    fn compress(
        &self,
        _plaintext: &[u8],
        _level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        Err(OmnizipError::Unsupported {
            codec: omnizip_codecs::CodecId::LZMA,
            reason: "lzip compression is not available through the FFI".into(),
        })
    }
    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let out = omnizip_lzma::lzip_decompress(compressed).map_err(map_lzma_err)?;
        check_expected(out.len(), expected_len, "lzip")?;
        Ok(out)
    }
}

fn map_lzma_err(e: omnizip_lzma::LzmaError) -> OmnizipError {
    OmnizipError::DecodeFailed {
        codec: omnizip_codecs::CodecId::LZMA,
        reason: e.to_string(),
    }
}

fn check_expected(got: usize, expected: u32, codec: &str) -> Result<(), OmnizipError> {
    let want = usize::try_from(expected).unwrap_or(usize::MAX);
    if got != want {
        return Err(OmnizipError::Corrupt {
            codec: omnizip_codecs::CodecId::LZMA,
            reason: format!("{codec}: expected {want} bytes, decoded {got}"),
        });
    }
    Ok(())
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn version_is_nul_terminated_semver() {
        let p = ozip_version();
        assert!(!p.is_null());
        let s = unsafe { std::ffi::CStr::from_ptr(p) }
            .to_string_lossy()
            .into_owned();
        assert_eq!(s, env!("CARGO_PKG_VERSION"));
        assert!(
            s.split('.').count() >= 3,
            "semver, not a pointer-to-garbage"
        );
    }

    fn roundtrip(codec_name: &str, level: u8, data: &[u8]) {
        let name = CString::new(codec_name).unwrap();
        let mut out_len = 0_usize;
        let ptr = unsafe {
            ozip_compress(
                name.as_ptr(),
                data.as_ptr(),
                data.len(),
                level,
                &mut out_len,
            )
        };
        assert!(
            !ptr.is_null(),
            "{} compress failed: {}",
            codec_name,
            unsafe { std::ffi::CStr::from_ptr(ozip_last_error()).to_string_lossy() }
        );
        let mut dec_len = 0_usize;
        let dec = unsafe { ozip_decompress(name.as_ptr(), ptr, out_len, data.len(), &mut dec_len) };
        assert!(
            !dec.is_null(),
            "{} decompress failed: {}",
            codec_name,
            unsafe { std::ffi::CStr::from_ptr(ozip_last_error()).to_string_lossy() }
        );
        let restored = unsafe { std::slice::from_raw_parts(dec, dec_len) };
        assert_eq!(restored, data, "{codec_name} roundtrip");
        unsafe { ozip_free(ptr, out_len) };
        unsafe { ozip_free(dec, dec_len) };
    }

    fn sample(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| u8::try_from(i % 251).expect("<251"))
            .collect()
    }

    #[test]
    fn roundtrips_all_codecs() {
        roundtrip("zstd", 6, &sample(50_000));
        roundtrip("bzip2", 9, &sample(50_000));
        roundtrip("lzma", 6, &sample(50_000));
        roundtrip("xz", 6, &sample(1000)); // alias
        roundtrip("deflate", 6, &sample(50_000));
        roundtrip("deflate64", 6, &sample(50_000));
        roundtrip("lzma-alone", 6, &sample(50_000));
        roundtrip("zlib", 6, &sample(50_000));
        roundtrip("gzip", 6, &sample(50_000));
        // Param-carrying PPMd names (order + mem bytes in the name).
        roundtrip("ppmd7:o6:m16777216", 0, &sample(20_000));
        roundtrip("ppmd8:o6:m16777216", 0, &sample(20_000));
    }

    #[test]
    fn lzip_decodes_unknown_length() {
        use std::ffi::CStr;
        // The good-1-v1 fixture from the lzip conformance set (the
        // same corpus omnizip-lzma's own lzip tests decode).
        let lzip_stream = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/lzma/good-1-v1.lz"
        ))
        .unwrap();
        let expected = omnizip_lzma::lzip_decompress(&lzip_stream).unwrap();
        assert!(!expected.is_empty());
        let name = CString::new("lzip").unwrap();
        let mut dec_len = 0_usize;
        let dec = unsafe {
            ozip_decompress(
                name.as_ptr(),
                lzip_stream.as_ptr(),
                lzip_stream.len(),
                usize::MAX,
                &mut dec_len,
            )
        };
        assert!(!dec.is_null(), "lzip decode failed: {}", unsafe {
            CStr::from_ptr(ozip_last_error()).to_string_lossy()
        });
        let restored = unsafe { std::slice::from_raw_parts(dec, dec_len) };
        assert_eq!(restored, &expected[..]);
        unsafe { ozip_free(dec, dec_len) };
    }

    #[test]
    fn unknown_length_paths_cover_new_codecs() {
        use std::ffi::CStr;
        for (name, encoded) in [
            (
                "deflate",
                omnizip_libdeflate::compress_raw(&sample(20_000), 6).unwrap(),
            ),
            (
                "deflate64",
                omnizip_deflate64::Deflate64Codec
                    .compress(&sample(20_000), CompressionLevel::new(6))
                    .unwrap(),
            ),
            (
                "lzma-alone",
                omnizip_lzma::lzma_alone_compress_with_options(
                    &sample(20_000),
                    &omnizip_lzma::LzmaOptions::default(),
                )
                .unwrap(),
            ),
            (
                "zlib",
                omnizip_libdeflate::LibdeflateCodec
                    .compress(&sample(20_000), CompressionLevel::new(6))
                    .unwrap(),
            ),
            ("gzip", {
                let opts = omnizip_archive_core::formats::gzip::GzipOptions::default();
                omnizip_archive_core::formats::gzip::compress(&sample(20_000), &opts).unwrap()
            }),
        ] {
            let cname = CString::new(name).unwrap();
            let mut dec_len = 0_usize;
            let dec = unsafe {
                ozip_decompress(
                    cname.as_ptr(),
                    encoded.as_ptr(),
                    encoded.len(),
                    usize::MAX,
                    &mut dec_len,
                )
            };
            assert!(
                !dec.is_null(),
                "{name} unknown-length decode failed: {}",
                unsafe { CStr::from_ptr(ozip_last_error()).to_string_lossy() }
            );
            let restored = unsafe { std::slice::from_raw_parts(dec, dec_len) };
            assert_eq!(restored.len(), 20_000, "{name} length");
            unsafe { ozip_free(dec, dec_len) };
        }
    }

    #[test]
    fn empty_and_tiny_inputs() {
        roundtrip("zstd", 3, &[]);
        roundtrip("bzip2", 1, b"x");
    }

    fn zip_archive() -> Vec<u8> {
        use omnizip_archive_core::{NewEntry, WriteOptions};
        use omnizip_zip::{ZipMethod, ZipWriter};
        let mut w = ZipWriter::new();
        let opts = WriteOptions::default();
        let body: Vec<u8> = (0..30_000u32).map(|i| (i % 251) as u8).collect();
        let p = ZipWriter::prepare(ZipMethod::Deflate, &body).unwrap();
        w.add_file_prepared(&NewEntry::file("a.bin", &opts), &p, &opts)
            .unwrap();
        let p2 = ZipWriter::prepare(ZipMethod::Deflate, b"hello archive tier").unwrap();
        w.add_file_prepared(&NewEntry::file("b.txt", &opts), &p2, &opts)
            .unwrap();
        w.finish_bytes().unwrap()
    }

    #[test]
    fn archive_handle_round_trips() {
        use std::ffi::CStr;
        let bytes = zip_archive();
        let h = unsafe { ozip_arch_open(bytes.as_ptr(), bytes.len(), std::ptr::null()) };
        assert!(!h.is_null(), "arch open failed: {}", unsafe {
            CStr::from_ptr(ozip_last_error()).to_string_lossy()
        });
        assert_eq!(unsafe { ozip_arch_count(h) }, 2);
        let n = unsafe { ozip_arch_entry_name(h, 0) };
        assert_eq!(unsafe { CStr::from_ptr(n) }.to_bytes(), b"a.bin");
        assert_eq!(unsafe { ozip_arch_entry_size(h, 0) }, 30_000);
        let mut out_len = 0usize;
        let buf = unsafe { ozip_arch_read_entry(h, 0, &mut out_len) };
        assert!(!buf.is_null());
        assert_eq!(out_len, 30_000);
        let restored = unsafe { std::slice::from_raw_parts(buf, out_len) };
        assert_eq!(restored.len(), 30_000);
        unsafe { ozip_free(buf, out_len) };
        let mut l2 = 0usize;
        let b2 = unsafe { ozip_arch_read_entry(h, 1, &mut l2) };
        assert_eq!(
            unsafe { std::slice::from_raw_parts(b2, l2) },
            b"hello archive tier"
        );
        unsafe { ozip_free(b2, l2) };
        unsafe { ozip_arch_close(h) };
    }

    #[test]
    fn archive_handle_fuzz_no_panics() {
        let bytes = zip_archive();
        let mut rng = 0xC0DE_AAAAu64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        for case in 0..600 {
            let mut input = bytes.clone();
            let flips = 1 + (next() % 8) as usize;
            for _ in 0..flips {
                let pos = (next() as usize) % input.len();
                input[pos] ^= (next() % 255 + 1) as u8;
            }
            let r = std::panic::catch_unwind(|| {
                let h = unsafe { ozip_arch_open(input.as_ptr(), input.len(), std::ptr::null()) };
                if !h.is_null() {
                    unsafe {
                        let count = ozip_arch_count(h);
                        for i in 0..count.min(4) {
                            let mut l = 0usize;
                            let b = ozip_arch_read_entry(h, i, &mut l);
                            if !b.is_null() {
                                ozip_free(b, l);
                            }
                        }
                        ozip_arch_close(h);
                    }
                }
            });
            assert!(r.is_ok(), "case {case} panicked");
        }
    }

    #[test]
    fn unknown_codec_sets_last_error() {
        let name = CString::new("nope").unwrap();
        let mut out_len = 0_usize;
        let ptr = unsafe { ozip_compress(name.as_ptr(), b"".as_ptr(), 0, 1, &mut out_len) };
        assert!(ptr.is_null());
        let err = unsafe { std::ffi::CStr::from_ptr(ozip_last_error()) }
            .to_string_lossy()
            .into_owned();
        assert!(err.contains("unknown codec 'nope'"), "{err}");
        assert!(err.contains("zstd"), "{err}");
    }

    #[test]
    fn unknown_length_decodes_all_codecs() {
        for codec in ["zstd", "bzip2", "lzma"] {
            let name = CString::new(codec).unwrap();
            let data = sample(5000);
            let mut out_len = 0_usize;
            let ptr =
                unsafe { ozip_compress(name.as_ptr(), data.as_ptr(), data.len(), 6, &mut out_len) };
            assert!(!ptr.is_null());
            let mut dec_len = 0_usize;
            let dec =
                unsafe { ozip_decompress(name.as_ptr(), ptr, out_len, usize::MAX, &mut dec_len) };
            assert!(!dec.is_null(), "{codec}: {}", unsafe {
                std::ffi::CStr::from_ptr(ozip_last_error()).to_string_lossy()
            });
            assert_eq!(
                unsafe { std::slice::from_raw_parts(dec, dec_len) },
                data.as_slice(),
                "{codec} unknown-length decode"
            );
            unsafe { ozip_free(ptr, out_len) };
            unsafe { ozip_free(dec, dec_len) };
        }
    }

    #[test]
    fn wrong_expected_len_errors() {
        let name = CString::new("zstd").unwrap();
        let data = sample(1000);
        let mut out_len = 0_usize;
        let ptr =
            unsafe { ozip_compress(name.as_ptr(), data.as_ptr(), data.len(), 3, &mut out_len) };
        assert!(!ptr.is_null());
        let mut dec_len = 0_usize;
        let dec = unsafe { ozip_decompress(name.as_ptr(), ptr, out_len, 999, &mut dec_len) };
        assert!(dec.is_null());
        let err = unsafe { std::ffi::CStr::from_ptr(ozip_last_error()) }
            .to_string_lossy()
            .into_owned();
        assert!(
            err.to_lowercase().contains("length") || err.to_lowercase().contains("size"),
            "{err}"
        );
        unsafe { ozip_free(ptr, out_len) };
    }
}
