# Ruby⇄Rust acceleration smoke: the gem's future `rust` tier calling
# the omnizip-ffi cdylib through stdlib Fiddle.
require 'fiddle'
require 'fiddle/import'

module OmnizipRust
  extend Fiddle::Importer
  dylib = ENV['OMNIZIP_FFI_DYLIB'] ||
         File.expand_path('../../target/release/libomnizip_ffi.dylib', __dir__)
dlload dylib

  extern 'char* ozip_last_error()'
  extern 'void ozip_free(void*, size_t)'
  extern 'void* ozip_compress(const char*, const void*, size_t, uint8_t, size_t*)'
  extern 'void* ozip_decompress(const char*, const void*, size_t, size_t, size_t*)'

  def self.compress(codec, data, level)
    out_len = [0].pack('Q')
    ptr = ozip_compress(codec, data, data.bytesize, level, out_len)
    raise ozip_last_error_s if ptr.null?
    len = out_len.unpack1('Q')
    buf = ptr.to_s(len)  # copy into a Ruby String
    ozip_free(ptr, len)
    buf
  end

  def self.decompress(codec, data, expected_len)
    out_len = [0].pack('Q')
    ptr = ozip_decompress(codec, data, data.bytesize, expected_len, out_len)
    raise ozip_last_error_s if ptr.null?
    len = out_len.unpack1('Q')
    buf = ptr.to_s(len)
    ozip_free(ptr, len)
    buf
  end

  def self.ozip_last_error_s
    ozip_last_error.to_s
  end
end

input = (0...200_000).map { |i| (i % 251).chr }.join
%w[zstd bzip2 lzma].each do |codec|
  comp = OmnizipRust.compress(codec, input, 6)
  plain = OmnizipRust.decompress(codec, comp, input.bytesize)
  raise "#{codec} MISMATCH" unless plain == input
  puts format('%-6s %d -> %d bytes, round-trip OK', codec, input.bytesize, comp.bytesize)
end
# error surface:
begin
  OmnizipRust.compress('nope', 'x', 1)
rescue RuntimeError => e
  puts "error path: #{e.message[0, 50]}"
end
puts 'RUBY-BRIDGE-OK'
