//! Bounds on what a hostile PDF can make the engine allocate.
//!
//! A PDF is untrusted input. Compressed streams can inflate a few kilobytes into gigabytes
//! ("decompression bombs"), predictor parameters can request rows of any width, and a short
//! content stream can hold millions of drawing operations. Running out of memory aborts the
//! process, so every place the engine decodes something goes through here.
//!
//! Anything over a limit is treated as unreadable and left exactly as it is, the same way the
//! engine treats anything else it does not understand. Each analysis runs the four presets at
//! once, so worst-case memory is roughly five times the per-reader figures below.

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

/// The input file. It is read into memory whole, and each analysis holds a few copies.
pub const FILE: u64 = 2 << 30;
/// Objects in a document, estimated before loading. Each loaded object costs a few kilobytes,
/// so this bounds memory at a few gigabytes. Real documents rarely pass 100,000.
pub const OBJECTS: usize = 1_000_000;
/// Documents with more objects than this are analysed one preset at a time, not in parallel.
pub const PARALLEL_OBJECTS: usize = 100_000;
/// Any one decoded stream, and every stream decoded while loading (object and xref streams).
pub const STREAM: usize = 256 << 20;
/// A page's content, or one form XObject, pattern or appearance stream.
pub const CONTENT: usize = 64 << 20;
/// Drawing operations in one content stream, estimated before decoding. Decoded operations
/// take a few hundred bytes each, far more than their text.
pub const OPERATIONS: usize = 1_000_000;
/// An embedded font program. Whole CJK fonts reach about 25 MB.
pub const FONT: usize = 128 << 20;
/// Metadata and CID-to-glyph maps (at most 128 KB when valid).
pub const SMALL: usize = 16 << 20;
/// Pixels in an image the engine will decode. 100 million is a 10,000 × 10,000 scan.
pub const IMAGE_PIXELS: u64 = 100_000_000;

/// Predictor rows wider than this are refused: no real image or xref stream comes close.
const MAX_COLUMNS: i64 = 1 << 20;
const MAX_COLORS: i64 = 64;

/// The stream's decoded bytes, or `None` when they would exceed `limit`, the decode
/// parameters are implausible, or decoding fails.
pub fn decode(stream: &Stream, limit: usize) -> Option<Vec<u8>> {
    if !params_ok(&stream.dict) {
        return None;
    }
    if !stream.dict.has(b"Filter") {
        return (stream.content.len() <= limit).then(|| stream.content.clone());
    }
    match decodable(&stream.dict)? {
        Params::AsWritten => stream.decompressed_content_with_limit(limit).ok(),
        Params::Unwrapped(p) => {
            let mut dict = stream.dict.clone();
            dict.set("DecodeParms", p);
            Stream::new(dict, stream.content.clone()).decompressed_content_with_limit(limit).ok()
        }
    }
}

/// How lopdf must be given a stream's `/DecodeParms` to decode it correctly.
enum Params {
    /// The dictionary is fine as it is.
    AsWritten,
    /// A one-filter stream whose parameters were written as `[<< … >>]`.
    Unwrapped(Dictionary),
}

/// Whether lopdf decodes these filters and parameters to the right bytes. It reads
/// `/DecodeParms` only as a direct dictionary and hands that one dictionary to every filter,
/// and it treats a `/Filter` it cannot read as no filter at all. Anything it would get wrong
/// returns `None`, so the stream is left exactly as it is:
/// - a `/Filter` that is not a name or an array of names (a reference, for one);
/// - parameters given by reference (resolving them needs the document);
/// - parameters for a chain of filters, unless every entry is null.
///
/// Before this check, `[<< /Predictor 15 … >>]` was ignored and images came out scrambled.
fn decodable(dict: &Dictionary) -> Option<Params> {
    let filters = match dict.get(b"Filter") {
        Err(_) => 0,
        Ok(Object::Name(_)) => 1,
        Ok(Object::Array(a)) if a.iter().all(|f| matches!(f, Object::Name(_))) => a.len(),
        Ok(_) => return None,
    };
    match dict.get(b"DecodeParms") {
        Err(_) | Ok(Object::Null) => Some(Params::AsWritten),
        Ok(Object::Dictionary(_)) if filters == 1 => Some(Params::AsWritten),
        Ok(Object::Array(a)) if a.len() == filters && a.iter().all(|p| matches!(p, Object::Null)) => Some(Params::AsWritten),
        Ok(Object::Array(a)) if a.len() == 1 && filters == 1 => match &a[0] {
            Object::Dictionary(p) => Some(Params::Unwrapped(p.clone())),
            _ => None,
        },
        Ok(_) => None,
    }
}

/// Whether `decode` can read this stream's filters and parameters. Used to report an image
/// that is left alone for this reason, instead of it failing silently later.
pub fn decodable_params(dict: &Dictionary) -> bool {
    params_ok(dict) && decodable(dict).is_some()
}

/// A page's content streams, joined, or `None` when any of them is over the limits or cannot
/// be decoded. (lopdf's own reader appends a stream's raw bytes when it fails to decode it,
/// which would be read as drawing operations.)
pub fn page_content(doc: &Document, page_id: ObjectId) -> Option<Vec<u8>> {
    let mut content = Vec::new();
    for id in doc.get_page_contents(page_id) {
        let Ok(Object::Stream(s)) = doc.get_object(id) else { continue };
        content.extend_from_slice(&decode(s, CONTENT.saturating_sub(content.len()))?);
        content.push(b'\n');
    }
    operations_ok(&content).then_some(content)
}

/// Whether a content stream is small enough in operations to decode. Counts tokens that
/// start with a letter, which includes every operator (and some names and text, so it errs
/// on the side of refusing).
pub fn operations_ok(content: &[u8]) -> bool {
    let regular = |b: u8| !b" \t\r\n\x0c\0()<>[]{}/%".contains(&b);
    let mut tokens = 0usize;
    let mut previous = b' ';
    for &b in content {
        if regular(b) && !regular(previous) && (b.is_ascii_alphabetic() || b == b'\'' || b == b'"') {
            tokens += 1;
            if tokens > OPERATIONS {
                return false;
            }
        }
        previous = b;
    }
    true
}

/// Rejects predictor parameters that would make the decoder allocate absurd rows.
/// The decoder sizes rows from `/Columns` whatever the output limit.
fn params_ok(dict: &Dictionary) -> bool {
    let check = |o: &Object| match o {
        Object::Dictionary(p) => {
            let int = |k: &[u8]| p.get(k).and_then(Object::as_i64).ok();
            int(b"Columns").is_none_or(|c| (0..=MAX_COLUMNS).contains(&c))
                && int(b"Colors").is_none_or(|c| (0..=MAX_COLORS).contains(&c))
                && int(b"BitsPerComponent").is_none_or(|b| matches!(b, 1 | 2 | 4 | 8 | 16))
        }
        _ => true,
    };
    match dict.get(b"DecodeParms") {
        Ok(Object::Array(items)) => items.iter().all(check),
        Ok(o) => check(o),
        Err(_) => true,
    }
}

/// Checks the raw file for predictor parameters that would blow up while loading, before
/// any parsing. Streams decoded at load time (cross-reference and object streams) are not
/// otherwise reachable by `params_ok`. A `/Columns` or `/Colors` value this large never
/// occurs in a real file.
pub fn raw_params_ok(bytes: &[u8]) -> bool {
    for (key, max) in [(&b"/Columns"[..], MAX_COLUMNS), (b"/Colors", MAX_COLORS)] {
        let mut at = 0;
        while let Some(i) = bytes[at..].windows(key.len()).position(|w| w == key) {
            let start = at + i + key.len();
            let digits: Vec<u8> = bytes[start..]
                .iter()
                .skip_while(|b| b.is_ascii_whitespace())
                .take_while(|b| b.is_ascii_digit())
                .take(20)
                .copied()
                .collect();
            if let Ok(n) = std::str::from_utf8(&digits).unwrap_or("").parse::<u128>() {
                if n > max as u128 {
                    return false;
                }
            }
            at = start;
        }
    }
    true
}

/// Estimates how many objects a file holds without parsing it: each `N G obj` header, plus
/// the `/N` count every object stream declares in its (uncompressed) dictionary.
pub fn raw_object_estimate(bytes: &[u8]) -> usize {
    let mut count = 0usize;
    let mut at = 0;
    while let Some(i) = bytes[at..].windows(4).position(|w| w == b" obj") {
        let pos = at + i;
        // "12 0 obj": a digit, a space and a digit before " obj".
        if pos >= 3 && bytes[pos - 1].is_ascii_digit() && bytes[pos - 2] == b' ' && bytes[pos - 3].is_ascii_digit() {
            count += 1;
        }
        at = pos + 4;
    }
    at = 0;
    while let Some(i) = bytes[at..].windows(7).position(|w| w == b"/ObjStm") {
        let pos = at + i;
        let window = &bytes[pos.saturating_sub(300)..(pos + 300).min(bytes.len())];
        if let Some(n) = window.windows(3).position(|w| w == b"/N ").map(|j| {
            window[j + 3..].iter().take_while(|b| b.is_ascii_digit()).take(12).fold(0usize, |v, d| v.saturating_mul(10).saturating_add((d - b'0') as usize))
        }) {
            count = count.saturating_add(n);
        }
        at = pos + 7;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    #[test]
    fn counts_operations_before_decoding() {
        assert!(operations_ok(b"q 1 0 0 1 0 0 cm /Im0 Do Q BT /F1 12 Tf (Hello) Tj ET"));
        assert!(!operations_ok(&b"q\n".repeat(OPERATIONS + 1)));
    }

    #[test]
    fn refuses_absurd_predictors() {
        let ok = dictionary! { "DecodeParms" => dictionary! { "Predictor" => 15, "Columns" => 2000, "Colors" => 3 } };
        let bad = dictionary! { "DecodeParms" => dictionary! { "Predictor" => 15, "Columns" => 2147483647i64 } };
        assert!(params_ok(&ok));
        assert!(!params_ok(&bad));
        assert!(raw_params_ok(b"<< /DecodeParms << /Columns 5 /Predictor 12 >> >>"));
        assert!(!raw_params_ok(b"<< /DecodeParms << /Columns 2147483647 >> >>"));
    }

    #[test]
    fn estimates_objects_before_loading() {
        assert_eq!(raw_object_estimate(b"1 0 obj\n<<>>\nendobj\n12 0 obj\n<< /Type /ObjStm /N 40 /First 9 >>"), 42);
    }

    #[test]
    fn decodes_parameters_only_in_forms_lopdf_reads_correctly() {
        let p = || dictionary! { "Predictor" => 12, "Columns" => 4 };
        // Rows of 4 bytes with PNG "Up" prediction: the second row repeats the first.
        let raw = vec![2u8, 1, 2, 3, 4, 2, 0, 0, 0, 0];
        let stream = |parms: Object| {
            use std::io::Write;
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(&raw).unwrap();
            Stream::new(dictionary! { "Filter" => "FlateDecode", "DecodeParms" => parms }, z.finish().unwrap())
        };
        let want = Some(vec![1, 2, 3, 4, 1, 2, 3, 4]);
        assert_eq!(decode(&stream(Object::Dictionary(p())), 64), want);
        assert_eq!(decode(&stream(Object::Array(vec![Object::Dictionary(p())])), 64), want);
        assert_eq!(decode(&stream(Object::Reference((9, 0))), 64), None);
        let mut chain = stream(Object::Array(vec![Object::Null, Object::Dictionary(p())]));
        chain.dict.set("Filter", vec![Object::Name(b"ASCIIHexDecode".to_vec()), Object::Name(b"FlateDecode".to_vec())]);
        assert_eq!(decode(&chain, 64), None);
        let mut by_reference = stream(Object::Null);
        assert_eq!(decode(&by_reference, 64), Some(raw.clone()));
        by_reference.dict.set("Filter", Object::Reference((9, 0)));
        assert_eq!(decode(&by_reference, 64), None);
    }

    #[test]
    fn bounds_decoding() {
        let mut s = Stream::new(dictionary! {}, vec![0u8; 4096]);
        s.compress().unwrap();
        assert_eq!(decode(&s, 4096).map(|d| d.len()), Some(4096));
        assert_eq!(decode(&s, 4095), None);
    }
}
