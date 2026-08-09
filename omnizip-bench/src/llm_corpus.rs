//! LLM-shaped synthetic corpora (TODO 91).
//!
//! These generators approximate the statistical properties of LLM
//! output without embedding any actual model weights or pipeline
//! calls. They use templated prose with templated placeholders to
//! match the high-redundancy, low-entropy properties described in
//! the cited research.
//!
//! ## Properties
//!
//! - **Repeated headers / signatures**: every response opens with
//!   patterns like "Sure, I'd be happy to help!" — repetition the
//!   dictionary trainer exploits.
//! - **Templated structure**: numbered/bulleted lists, JSON-like
//!   blocks, code fences.
//! - **Common phrases**: build/run/explain/test, defs of idioms.
//! - **Low semantic content density**: function names and idioms
//!   repeat far more than they do in natural human text.

const SIGNATURE_OPENINGS: &[&str] = &[
    "Sure, I'd be happy to help with that!\n\n",
    "Of course — here's how I would approach this:\n\n",
    "Great question. Let me break this down step by step:\n\n",
    "Absolutely. Here's a clear explanation:\n\n",
    "I'll walk you through this carefully.\n\n",
];

const SIGNATURE_CLOSINGS: &[&str] = &[
    "\n\nLet me know if you'd like more detail or have follow-up questions.",
    "\n\nHope this helps — happy to clarify any of the steps above.",
    "\n\nIf you'd like a different approach, just let me know.",
    "\n\nFeel free to ask if anything is unclear.",
];

const COMMON_PHRASES: &[&str] = &[
    "In this example, we ",
    "Here's a small function that ",
    "Let's start by looking at ",
    "First, we need to ",
    "Then we can iterate over ",
    "Finally, we should ",
    "Note that the implementation ",
    "We use the standard ",
    "This approach is ",
    "You can verify that ",
];

const CODE_FENCE_TEMPLATES: &[&str] = &[
    "```python\ndef {name}({args}):\n    \"\"\"{docstring}\"\"\"\n    {body}\n```\n",
    "```rust\nfn {name}({args}) -> {ret} {{\n    {body}\n}}\n```\n",
    "```javascript\nfunction {name}({args}) {{\n    {body}\n}}\n```\n",
];

const JSON_BLOCK_TEMPLATE: &str = "{\n  \"name\": \"{name}\",\n  \"type\": \"function\",\n  \"description\": \"{desc}\",\n  \"parameters\": {params}\n}\n";

const NAME_POOL: &[&str] = &[
    "add",
    "sub",
    "mul",
    "div",
    "mod",
    "inc",
    "dec",
    "scale",
    "shift",
    "rotate",
    "translate",
    "normalize",
    "denormalize",
    "encode",
    "decode",
    "encrypt",
    "decrypt",
    "hash",
    "sign",
    "verify",
    "parse",
    "format",
    "serialize",
    "deserialize",
    "validate",
    "sanitize",
    "trim",
    "split",
    "join",
    "merge",
    "filter",
    "map",
    "reduce",
    "fold",
    "scan",
    "zip",
    "unzip",
    "sort",
    "search",
    "insert",
    "remove",
    "delete",
    "update",
];

const ARG_POOL: &[&str] = &[
    "x: i64",
    "y: i64",
    "n: usize",
    "s: &str",
    "buf: &mut [u8]",
    "data: Vec<u8>",
    "key: &[u8]",
    "count: u32",
    "index: usize",
    "offset: u64",
    "length: usize",
    "input: &str",
    "output: &mut String",
];

/// Generate a ChatGPT-conversational-style response of roughly
/// `size` bytes by templating sign-offs, common phrases, code
/// fences, and numbered lists.
#[must_use]
pub fn chat_response(xs: &mut XorShift, size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(SIGNATURE_OPENINGS[xs.next_usize(SIGNATURE_OPENINGS.len())].as_bytes());
    while out.len() < size {
        // Paragraph: 2-5 phrases stitched with common transitional words.
        let n_phrases = 2 + xs.next_usize(4);
        for _ in 0..n_phrases {
            out.extend_from_slice(COMMON_PHRASES[xs.next_usize(COMMON_PHRASES.len())].as_bytes());
            let subject = NAME_POOL[xs.next_usize(NAME_POOL.len())];
            out.extend_from_slice(subject.as_bytes());
            let tail_word = match xs.next_u32(4) {
                0 => " over the iterable.\n",
                1 => " using the standard library.\n",
                2 => " on each thread independently.\n",
                _ => " with proper error handling.\n",
            };
            out.extend_from_slice(tail_word.as_bytes());
        }
        if xs.next_bool() {
            // Insert a code fence.
            let tpl = CODE_FENCE_TEMPLATES[xs.next_usize(CODE_FENCE_TEMPLATES.len())];
            let name = NAME_POOL[xs.next_usize(NAME_POOL.len())];
            let arg1 = ARG_POOL[xs.next_usize(ARG_POOL.len())];
            let arg2 = ARG_POOL[xs.next_usize(ARG_POOL.len())];
            let body = if xs.next_bool() {
                format!("{name} + {arg1}")
            } else {
                format!("return {name};")
            };
            let filled = tpl
                .replace("{name}", name)
                .replace("{args}", &format!("{arg1}, {arg2}"))
                .replace("{ret}", "Result<()")
                .replace("{docstring}", "Do the thing.")
                .replace("{body}", &body);
            out.extend_from_slice(filled.as_bytes());
        }
    }
    out.truncate(size);
    out.extend_from_slice(SIGNATURE_CLOSINGS[xs.next_usize(SIGNATURE_CLOSINGS.len())].as_bytes());
    out
}

/// Code-generation payload: a rapid sequence of code fences.
#[must_use]
pub fn code_gen(xs: &mut XorShift, size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        let tpl = CODE_FENCE_TEMPLATES[xs.next_usize(CODE_FENCE_TEMPLATES.len())];
        let name = NAME_POOL[xs.next_usize(NAME_POOL.len())];
        let arg = ARG_POOL[xs.next_usize(ARG_POOL.len())];
        let body = if xs.next_bool() {
            format!("return {name}({arg});")
        } else {
            format!("let x = {arg};\n    println!(\"{{x:?}}\");")
        };
        let filled = tpl
            .replace("{name}", name)
            .replace("{args}", arg)
            .replace("{ret}", "()")
            .replace("{docstring}", "Helper.")
            .replace("{body}", &body);
        out.extend_from_slice(filled.as_bytes());
    }
    out.truncate(size);
    out
}

/// Structured-JSON payload.
#[must_use]
pub fn structured_json(xs: &mut XorShift, size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        let name = NAME_POOL[xs.next_usize(NAME_POOL.len())];
        let desc = "Performs a specific transformation on the input.";
        let params = format!(
            "{{ \"input\": \"{}\", \"output\": \"{}\" }}",
            ARG_POOL[xs.next_usize(ARG_POOL.len())],
            NAME_POOL[xs.next_usize(NAME_POOL.len())]
        );
        let filled = JSON_BLOCK_TEMPLATE
            .replace("{name}", name)
            .replace("{desc}", desc)
            .replace("{params}", &params);
        out.extend_from_slice(filled.as_bytes());
    }
    out.truncate(size);
    out
}

/// xorshift64* — deterministic, no time source. Same as `synthetic.rs`.
pub struct XorShift {
    state: u64,
}

impl XorShift {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn next_usize(&mut self, max: usize) -> usize {
        (self.next_u64() as usize) % max.max(1)
    }

    pub fn next_u32(&mut self, max: u32) -> u32 {
        (self.next_u64() as u32) % max.max(1)
    }

    pub fn next_bool(&mut self) -> bool {
        self.next_u64() & 1 == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_response_is_approximately_requested_size() {
        let mut xs = XorShift::new(42);
        let v = chat_response(&mut xs, 4096);
        // ±20% tolerance — exact size depends on the last code fence.
        assert!(v.len() >= 4096 - 1024, "got {} bytes", v.len());
        assert!(v.len() <= 4096 + 512, "got {} bytes", v.len());
    }

    #[test]
    fn code_gen_produces_code_fences() {
        let mut xs = XorShift::new(7);
        let v = code_gen(&mut xs, 1024);
        assert!(v.windows(3).any(|w| w == b"```"));
    }

    #[test]
    fn structured_json_produces_braces() {
        let mut xs = XorShift::new(99);
        let v = structured_json(&mut xs, 1024);
        assert!(v.iter().all(|&b| b == b'{'
            || b == b'}'
            || b.is_ascii_graphic()
            || b == b' '
            || b == b'\n'));
        assert!(v.contains(&b'{')); // ensure not zero-filled
    }

    #[test]
    fn xorshift_is_deterministic() {
        let mut a = XorShift::new(123);
        let mut b = XorShift::new(123);
        for _ in 0..10 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }
}
