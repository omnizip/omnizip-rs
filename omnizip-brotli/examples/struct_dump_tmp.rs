// Stream-structure dump: decodes a .br file with BROTLI_STRUCT_DUMP set.
use omnizip_codecs::Codec;
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let orig = std::env::args().nth(2).unwrap();
    let stream = std::fs::read(&path).unwrap();
    let size = std::fs::read(orig).unwrap().len() as u32;
    let codec = omnizip_brotli::BrotliCodec::new();
    match codec.decompress(&stream, size) {
        Ok(out) => {
            println!(
                "decoded {} -> {} | dict_hits={:?}",
                stream.len(),
                out.len(),
                omnizip_brotli::decoder_full::dict_hits()
            );
            for (pos, len, dist) in omnizip_brotli::decoder_full::dict_log() {
                println!("DICTMATCH pos={pos} len={len} dist={dist}");
            }
        }
        Err(e) => println!("decode error: {e:?}"),
    }
}
