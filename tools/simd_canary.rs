// portable_simd stabilization canary (task 30's reopen condition).
// Compiles on stable today ONLY when std::simd stabilizes; the CI job
// below is allowed to fail until then and must be flipped to required
// (opening the SIMD program) the day it goes green.
fn main() {
    use std::simd::u8x16;
    use std::simd::cmp::SimdPartialEq;
    let a = u8x16::splat(1);
    let b = u8x16::splat(2);
    assert_eq!(a.simd_eq(b).to_bitmask(), 0);
    println!("portable_simd is stable — task 30's gate is OPEN");
}
