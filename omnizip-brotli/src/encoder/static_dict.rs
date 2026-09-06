//! Faithful port of google/brotli `BrotliFindAllStaticDictionaryMatches`
//! (`src/enc/static_dict.rs`, BSD-3-Clause) — the HQ-zopfli (q10/q11)
//! static-dictionary search. For each word in the 4-byte hash bucket it
//! derives the whole transform family: the raw word, omit-1..9 prefixes,
//! the "ing " continuation, and the affix continuations (" ", " a ",
//! "s ", "t ", "nd ", "by ", "in ", "is ", "for ", "from ", "of ",
//! "on ", "not ", " the ", " that ", " to ", " with ", quotes and
//! punctuation, plus uppercase variants and the leading " "/"."
//! "e "/"s "/",\xc2\xa0" " the "/".com/" blocks). One candidate per
//! PRODUCED length; the smallest word-id wins (upstream `AddMatch`).

use super::static_dict_lut::{word_entry, K_STATIC_DICTIONARY_BUCKETS};
use crate::dictionary::{DICTIONARY_DATA, OFFSETS_BY_LENGTH, SIZE_BITS_BY_LENGTH};

pub const MAX_STATIC_DICTIONARY_MATCH_LEN: usize = 37;
const K_INVALID_MATCH: u32 = u32::MAX;
const K_UPPERCASE_FIRST: u8 = 10;

const K_OMIT_LAST_N_TRANSFORMS: [u8; 10] = [0, 12, 27, 23, 42, 63, 56, 48, 59, 64];

#[inline]
fn hash(data: &[u8]) -> usize {
    let mut b = [0u8; 4];
    b.copy_from_slice(&data[..4]);
    (u32::from_le_bytes(b).wrapping_mul(0x1E35_A7BD) >> (32 - 15)) as usize
}

#[inline]
fn dict_match_length(data: &[u8], id: usize, len: usize, maxlen: usize) -> usize {
    let offset = OFFSETS_BY_LENGTH[len] as usize + len * id;
    let limit = len.min(maxlen);
    let dict = &DICTIONARY_DATA[offset..];
    let mut i = 0usize;
    while i < limit && i < dict.len() && i < data.len() && dict[i] == data[i] {
        i += 1;
    }
    i
}

/// Upstream `IsMatch`: raw (transform 0), UppercaseFirst (10), or
/// UPPERCASE (other) word must match `data[..l]` exactly.
fn is_match(l: usize, transform: u8, idx: u16, data: &[u8], max_length: usize) -> bool {
    if l > max_length || l == 0 {
        return false;
    }
    let offset = OFFSETS_BY_LENGTH[l] as usize + l * usize::from(idx);
    let dict = &DICTIONARY_DATA[offset..];
    if dict.len() < l || data.len() < l {
        return false;
    }
    if transform == 0 {
        for i in 0..l {
            if dict[i] != data[i] {
                return false;
            }
        }
        true
    } else if transform == K_UPPERCASE_FIRST {
        let d0 = dict[0];
        if !(d0.is_ascii_lowercase()) || (d0 ^ 32) != data[0] {
            return false;
        }
        for i in 1..l {
            if dict[i] != data[i] {
                return false;
            }
        }
        true
    } else {
        for i in 0..l {
            let d = dict[i];
            if d.is_ascii_lowercase() {
                if (d ^ 32) != data[i] {
                    return false;
                }
            } else if d != data[i] {
                return false;
            }
        }
        true
    }
}

/// `matches[len] = (word_id << 5) | word_length`, smallest wins.
#[inline]
fn add_match(id: usize, len: usize, len_code: usize, matches: &mut [u32; 38]) {
    let m = ((id as u32) << 5).wrapping_add(len_code as u32);
    if m < matches[len] {
        matches[len] = m;
    }
}

/// Populate `matches[produced_len]` for every dictionary-derived match
/// of `data[..max_length]` with produced length >= `min_length`.
/// Returns true when at least one match was found.
pub fn find_all_static_dictionary_matches(
    data: &[u8],
    min_length: usize,
    max_length: usize,
    matches: &mut [u32; 38],
) -> bool {
    let mut has_found_match = false;
    matches.fill(K_INVALID_MATCH);

    // ---- main bucket: Hash(data)
    walk_bucket(
        data,
        data,
        0,
        min_length,
        max_length,
        matches,
        0,
        &mut has_found_match,
        false,
    );

    // ---- leading ' ' or '.': Hash(data[1..]), transforms 6/32 (+caps variants 85/30, affixes)
    if max_length >= 5 && (data[0] == b' ' || data[0] == b'.') {
        let is_space = data[0] == b' ';
        walk_bucket(
            &data[1..],
            data,
            1,
            min_length,
            max_length,
            matches,
            if is_space { 6 } else { 32 },
            &mut has_found_match,
            false,
        );
        // uppercase variants under the leading ' ' only (upstream is_space branch)
        if is_space {
            walk_bucket(
                &data[1..],
                data,
                1,
                min_length,
                max_length,
                matches,
                85,
                &mut has_found_match,
                true,
            );
        }
    }

    // ---- leading "e " / "s " / ", " / NBSP (0xC2 0xA0): Hash(data[2..]), transforms 18/7/13/102
    if max_length >= 6
        && ((data[1] == b' ' && (data[0] == b'e' || data[0] == b's' || data[0] == b','))
            || (data[0] == 0xC2 && data[1] == 0xA0))
    {
        let t = if data[0] == 0xC2 {
            102
        } else if data[0] == b'e' {
            18
        } else if data[0] == b's' {
            7
        } else {
            13
        };
        walk_bucket(
            &data[2..],
            data,
            2,
            min_length,
            max_length,
            matches,
            t,
            &mut has_found_match,
            false,
        );
    }

    // ---- leading " the " / ".com/": Hash(data[5..]), transforms 41/72 (+62 " of ", 73 " of the ")
    if max_length >= 9
        && ((data[0] == b' '
            && data[1] == b't'
            && data[2] == b'h'
            && data[3] == b'e'
            && data[4] == b' ')
            || (data[0] == b'.'
                && data[1] == b'c'
                && data[2] == b'o'
                && data[3] == b'm'
                && data[4] == b'/'))
    {
        let base_t = if data[0] == b' ' { 41 } else { 72 };
        // This block needs its own walk (word must match data[5..] raw and
        // continue with ' ' before " of "/" of the " applies).
        let sub = &data[5..];
        let sub_max = max_length.saturating_sub(5);
        let mut offset = K_STATIC_DICTIONARY_BUCKETS[hash(sub)] as usize;
        let mut end = offset == 0;
        while !end {
            let (l_flag, transform, idx) = word_entry(offset);
            offset += 1;
            let l = usize::from(l_flag & 0x1F);
            let n = 1usize << SIZE_BITS_BY_LENGTH[l];
            let id = usize::from(idx);
            end = l_flag & 0x20 != 0;
            if transform != 0 || SIZE_BITS_BY_LENGTH[l] == 0 || l == 0 {
                continue;
            }
            if is_match(l, 0, idx, sub, sub_max) {
                add_match(id + base_t * n, l + 5, l, matches);
                has_found_match = true;
                if l + 5 < max_length {
                    let s = &data[l + 5..];
                    if data[0] == b' '
                        && l + 8 < max_length
                        && s[0] == b' '
                        && s[1] == b'o'
                        && s[2] == b'f'
                        && s[3] == b' '
                    {
                        add_match(id + 62 * n, l + 9, l, matches);
                        if l + 12 < max_length
                            && s[4] == b't'
                            && s[5] == b'h'
                            && s[6] == b'e'
                            && s[7] == b' '
                        {
                            add_match(id + 73 * n, l + 13, l, matches);
                        }
                    }
                }
            }
        }
    }
    has_found_match
}

/// One bucket walk. `sub_t` selects the sub-block (0 = main over
/// `data` itself; 6/32 = leading " "/"." over data[1..]; 102 = NBSP
/// over data[2..]; 18/7/13 = leading "e "/"s "/", " over data[2..]).
/// `caps` additionally derives the leading-" " uppercase variants
/// (upstream's is_space else-branch, transforms 85/30 + continuations).
#[allow(clippy::too_many_lines)]
fn walk_bucket(
    data: &[u8],
    full: &[u8],
    lead: usize,
    min_length: usize,
    max_length: usize,
    matches: &mut [u32; 38],
    sub_t: usize,
    has_found_match: &mut bool,
    caps: bool,
) {
    // `data` is the probed slice (full[lead..]); `full` is the original
    // — sub-block continuation reads and the max_length guards are
    // relative to FULL positions (upstream indexes one `data` param).
    let _ = full;
    let sub_max = max_length.saturating_sub(lead);
    if data.len() < 4 {
        return;
    }
    let mut offset = K_STATIC_DICTIONARY_BUCKETS[hash(data)] as usize;
    let mut end = offset == 0;
    while !end {
        let (l_flag, transform, idx) = word_entry(offset);
        offset += 1;
        let l = usize::from(l_flag & 0x1F);
        end = l_flag & 0x20 != 0;
        if SIZE_BITS_BY_LENGTH[l] == 0 || l < 4 {
            continue;
        }
        let n = 1usize << SIZE_BITS_BY_LENGTH[l];
        let id = usize::from(idx);

        // The caps walk (upstream's is_space else-branch) handles ONLY
        // transformed entries: raw words belong to the 6/32 sub-block
        // walk. Leaking them here routed them through the 18/7/13 arm,
        // which multiplies by `sub_t` (= 85) — fabricating bogus
        // " " + ALL-CAPS(w) + " " candidates for inputs that merely
        // matched the raw word (found via a two-byte-away ' ').
        if caps && transform == 0 {
            continue;
        }

        if sub_t != 0 && transform != 0 {
            // Sub-block uppercase variants (upstream's is_space
            // else-branch; only the caps walk reaches this).
            if !caps {
                continue;
            }
            let is_all_caps = transform != K_UPPERCASE_FIRST;
            if is_match(l, transform, idx, data, sub_max) {
                add_match(
                    id + (if is_all_caps { 85 } else { 30 }) * n,
                    l + 1,
                    l,
                    matches,
                );
                *has_found_match = true;
                if l + 2 < max_length {
                    let s = &full[l + 1..];
                    if s[0] == b' ' {
                        add_match(
                            id + (if is_all_caps { 83 } else { 15 }) * n,
                            l + 2,
                            l,
                            matches,
                        );
                    } else if s[0] == b',' {
                        if !is_all_caps {
                            add_match(id + 109 * n, l + 2, l, matches);
                        }
                        if s[1] == b' ' {
                            add_match(
                                id + (if is_all_caps { 111 } else { 65 }) * n,
                                l + 3,
                                l,
                                matches,
                            );
                        }
                    } else if s[0] == b'.' {
                        add_match(
                            id + (if is_all_caps { 115 } else { 96 }) * n,
                            l + 2,
                            l,
                            matches,
                        );
                        if s[1] == b' ' {
                            add_match(
                                id + (if is_all_caps { 117 } else { 91 }) * n,
                                l + 3,
                                l,
                                matches,
                            );
                        }
                    } else if s[0] == b'=' {
                        if s[1] == b'"' {
                            add_match(
                                id + (if is_all_caps { 110 } else { 118 }) * n,
                                l + 3,
                                l,
                                matches,
                            );
                        } else if s[1] == b'\'' {
                            add_match(
                                id + (if is_all_caps { 119 } else { 120 }) * n,
                                l + 3,
                                l,
                                matches,
                            );
                        }
                    }
                }
            }
            continue;
        }

        if sub_t != 0 {
            // ---- sub-blocks: raw word (+ fixed continuation deltas)
            match sub_t {
                6 | 32 => {
                    if !is_match(l, 0, idx, data, sub_max) {
                        continue;
                    }
                    add_match(id + sub_t * n, l + 1, l, matches);
                    *has_found_match = true;
                    if l + 2 >= max_length {
                        continue;
                    }
                    let s = &full[l + 1..];
                    if s[0] == b' ' {
                        add_match(
                            id + (if sub_t == 6 { 2 } else { 77 }) * n,
                            l + 2,
                            l,
                            matches,
                        );
                    } else if s[0] == b'(' {
                        add_match(
                            id + (if sub_t == 6 { 89 } else { 67 }) * n,
                            l + 2,
                            l,
                            matches,
                        );
                    } else if sub_t == 6 {
                        if s[0] == b',' {
                            add_match(id + 103 * n, l + 2, l, matches);
                            if s[1] == b' ' {
                                add_match(id + 33 * n, l + 3, l, matches);
                            }
                        } else if s[0] == b'.' {
                            add_match(id + 71 * n, l + 2, l, matches);
                            if s[1] == b' ' {
                                add_match(id + 52 * n, l + 3, l, matches);
                            }
                        } else if s[0] == b'=' {
                            if s[1] == b'"' {
                                add_match(id + 81 * n, l + 3, l, matches);
                            } else if s[1] == b'\'' {
                                add_match(id + 98 * n, l + 3, l, matches);
                            }
                        }
                    }
                }
                102 => {
                    if is_match(l, 0, idx, data, sub_max) {
                        add_match(id + 102 * n, l + 2, l, matches);
                        *has_found_match = true;
                    }
                }
                _ => {
                    // 18/7/13: "e "/"s "/", " + word + " "
                    if l + 2 < max_length
                        && is_match(l, 0, idx, data, sub_max)
                        && full[l + 2] == b' '
                    {
                        add_match(id + sub_t * n, l + 3, l, matches);
                        *has_found_match = true;
                    }
                }
            }
            continue;
        }

        if transform == 0 {
            let matchlen = dict_match_length(data, id, l, max_length);
            if matchlen == l {
                add_match(id, l, l, matches);
                *has_found_match = true;
            }
            if matchlen + 1 >= l {
                add_match(id + 12 * n, l - 1, l, matches);
                *has_found_match = true;
                if l + 2 < max_length
                    && data[l - 1] == b'i'
                    && data[l] == b'n'
                    && data[l + 1] == b'g'
                    && data[l + 2] == b' '
                {
                    add_match(id + 49 * n, l + 3, l, matches);
                }
            }
            let minlen = if l > 9 {
                min_length.max(l - 9)
            } else {
                min_length
            };
            let maxlen = matchlen.min(l.saturating_sub(2));
            for len in minlen..=maxlen {
                add_match(
                    id + usize::from(K_OMIT_LAST_N_TRANSFORMS[l - len]) * n,
                    len,
                    l,
                    matches,
                );
                *has_found_match = true;
            }
            if matchlen < l || l + 6 >= max_length {
                continue;
            }
            let s = &data[l..];
            if s[0] == b' ' {
                add_match(id + n, l + 1, l, matches);
                if s[1] == b'a' {
                    if s[2] == b' ' {
                        add_match(id + 28 * n, l + 3, l, matches);
                    } else if s[2] == b's' && s[3] == b' ' {
                        add_match(id + 46 * n, l + 4, l, matches);
                    } else if s[2] == b't' && s[3] == b' ' {
                        add_match(id + 60 * n, l + 4, l, matches);
                    } else if s[2] == b'n' && s[3] == b'd' && s[4] == b' ' {
                        add_match(id + 10 * n, l + 5, l, matches);
                    }
                } else if s[1] == b'b' && s[2] == b'y' && s[3] == b' ' {
                    add_match(id + 38 * n, l + 4, l, matches);
                } else if s[1] == b'i' {
                    if s[2] == b'n' && s[3] == b' ' {
                        add_match(id + 16 * n, l + 4, l, matches);
                    } else if s[2] == b's' && s[3] == b' ' {
                        add_match(id + 47 * n, l + 4, l, matches);
                    }
                } else if s[1] == b'f' {
                    if s[2] == b'o' && s[3] == b'r' && s[4] == b' ' {
                        add_match(id + 25 * n, l + 5, l, matches);
                    } else if s[2] == b'r' && s[3] == b'o' && s[4] == b'm' && s[5] == b' ' {
                        add_match(id + 37 * n, l + 6, l, matches);
                    }
                } else if s[1] == b'o' {
                    if s[2] == b'f' && s[3] == b' ' {
                        add_match(id + 8 * n, l + 4, l, matches);
                    } else if s[2] == b'n' && s[3] == b' ' {
                        add_match(id + 45 * n, l + 4, l, matches);
                    }
                } else if s[1] == b'n' && s[2] == b'o' && s[3] == b't' && s[4] == b' ' {
                    add_match(id + 80 * n, l + 5, l, matches);
                } else if s[1] == b't' {
                    if s[2] == b'h' {
                        if s[3] == b'e' && s[4] == b' ' {
                            add_match(id + 5 * n, l + 5, l, matches);
                        } else if s[3] == b'a' && s[4] == b't' && s[5] == b' ' {
                            add_match(id + 29 * n, l + 6, l, matches);
                        }
                    } else if s[2] == b'o' && s[3] == b' ' {
                        add_match(id + 17 * n, l + 4, l, matches);
                    }
                } else if s[1] == b'w'
                    && s[2] == b'i'
                    && s[3] == b't'
                    && s[4] == b'h'
                    && s[5] == b' '
                {
                    add_match(id + 35 * n, l + 6, l, matches);
                }
            } else if s[0] == b'"' {
                add_match(id + 19 * n, l + 1, l, matches);
                if s[1] == b'>' {
                    add_match(id + 21 * n, l + 2, l, matches);
                }
            } else if s[0] == b'.' {
                add_match(id + 20 * n, l + 1, l, matches);
                if s[1] == b' ' {
                    add_match(id + 31 * n, l + 2, l, matches);
                    if s[2] == b'T' && s[3] == b'h' {
                        if s[4] == b'e' && s[5] == b' ' {
                            add_match(id + 43 * n, l + 6, l, matches);
                        } else if s[4] == b'i' && s[5] == b's' && s[6] == b' ' {
                            add_match(id + 75 * n, l + 7, l, matches);
                        }
                    }
                }
            } else if s[0] == b',' {
                add_match(id + 76 * n, l + 1, l, matches);
                if s[1] == b' ' {
                    add_match(id + 14 * n, l + 2, l, matches);
                }
            } else if s[0] == b'\n' {
                add_match(id + 22 * n, l + 1, l, matches);
                if s[1] == b'\t' {
                    add_match(id + 50 * n, l + 2, l, matches);
                }
            } else if s[0] == b']' {
                add_match(id + 24 * n, l + 1, l, matches);
            } else if s[0] == b'\'' {
                add_match(id + 36 * n, l + 1, l, matches);
            } else if s[0] == b':' {
                add_match(id + 51 * n, l + 1, l, matches);
            } else if s[0] == b'(' {
                add_match(id + 57 * n, l + 1, l, matches);
            } else if s[0] == b'=' {
                if s[1] == b'"' {
                    add_match(id + 70 * n, l + 2, l, matches);
                } else if s[1] == b'\'' {
                    add_match(id + 86 * n, l + 2, l, matches);
                }
            } else if s[0] == b'a' && s[1] == b'l' && s[2] == b' ' {
                add_match(id + 84 * n, l + 3, l, matches);
            } else if s[0] == b'e' {
                if s[1] == b'd' && s[2] == b' ' {
                    add_match(id + 53 * n, l + 3, l, matches);
                } else if s[1] == b'r' && s[2] == b' ' {
                    add_match(id + 82 * n, l + 3, l, matches);
                } else if s[1] == b's' && s[2] == b't' && s[3] == b' ' {
                    add_match(id + 95 * n, l + 4, l, matches);
                }
            } else if s[0] == b'f' && s[1] == b'u' && s[2] == b'l' && s[3] == b' ' {
                add_match(id + 90 * n, l + 4, l, matches);
            } else if s[0] == b'i' {
                if s[1] == b'v' && s[2] == b'e' && s[3] == b' ' {
                    add_match(id + 92 * n, l + 4, l, matches);
                } else if s[1] == b'z' && s[2] == b'e' && s[3] == b' ' {
                    add_match(id + 100 * n, l + 4, l, matches);
                }
            } else if s[0] == b'l' {
                if s[1] == b'e' && s[2] == b's' && s[3] == b's' && s[4] == b' ' {
                    add_match(id + 93 * n, l + 5, l, matches);
                } else if s[1] == b'y' && s[2] == b' ' {
                    add_match(id + 61 * n, l + 3, l, matches);
                }
            } else if s[0] == b'o' && s[1] == b'u' && s[2] == b's' && s[3] == b' ' {
                add_match(id + 106 * n, l + 4, l, matches);
            }
        } else {
            // Uppercase variants in the MAIN block: ALL-CAPS (44) /
            // UppercaseFirst (9) + continuation.
            let is_all_caps = transform != K_UPPERCASE_FIRST;
            if is_match(l, transform, idx, data, max_length) {
                add_match(id + (if is_all_caps { 44 } else { 9 }) * n, l, l, matches);
                *has_found_match = true;
                if l + 1 < max_length {
                    let s = &data[l..];
                    if s[0] == b' ' {
                        add_match(
                            id + (if is_all_caps { 68 } else { 4 }) * n,
                            l + 1,
                            l,
                            matches,
                        );
                    } else if s[0] == b'"' {
                        add_match(
                            id + (if is_all_caps { 87 } else { 66 }) * n,
                            l + 1,
                            l,
                            matches,
                        );
                        if s[1] == b'>' {
                            add_match(
                                id + (if is_all_caps { 97 } else { 69 }) * n,
                                l + 2,
                                l,
                                matches,
                            );
                        }
                    } else if s[0] == b'.' {
                        add_match(
                            id + (if is_all_caps { 101 } else { 79 }) * n,
                            l + 1,
                            l,
                            matches,
                        );
                        if s[1] == b' ' {
                            add_match(
                                id + (if is_all_caps { 114 } else { 88 }) * n,
                                l + 2,
                                l,
                                matches,
                            );
                        }
                    } else if s[0] == b',' {
                        add_match(
                            id + (if is_all_caps { 112 } else { 99 }) * n,
                            l + 1,
                            l,
                            matches,
                        );
                        if s[1] == b' ' {
                            add_match(
                                id + (if is_all_caps { 107 } else { 58 }) * n,
                                l + 2,
                                l,
                                matches,
                            );
                        }
                    } else if s[0] == b'\'' {
                        add_match(
                            id + (if is_all_caps { 94 } else { 74 }) * n,
                            l + 1,
                            l,
                            matches,
                        );
                    } else if s[0] == b'(' {
                        add_match(
                            id + (if is_all_caps { 113 } else { 78 }) * n,
                            l + 1,
                            l,
                            matches,
                        );
                    } else if s[0] == b'=' {
                        if s[1] == b'"' {
                            add_match(
                                id + (if is_all_caps { 105 } else { 104 }) * n,
                                l + 2,
                                l,
                                matches,
                            );
                        } else if s[1] == b'\'' {
                            add_match(
                                id + (if is_all_caps { 116 } else { 108 }) * n,
                                l + 2,
                                l,
                                matches,
                            );
                        }
                    }
                }
            }
        }
    }
}
