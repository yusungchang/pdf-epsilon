//! Downsample and re-encode raster images.
//!
//! Only images this module fully understands are touched: 8-bit gray or RGB samples
//! (device, calibrated or ICC based), stored raw, deflated, LZW'd or as baseline JPEG.
//! Anything else is left exactly as it was: CMYK, indexed and spot colour, 1-bit and
//! 16-bit images, JPEG 2000, JBIG2, CCITT fax, images with a /Decode array or a
//! colour-key /Mask, and stencil masks.

use std::collections::{HashMap, HashSet};
use std::io::Write;

use image::{DynamicImage, GrayImage, ImageFormat, RgbImage};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use serde::{Deserialize, Serialize};

use crate::placement::Size;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagePolicy {
    /// Highest resolution kept, in pixels per inch of drawn size.
    pub max_dpi: f32,
    /// JPEG quality for photographic images, 1 to 100.
    pub jpeg_quality: u8,
}

#[derive(Default)]
pub struct Stats {
    pub seen: u32,
    pub downsampled: u32,
    pub reencoded: u32,
    /// Every image that was replaced, with its encoded size before and after.
    pub changed: Vec<Change>,
}

pub struct Change {
    pub id: ObjectId,
    pub before: usize,
    pub after: usize,
    pub width: u32,
    pub height: u32,
    pub downsampled: bool,
}

/// Downsample only when an image has this much more resolution than needed;
/// a small reduction is not worth a resample.
const DOWNSAMPLE_THRESHOLD: f32 = 1.25;
/// Re-encoding an existing JPEG without downsampling loses a generation of quality,
/// so it is only accepted when it saves at least this share of the bytes.
const JPEG_REENCODE_MIN_SAVING: f32 = 0.25;
/// Sampled images with more distinct colours than this are treated as photographs.
const PHOTO_COLOURS: usize = 1024;

#[derive(Clone, Copy, PartialEq)]
enum Space {
    Gray,
    Rgb,
}

impl Space {
    fn channels(self) -> usize {
        match self {
            Space::Gray => 1,
            Space::Rgb => 3,
        }
    }
}

fn is_image(obj: &Object) -> bool {
    matches!(obj, Object::Stream(s) if matches!(s.dict.get(b"Subtype").and_then(|o| o.as_name()), Ok(b"Image")))
}

pub fn count(doc: &Document) -> u32 {
    doc.objects.values().filter(|o| is_image(o)).count() as u32
}

/// Which images are soft masks of which, and which must keep their size.
struct Context {
    parent_of: HashMap<ObjectId, ObjectId>,
    fixed_size: HashSet<ObjectId>,
}

fn context(doc: &Document, ids: &[ObjectId]) -> Context {
    // Soft masks are judged by their parent's drawn size and are never made lossy.
    // A /Matte soft mask must keep its parent's dimensions, so neither is resized.
    let mut parent_of: HashMap<ObjectId, ObjectId> = HashMap::new();
    let mut fixed_size: HashSet<ObjectId> = HashSet::new();
    for id in ids {
        let Ok(Object::Stream(s)) = doc.get_object(*id) else { continue };
        if let Ok(Object::Reference(mask)) = s.dict.get(b"SMask") {
            parent_of.insert(*mask, *id);
            if let Ok(Object::Stream(m)) = doc.get_object(*mask) {
                if m.dict.has(b"Matte") {
                    fixed_size.insert(*id);
                    fixed_size.insert(*mask);
                }
            }
        }
    }
    Context { parent_of, fixed_size }
}

impl Context {
    fn job(&self, id: ObjectId, sizes: &HashMap<ObjectId, Size>, fallback: Size) -> Job {
        let drawn = sizes
            .get(&id)
            .or_else(|| self.parent_of.get(&id).and_then(|p| sizes.get(p)))
            .copied()
            .filter(|(w, h)| *w >= 1.0 && *h >= 1.0)
            .unwrap_or(fallback);
        Job { drawn, is_soft_mask: self.parent_of.contains_key(&id), may_resize: !self.fixed_size.contains(&id) }
    }
}

/// Recompresses every image the policy can improve. `progress` gets the share of images
/// done, from 0 to 1.
pub fn recompress(
    doc: &mut Document,
    policy: &ImagePolicy,
    sizes: &HashMap<ObjectId, Size>,
    fallback: Size,
    progress: &dyn Fn(f32),
) -> Stats {
    let mut stats = Stats::default();
    let ids: Vec<ObjectId> = doc.objects.iter().filter(|(_, o)| is_image(o)).map(|(id, _)| *id).collect();
    stats.seen = ids.len() as u32;
    let ctx = context(doc, &ids);

    let total = ids.len().max(1) as f32;
    for (i, id) in ids.into_iter().enumerate() {
        progress(i as f32 / total);
        let job = ctx.job(id, sizes, fallback);
        let Ok(Object::Stream(stream)) = doc.get_object(id) else { continue };
        let before = stream.content.len();
        let Some(done) = process(doc, stream, policy, job) else { continue };
        if done.downsampled {
            stats.downsampled += 1;
        }
        if done.reencoded {
            stats.reencoded += 1;
        }
        stats.changed.push(Change {
            id,
            before,
            after: done.stream.content.len(),
            width: int(doc, &done.stream.dict, b"Width").unwrap_or(0) as u32,
            height: int(doc, &done.stream.dict, b"Height").unwrap_or(0) as u32,
            downsampled: done.downsampled,
        });
        doc.objects.insert(id, Object::Stream(done.stream));
    }
    progress(1.0);
    stats
}

struct Job {
    drawn: Size,
    is_soft_mask: bool,
    may_resize: bool,
}

struct Done {
    stream: Stream,
    downsampled: bool,
    reencoded: bool,
}

/// The pixels of an image this module understands, and whether it was stored as JPEG.
fn decode_pixels(doc: &Document, stream: &Stream, is_soft_mask: bool) -> Option<(DynamicImage, bool)> {
    let dict = &stream.dict;
    if unsupported(doc, dict, is_soft_mask).is_some() {
        return None;
    }
    let width = u32::try_from(int(doc, dict, b"Width")?).ok()?;
    let height = u32::try_from(int(doc, dict, b"Height")?).ok()?;
    if width == 0 || height == 0 || width as u64 * height as u64 > crate::limits::IMAGE_PIXELS {
        return None;
    }
    let space = colour_space(doc, dict, is_soft_mask)?;
    let decoded = match filters(dict).as_slice() {
        [f] if f == b"DCTDecode" => {
            let img = image::load_from_memory_with_format(&stream.content, ImageFormat::Jpeg).ok()?;
            let img = match (space, img) {
                (Space::Gray, DynamicImage::ImageLuma8(g)) => DynamicImage::ImageLuma8(g),
                (Space::Rgb, DynamicImage::ImageRgb8(c)) => DynamicImage::ImageRgb8(c),
                _ => return None,
            };
            if img.width() != width || img.height() != height {
                return None;
            }
            (img, true)
        }
        [] => (raw_image(&stream.content, dict, doc, space, width, height)?, false),
        [f] if f == b"FlateDecode" || f == b"LZWDecode" => {
            // Never decode more than the pixels can use, so a bomb stops at the image's own size.
            let expected = width as usize * height as usize * space.channels();
            let data = crate::limits::decode(stream, expected.saturating_add(expected / 8 + 4096).min(crate::limits::STREAM))?;
            (raw_image(&data, dict, doc, space, width, height)?, false)
        }
        _ => return None,
    };
    Some(decoded)
}

fn process(doc: &Document, stream: &Stream, policy: &ImagePolicy, job: Job) -> Option<Done> {
    let dict = &stream.dict;
    let original_len = stream.content.len();
    let (pixels, was_jpeg) = decode_pixels(doc, stream, job.is_soft_mask)?;
    let (width, height) = (pixels.width(), pixels.height());

    // How many pixels does the drawn size actually need?
    let need_w = job.drawn.0 / 72.0 * policy.max_dpi;
    let need_h = job.drawn.1 / 72.0 * policy.max_dpi;
    let scale = (need_w / width as f32).max(need_h / height as f32).min(1.0);
    let downsample = job.may_resize && scale * DOWNSAMPLE_THRESHOLD < 1.0;
    let pixels = if downsample {
        let w = ((width as f32 * scale).round() as u32).max(1);
        let h = ((height as f32 * scale).round() as u32).max(1);
        pixels.resize_exact(w, h, image::imageops::FilterType::Lanczos3)
    } else {
        pixels
    };

    let lossy = !job.is_soft_mask && (was_jpeg || looks_photographic(&pixels));
    let (content, filter) = if lossy {
        (jpeg(&pixels, policy.jpeg_quality)?, "DCTDecode")
    } else {
        (deflate(pixels.as_bytes())?, "FlateDecode")
    };

    let worth_it = if was_jpeg && !downsample {
        (content.len() as f32) <= original_len as f32 * (1.0 - JPEG_REENCODE_MIN_SAVING)
    } else {
        content.len() < original_len
    };
    if !worth_it {
        return None;
    }

    let mut new_dict = dict.clone();
    new_dict.set("Filter", Object::Name(filter.as_bytes().to_vec()));
    new_dict.remove(b"DecodeParms");
    new_dict.set("Width", Object::Integer(pixels.width() as i64));
    new_dict.set("Height", Object::Integer(pixels.height() as i64));
    new_dict.set("BitsPerComponent", Object::Integer(8));
    let mut new_stream = Stream::new(new_dict, content);
    new_stream.allows_compression = false;
    Some(Done { stream: new_stream, downsampled: downsample, reencoded: true })
}

/// Why this module will not touch an image, as a code (`cmyk`, `jpeg2000`, ...), or `None`
/// when it can try. The interface translates the code; see `imageReason.*` in `ui/locales`.
/// Decoding can still fail later, in which case the image is also left alone.
pub fn unsupported(doc: &Document, dict: &Dictionary, is_soft_mask: bool) -> Option<&'static str> {
    if dict.get(b"ImageMask").and_then(|o| o.as_bool()).unwrap_or(false) {
        return Some("stencilMask");
    }
    if dict.has(b"Decode") {
        return Some("decodeArray");
    }
    if matches!(dict.get(b"Mask"), Ok(Object::Array(_))) {
        return Some("colourKeyMask");
    }
    let pixels = int(doc, dict, b"Width").unwrap_or(0).max(0) as u64 * int(doc, dict, b"Height").unwrap_or(0).max(0) as u64;
    if pixels > crate::limits::IMAGE_PIXELS {
        return Some("tooLarge");
    }
    // Parameters the decoder would misread (see `limits::decodable`), and JPEG parameters
    // such as /ColorTransform, which the JPEG decoder does not take, change the pixels.
    if !crate::limits::decodable_params(dict) {
        return Some("unusualFilter");
    }
    let filters = filters(dict);
    if filters.as_slice() == [b"DCTDecode".to_vec()] && !matches!(dict.get(b"DecodeParms"), Err(_) | Ok(Object::Null)) {
        return Some("unusualFilter");
    }
    let is_jpeg = match filters.as_slice() {
        [] => false,
        [f] if f == b"DCTDecode" => true,
        [f] if f == b"FlateDecode" || f == b"LZWDecode" => false,
        [f] if f == b"JPXDecode" => return Some("jpeg2000"),
        [f] if f == b"JBIG2Decode" => return Some("jbig2"),
        [f] if f == b"CCITTFaxDecode" => return Some("ccittFax"),
        [_] => return Some("unusualFilter"),
        _ => return Some("chainedFilters"),
    };
    if colour_space(doc, dict, is_soft_mask).is_none() {
        return Some(match colour_name(doc, dict).as_str() {
            "CMYK" => "cmyk",
            "Indexed" => "indexed",
            "Separation" | "DeviceN" => "spotColour",
            "Lab" => "lab",
            _ => "unusualColourSpace",
        });
    }
    if !is_jpeg {
        match int(doc, dict, b"BitsPerComponent").unwrap_or(8) {
            8 => {}
            1 => return Some("oneBit"),
            16 => return Some("sixteenBit"),
            _ => return Some("unusualBitDepth"),
        }
    }
    None
}

/// A short name for an image's colour space: Gray, RGB, CMYK, Indexed, Separation, DeviceN, Lab, or the raw name.
pub fn colour_name(doc: &Document, dict: &Dictionary) -> String {
    let Ok(cs) = dict.get(b"ColorSpace") else { return "none".into() };
    let cs = match cs {
        Object::Reference(id) => match doc.get_object(*id) {
            Ok(o) => o,
            Err(_) => return "unknown".into(),
        },
        o => o,
    };
    let name = |n: &[u8]| match n {
        b"DeviceGray" | b"G" | b"CalGray" => "Gray".to_string(),
        b"DeviceRGB" | b"RGB" | b"CalRGB" => "RGB".to_string(),
        b"DeviceCMYK" | b"CMYK" => "CMYK".to_string(),
        b"I" => "Indexed".to_string(),
        other => String::from_utf8_lossy(other).into_owned(),
    };
    match cs {
        Object::Name(n) => name(n),
        Object::Array(a) => match a.first().and_then(|o| o.as_name().ok()) {
            Some(b"ICCBased") => {
                let n = a
                    .get(1)
                    .and_then(|o| o.as_reference().ok())
                    .and_then(|id| doc.get_object(id).ok())
                    .and_then(|o| o.as_stream().ok())
                    .and_then(|s| s.dict.get(b"N").ok())
                    .and_then(|n| n.as_i64().ok());
                match n {
                    Some(1) => "Gray".into(),
                    Some(3) => "RGB".into(),
                    Some(4) => "CMYK".into(),
                    _ => "ICC".into(),
                }
            }
            Some(n) => name(n),
            None => "unknown".into(),
        },
        _ => "unknown".into(),
    }
}

/// The image's filter chain as written, e.g. `DCTDecode`, or `none`.
pub fn encoding_name(dict: &Dictionary) -> String {
    let f = filters(dict);
    if f.is_empty() {
        return "none".into();
    }
    f.iter().map(|n| String::from_utf8_lossy(n).into_owned()).collect::<Vec<_>>().join(" + ")
}

fn raw_image(data: &[u8], dict: &Dictionary, doc: &Document, space: Space, w: u32, h: u32) -> Option<DynamicImage> {
    if int(doc, dict, b"BitsPerComponent").unwrap_or(8) != 8 {
        return None;
    }
    let len = w as usize * h as usize * space.channels();
    // Exactly the pixels, or at most one row more (some writers pad). More than that means the
    // data was not decoded as its writer meant, for example a predictor left unapplied.
    if data.len() < len || data.len() > len + w as usize * space.channels() {
        return None;
    }
    let data = data[..len].to_vec();
    match space {
        Space::Gray => GrayImage::from_raw(w, h, data).map(DynamicImage::ImageLuma8),
        Space::Rgb => RgbImage::from_raw(w, h, data).map(DynamicImage::ImageRgb8),
    }
}

/// Counts distinct colours on a sample. Screenshots, charts and line art have few;
/// JPEG would put ringing round their edges, so they stay lossless.
fn looks_photographic(img: &DynamicImage) -> bool {
    let bytes = img.as_bytes();
    let channels = img.color().channel_count() as usize;
    let pixels = bytes.len() / channels;
    let step = (pixels / 40_000).max(1);
    let mut seen = HashSet::new();
    for i in (0..pixels).step_by(step) {
        let px = &bytes[i * channels..(i + 1) * channels];
        let key = px.iter().fold(0u32, |k, b| (k << 8) | *b as u32);
        seen.insert(key);
        if seen.len() > PHOTO_COLOURS {
            return true;
        }
    }
    false
}

fn jpeg(img: &DynamicImage, quality: u8) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    img.write_with_encoder(encoder).ok()?;
    Some(out)
}

fn deflate(data: &[u8]) -> Option<Vec<u8>> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    enc.write_all(data).ok()?;
    enc.finish().ok()
}

pub(crate) fn int(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<i64> {
    let obj = dict.get(key).ok()?;
    let obj = match obj {
        Object::Reference(id) => doc.get_object(*id).ok()?,
        o => o,
    };
    obj.as_i64().ok()
}

fn filters(dict: &Dictionary) -> Vec<Vec<u8>> {
    match dict.get(b"Filter") {
        Ok(Object::Name(n)) => vec![n.clone()],
        Ok(Object::Array(a)) => a.iter().filter_map(|o| o.as_name().ok().map(|n| n.to_vec())).collect(),
        _ => Vec::new(),
    }
}

fn colour_space(doc: &Document, dict: &Dictionary, is_soft_mask: bool) -> Option<Space> {
    let cs = match dict.get(b"ColorSpace") {
        Ok(o) => o,
        Err(_) => return is_soft_mask.then_some(Space::Gray),
    };
    let cs = match cs {
        Object::Reference(id) => doc.get_object(*id).ok()?,
        o => o,
    };
    match cs {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"G" => Some(Space::Gray),
            b"DeviceRGB" | b"RGB" => Some(Space::Rgb),
            _ => None,
        },
        Object::Array(a) => match a.first()?.as_name().ok()? {
            b"CalGray" => Some(Space::Gray),
            b"CalRGB" => Some(Space::Rgb),
            b"ICCBased" => {
                let profile = doc.get_object(a.get(1)?.as_reference().ok()?).ok()?.as_stream().ok()?;
                match profile.dict.get(b"N").ok()?.as_i64().ok()? {
                    1 => Some(Space::Gray),
                    3 => Some(Space::Rgb),
                    _ => None,
                }
            }
            _ => None,
        },
        _ => None,
    }
}
