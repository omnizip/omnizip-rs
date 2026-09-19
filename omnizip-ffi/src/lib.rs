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
        "zstd" => Ok(Box::new(omnizip_zstd::codec::ZstdCodec)),
        "bzip2" => Ok(Box::new(omnizip_bzip2::codec::Bzip2Codec)),
        "lzma" | "xz" => Ok(Box::new(omnizip_lzma::codec::LzmaCodec)),
        other => Err(format!(
            "unknown codec '{other}' (available: zstd, bzip2, lzma)"
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
    let expected = u32::try_from(expected_len).unwrap_or(u32::MAX);
    let result = std::panic::catch_unwind(|| {
        codec_by_name(&name).and_then(|c| {
            c.decompress(data, expected)
                .map_err(|e: OmnizipError| e.to_string())
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

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use std::ffi::CString;

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
    fn roundtrips_all_three_codecs() {
        roundtrip("zstd", 6, &sample(50_000));
        roundtrip("bzip2", 9, &sample(50_000));
        roundtrip("lzma", 6, &sample(50_000));
        roundtrip("xz", 6, &sample(1000)); // alias
    }

    #[test]
    fn empty_and_tiny_inputs() {
        roundtrip("zstd", 3, &[]);
        roundtrip("bzip2", 1, b"x");
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
