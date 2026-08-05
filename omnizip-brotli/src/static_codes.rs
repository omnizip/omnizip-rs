//! Static Huffman code tables for Brotli insert-copy commands and
//! distances (RFC 7932 §10.3 + §10.4).
//!
//! These are predefined Huffman codes used by `store_meta_block_fast`
//! in the upstream brotli encoder to avoid the per-metablock tree
//! encoding cost. The encoder uses them unchanged for the command and
//! distance alphabets; only the literal alphabet gets a custom tree.
//!
//! Ported from `brotli/src/enc/constants.rs` (BSD-3-Clause, compatible
//! with our MIT OR Apache-2.0).

#![forbid(unsafe_code)]

/// Static Huffman code depths for the 704-symbol insert-and-copy
/// alphabet. First 512 symbols get 9-bit codes; symbols 512..704 get
/// 11-bit codes. Built via const eval.
pub static K_STATIC_COMMAND_CODE_DEPTH: [u8; 704] = {
    let mut arr = [0u8; 704];
    let mut i = 0;
    while i < 512 {
        arr[i] = 9;
        i += 1;
    }
    while i < 704 {
        arr[i] = 11;
        i += 1;
    }
    arr
};

pub static K_STATIC_COMMAND_CODE_BITS: [u16; 704] = [
    0, 256, 128, 384, 64, 320, 192, 448, 32, 288, 160, 416, 96, 352, 224, 480, 
    16, 272, 144, 400, 80, 336, 208, 464, 48, 304, 176, 432, 112, 368, 240, 496, 
    8, 264, 136, 392, 72, 328, 200, 456, 40, 296, 168, 424, 104, 360, 232, 488, 
    24, 280, 152, 408, 88, 344, 216, 472, 56, 312, 184, 440, 120, 376, 248, 504, 
    4, 260, 132, 388, 68, 324, 196, 452, 36, 292, 164, 420, 100, 356, 228, 484, 
    20, 276, 148, 404, 84, 340, 212, 468, 52, 308, 180, 436, 116, 372, 244, 500, 
    12, 268, 140, 396, 76, 332, 204, 460, 44, 300, 172, 428, 108, 364, 236, 492, 
    28, 284, 156, 412, 92, 348, 220, 476, 60, 316, 188, 444, 124, 380, 252, 508, 
    2, 258, 130, 386, 66, 322, 194, 450, 34, 290, 162, 418, 98, 354, 226, 482, 
    18, 274, 146, 402, 82, 338, 210, 466, 50, 306, 178, 434, 114, 370, 242, 498, 
    10, 266, 138, 394, 74, 330, 202, 458, 42, 298, 170, 426, 106, 362, 234, 490, 
    26, 282, 154, 410, 90, 346, 218, 474, 58, 314, 186, 442, 122, 378, 250, 506, 
    6, 262, 134, 390, 70, 326, 198, 454, 38, 294, 166, 422, 102, 358, 230, 486, 
    22, 278, 150, 406, 86, 342, 214, 470, 54, 310, 182, 438, 118, 374, 246, 502, 
    14, 270, 142, 398, 78, 334, 206, 462, 46, 302, 174, 430, 110, 366, 238, 494, 
    30, 286, 158, 414, 94, 350, 222, 478, 62, 318, 190, 446, 126, 382, 254, 510, 
    1, 257, 129, 385, 65, 321, 193, 449, 33, 289, 161, 417, 97, 353, 225, 481, 
    17, 273, 145, 401, 81, 337, 209, 465, 49, 305, 177, 433, 113, 369, 241, 497, 
    9, 265, 137, 393, 73, 329, 201, 457, 41, 297, 169, 425, 105, 361, 233, 489, 
    25, 281, 153, 409, 89, 345, 217, 473, 57, 313, 185, 441, 121, 377, 249, 505, 
    5, 261, 133, 389, 69, 325, 197, 453, 37, 293, 165, 421, 101, 357, 229, 485, 
    21, 277, 149, 405, 85, 341, 213, 469, 53, 309, 181, 437, 117, 373, 245, 501, 
    13, 269, 141, 397, 77, 333, 205, 461, 45, 301, 173, 429, 109, 365, 237, 493, 
    29, 285, 157, 413, 93, 349, 221, 477, 61, 317, 189, 445, 125, 381, 253, 509, 
    3, 259, 131, 387, 67, 323, 195, 451, 35, 291, 163, 419, 99, 355, 227, 483, 
    19, 275, 147, 403, 83, 339, 211, 467, 51, 307, 179, 435, 115, 371, 243, 499, 
    11, 267, 139, 395, 75, 331, 203, 459, 43, 299, 171, 427, 107, 363, 235, 491, 
    27, 283, 155, 411, 91, 347, 219, 475, 59, 315, 187, 443, 123, 379, 251, 507, 
    7, 1031, 519, 1543, 263, 1287, 775, 1799, 135, 1159, 647, 1671, 391, 1415, 903, 1927, 
    71, 1095, 583, 1607, 327, 1351, 839, 1863, 199, 1223, 711, 1735, 455, 1479, 967, 1991, 
    39, 1063, 551, 1575, 295, 1319, 807, 1831, 167, 1191, 679, 1703, 423, 1447, 935, 1959, 
    103, 1127, 615, 1639, 359, 1383, 871, 1895, 231, 1255, 743, 1767, 487, 1511, 999, 2023, 
    23, 1047, 535, 1559, 279, 1303, 791, 1815, 151, 1175, 663, 1687, 407, 1431, 919, 1943, 
    87, 1111, 599, 1623, 343, 1367, 855, 1879, 215, 1239, 727, 1751, 471, 1495, 983, 2007, 
    55, 1079, 567, 1591, 311, 1335, 823, 1847, 183, 1207, 695, 1719, 439, 1463, 951, 1975, 
    119, 1143, 631, 1655, 375, 1399, 887, 1911, 247, 1271, 759, 1783, 503, 1527, 1015, 2039, 
    15, 1039, 527, 1551, 271, 1295, 783, 1807, 143, 1167, 655, 1679, 399, 1423, 911, 1935, 
    79, 1103, 591, 1615, 335, 1359, 847, 1871, 207, 1231, 719, 1743, 463, 1487, 975, 1999, 
    47, 1071, 559, 1583, 303, 1327, 815, 1839, 175, 1199, 687, 1711, 431, 1455, 943, 1967, 
    111, 1135, 623, 1647, 367, 1391, 879, 1903, 239, 1263, 751, 1775, 495, 1519, 1007, 2031, 
    31, 1055, 543, 1567, 287, 1311, 799, 1823, 159, 1183, 671, 1695, 415, 1439, 927, 1951, 
    95, 1119, 607, 1631, 351, 1375, 863, 1887, 223, 1247, 735, 1759, 479, 1503, 991, 2015, 
    63, 1087, 575, 1599, 319, 1343, 831, 1855, 191, 1215, 703, 1727, 447, 1471, 959, 1983, 
    127, 1151, 639, 1663, 383, 1407, 895, 1919, 255, 1279, 767, 1791, 511, 1535, 1023, 2047, 
];

/// Static Huffman code depths for the 64-symbol distance alphabet.
/// All 6-bit codes.
pub static K_STATIC_DISTANCE_CODE_DEPTH: [u8; 64] = [6; 64];

pub static K_STATIC_DISTANCE_CODE_BITS: [u16; 64] = [
    0, 32, 16, 48, 8, 40, 24, 56, 4, 36, 20, 52, 12, 44, 28, 60, 
    2, 34, 18, 50, 10, 42, 26, 58, 6, 38, 22, 54, 14, 46, 30, 62, 
    1, 33, 17, 49, 9, 41, 25, 57, 5, 37, 21, 53, 13, 45, 29, 61, 
    3, 35, 19, 51, 11, 43, 27, 59, 7, 39, 23, 55, 15, 47, 31, 63, 
];

/// Insert-length base values per RFC 7932 §10.3.
pub static K_INS_BASE: [u32; 24] = [
    0, 1, 2, 3, 4, 5, 6, 8, 10, 14, 18, 26, 34, 50, 66, 98, 130, 194, 322, 578, 1090, 2114, 6210,
    22594,
];

/// Insert-length extra-bit counts per RFC 7932 §10.3.
pub static K_INS_EXTRA: [u32; 24] = [
    0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 7, 8, 9, 10, 12, 14, 24,
];

/// Copy-length base values per RFC 7932 §10.3.
pub static K_COPY_BASE: [u32; 24] = [
    2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 14, 18, 22, 30, 38, 54, 70, 102, 134, 198, 326, 582, 1094,
    2118,
];

/// Copy-length extra-bit counts per RFC 7932 §10.3.
pub static K_COPY_EXTRA: [u32; 24] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 7, 8, 9, 10, 24,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_have_correct_lengths() {
        assert_eq!(K_STATIC_COMMAND_CODE_DEPTH.len(), 704);
        assert_eq!(K_STATIC_COMMAND_CODE_BITS.len(), 704);
        assert_eq!(K_STATIC_DISTANCE_CODE_DEPTH.len(), 64);
        assert_eq!(K_STATIC_DISTANCE_CODE_BITS.len(), 64);
    }

    #[test]
    fn command_depths_match_expected_distribution() {
        for &d in &K_STATIC_COMMAND_CODE_DEPTH[..512] {
            assert_eq!(d, 9);
        }
        for &d in &K_STATIC_COMMAND_CODE_DEPTH[512..] {
            assert_eq!(d, 11);
        }
    }

    #[test]
    fn distance_depths_all_six_bits() {
        for &d in &K_STATIC_DISTANCE_CODE_DEPTH[..] {
            assert_eq!(d, 6);
        }
    }

    #[test]
    fn command_bits_are_distinct_per_length() {
        let mut nine_bit = Vec::new();
        let mut eleven_bit = Vec::new();
        for (i, &d) in K_STATIC_COMMAND_CODE_DEPTH.iter().enumerate() {
            match d {
                9 => nine_bit.push(K_STATIC_COMMAND_CODE_BITS[i]),
                11 => eleven_bit.push(K_STATIC_COMMAND_CODE_BITS[i]),
                _ => panic!("unexpected depth {d}"),
            }
        }
        let nine_unique: std::collections::BTreeSet<u16> = nine_bit.iter().copied().collect();
        let eleven_unique: std::collections::BTreeSet<u16> = eleven_bit.iter().copied().collect();
        assert_eq!(nine_unique.len(), nine_bit.len(), "9-bit codes not unique");
        assert_eq!(
            eleven_unique.len(),
            eleven_bit.len(),
            "11-bit codes not unique"
        );
    }
}
