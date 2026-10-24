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
///
/// Images are independent of each other, so they are worked on in parallel, then applied in
/// their original order: the result is the same as one at a time. How many decoded images may be
/// in memory at once is limited across every run in the process (see `Budget`).
pub fn recompress(
    doc: &mut Document,
    policy: &ImagePolicy,
    sizes: &HashMap<ObjectId, Size>,
    fallback: Size,
    share: Option<&Share>,
    progress: &dyn Fn(f32),
) -> Stats {
    let mut stats = Stats::default();
    let ids: Vec<ObjectId> = doc.objects.iter().filter(|(_, o)| is_image(o)).map(|(id, _)| *id).collect();
    stats.seen = ids.len() as u32;
    let ctx = context(doc, &ids);

    let total = ids.len().max(1) as f32;
    progress(0.0);
    let limit = image_memory();
    let results = in_parallel(doc, &ids, &|doc, id| {
        let job = ctx.job(id, sizes, fallback);
        let Ok(Object::Stream(stream)) = doc.get_object(id) else { return None };
        let _memory = BUDGET.take(decoded_bytes(doc, &stream.dict, job.is_soft_mask), limit);
        process(doc, stream, policy, job, share)
    }, &|done| progress(done as f32 / total));

    for (id, done) in ids.into_iter().zip(results) {
        let Some(done) = done else { continue };
        let before = match doc.get_object(id) {
            Ok(Object::Stream(s)) => s.content.len(),
            _ => continue,
        };
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

/// Runs `work` on every id across the machine's cores, returning the results in the order of
/// `ids`. `done` is called on this thread with the number finished so far. If it panics (a
/// cancellation, see `crate::Cancelled`), the workers stop after their current image and the
/// panic goes on.
fn in_parallel<T: Send>(
    doc: &Document,
    ids: &[ObjectId],
    work: &(dyn Fn(&Document, ObjectId) -> Option<T> + Sync),
    done: &dyn Fn(usize),
) -> Vec<Option<T>> {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(ids.len()).max(1);
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let mut results: Vec<Option<T>> = (0..ids.len()).map(|_| None).collect();
    let mut stopped = None;
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel::<(usize, Option<T>)>();
        for _ in 0..workers {
            let tx = tx.clone();
            let (next, stop) = (&next, &stop);
            scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= ids.len() || stop.load(Ordering::Relaxed) {
                    break;
                }
                if tx.send((i, work(doc, ids[i]))).is_err() {
                    break;
                }
            });
        }
        drop(tx);
        let mut finished = 0;
        for (i, r) in rx {
            results[i] = r;
            finished += 1;
            if let Err(p) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| done(finished))) {
                stop.store(true, Ordering::Relaxed);
                stopped = Some(p);
                break;
            }
        }
    });
    if let Some(p) = stopped {
        std::panic::resume_unwind(p);
    }
    results
}

/// How much memory working on an image takes: its decoded pixels (one byte per pixel for grey,
/// three for colour), plus as much again for a resized copy and the encoded result.
fn decoded_bytes(doc: &Document, dict: &Dictionary, is_soft_mask: bool) -> u64 {
    let w = int(doc, dict, b"Width").unwrap_or(0).clamp(0, 1 << 20) as u64;
    let h = int(doc, dict, b"Height").unwrap_or(0).clamp(0, 1 << 20) as u64;
    let channels = colour_space(doc, dict, is_soft_mask).map(|s| s.channels() as u64).unwrap_or(3);
    (w * h * channels).saturating_mul(2).min(crate::limits::IMAGE_PIXELS * 6)
}

/// At most this much memory for images worked on at once, on any computer.
const IMAGE_MEMORY: u64 = 768 << 20;

/// Memory for images worked on at once, sized to this computer: an eighth of installed memory,
/// at most `IMAGE_MEMORY`, and at most a quarter of what is free now. Below 96 MB it is zero:
/// one image at a time, as before 0.10.0. Unknown installed memory counts as 2 GB.
pub(crate) fn image_memory() -> u64 {
    budget_for(crate::system::memory())
}

fn budget_for((total, available): (u64, u64)) -> u64 {
    let mut limit = if total > 0 { (total / 8).min(IMAGE_MEMORY) } else { 256 << 20 };
    if available > 0 {
        limit = limit.min(available / 4);
    }
    if limit < 96 << 20 { 0 } else { limit }
}

/// Memory for decoded images, shared by every run in the process (an analysis runs four
/// presets at once). An image waits until its share fits within the run's limit; one alone always
/// may run, so a single large image is no slower than before. Before 0.10.0 each run worked on
/// one image at a time.
struct Budget {
    used: std::sync::Mutex<u64>,
    freed: std::sync::Condvar,
}

static BUDGET: Budget = Budget { used: std::sync::Mutex::new(0), freed: std::sync::Condvar::new() };

struct Taken(u64);

impl Budget {
    fn take(&'static self, bytes: u64, limit: u64) -> Taken {
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        while *used > 0 && *used + bytes > limit {
            used = self.freed.wait(used).unwrap_or_else(|e| e.into_inner());
        }
        *used += bytes;
        #[cfg(test)]
        {
            let now = AT_ONCE.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            MOST_AT_ONCE.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
        }
        Taken(bytes)
    }
}

/// Images being worked on now, and the most at once: tests check the budget with these.
#[cfg(test)]
static AT_ONCE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static MOST_AT_ONCE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl Drop for Taken {
    fn drop(&mut self) {
        #[cfg(test)]
        AT_ONCE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        let mut used = BUDGET.used.lock().unwrap_or_else(|e| e.into_inner());
        *used -= self.0;
        BUDGET.freed.notify_all();
    }
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

/// What the runs of one analysis share: each image decoded once for all the presets that change
/// images, and dropped as soon as the last of them has it (see `memo.rs`). Saving a single copy
/// shares nothing and keeps nothing.
pub struct Share {
    decoded: crate::memo::Memo<std::sync::Arc<(DynamicImage, bool)>>,
}

impl Share {
    /// For `runs` runs that each decode the document's images.
    pub fn new(runs: usize) -> Share {
        Share { decoded: crate::memo::Memo::new(image_memory() as usize, runs) }
    }
}

/// `decode_pixels`, worked out once for the same image across the presets of an analysis. The
/// key holds everything decoding depends on: the stream's bytes and the dictionary entries it
/// reads, with the colour space as resolved in this document.
fn decoded_pixels(doc: &Document, stream: &Stream, is_soft_mask: bool, share: Option<&Share>) -> Option<std::sync::Arc<(DynamicImage, bool)>> {
    let Some(share) = share else { return decode_pixels(doc, stream, is_soft_mask).map(std::sync::Arc::new) };
    let dict = &stream.dict;
    if unsupported(doc, dict, is_soft_mask).is_some() {
        return None;
    }
    let space = colour_space(doc, dict, is_soft_mask)?;
    let entries = format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}",
        int(doc, dict, b"Width"),
        int(doc, dict, b"Height"),
        int(doc, dict, b"BitsPerComponent"),
        dict.get(b"Filter").ok(),
        dict.get(b"DecodeParms").ok(),
        space.channels(),
        is_soft_mask
    );
    let key = crate::memo::key(&[b"image", entries.as_bytes(), &stream.content]);
    share.decoded.get_or(key, |d| d.0.as_bytes().len(), || decode_pixels(doc, stream, is_soft_mask).map(std::sync::Arc::new))
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

fn process(doc: &Document, stream: &Stream, policy: &ImagePolicy, job: Job, share: Option<&Share>) -> Option<Done> {
    let dict = &stream.dict;
    let original_len = stream.content.len();
    let decoded = decoded_pixels(doc, stream, job.is_soft_mask, share)?;
    let (original, was_jpeg) = (&decoded.0, decoded.1);
    let (width, height) = (original.width(), original.height());

    // How many pixels does the drawn size actually need?
    let need_w = job.drawn.0 / 72.0 * policy.max_dpi;
    let need_h = job.drawn.1 / 72.0 * policy.max_dpi;
    let scale = (need_w / width as f32).max(need_h / height as f32).min(1.0);
    let downsample = job.may_resize && scale * DOWNSAMPLE_THRESHOLD < 1.0;
    let resized;
    let pixels: &DynamicImage = if downsample {
        let w = ((width as f32 * scale).round() as u32).max(1);
        let h = ((height as f32 * scale).round() as u32).max(1);
        resized = resample(original, w, h);
        &resized
    } else {
        original
    };

    let lossy = !job.is_soft_mask && (was_jpeg || looks_photographic(pixels));
    let (content, filter) = if lossy {
        (jpeg(pixels, policy.jpeg_quality)?, "DCTDecode")
    } else {
        (deflate(pixels.as_bytes())?, "FlateDecode")
    };

    // Read the new image back as a viewer will, and compare it with the pixels meant.
    if !reads_back(&content, lossy, pixels) {
        return None;
    }

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

/// Largest average difference, per channel out of 255, allowed between a JPEG and the pixels
/// it was made from. Quality 60 stays well under it on photographs.
const JPEG_MAX_DIFFERENCE: f64 = 20.0;

/// Whether `content` decodes back to `pixels`: exactly when lossless, closely when JPEG.
/// Catches an encoder that wrote something other than the picture; the image is then kept.
fn reads_back(content: &[u8], lossy: bool, pixels: &DynamicImage) -> bool {
    let want = pixels.as_bytes();
    if !lossy {
        use std::io::Read;
        let mut got = Vec::with_capacity(want.len());
        let reader = flate2::read::ZlibDecoder::new(content).take(want.len() as u64 + 1);
        return std::io::BufReader::new(reader).read_to_end(&mut got).is_ok() && got == want;
    }
    let Ok(img) = image::load_from_memory_with_format(content, ImageFormat::Jpeg) else { return false };
    if img.width() != pixels.width() || img.height() != pixels.height() {
        return false;
    }
    let got = match pixels {
        DynamicImage::ImageLuma8(_) => img.to_luma8().into_raw(),
        _ => img.to_rgb8().into_raw(),
    };
    if got.len() != want.len() {
        return false;
    }
    // A sample of about 200,000 values is plenty to tell a picture from a broken one.
    let step = (want.len() / 200_000).max(1);
    let (mut sum, mut n) = (0u64, 0u64);
    for i in (0..want.len()).step_by(step) {
        sum += (want[i] as i32 - got[i] as i32).unsigned_abs() as u64;
        n += 1;
    }
    n > 0 && sum as f64 / n as f64 <= JPEG_MAX_DIFFERENCE
}

/// The image at a new size, with a Lanczos3 filter. fast_image_resize uses the processor's
/// vector instructions: about four times faster than the image crate's filter, and within
/// 0.1 of 255 of it on average. Falls back to the image crate if it refuses.
fn resample(img: &DynamicImage, w: u32, h: u32) -> DynamicImage {
    use fast_image_resize::{images::Image, FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
    let (kind, gray) = match img {
        DynamicImage::ImageLuma8(_) => (PixelType::U8, true),
        _ => (PixelType::U8x3, false),
    };
    let fast = || -> Option<DynamicImage> {
        let src = Image::from_vec_u8(img.width(), img.height(), img.as_bytes().to_vec(), kind).ok()?;
        let mut dst = Image::new(w, h, kind);
        let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
        Resizer::new().resize(&src, &mut dst, &options).ok()?;
        let out = dst.into_vec();
        if gray {
            GrayImage::from_raw(w, h, out).map(DynamicImage::ImageLuma8)
        } else {
            RgbImage::from_raw(w, h, out).map(DynamicImage::ImageRgb8)
        }
    };
    fast().unwrap_or_else(|| img.resize_exact(w, h, image::imageops::FilterType::Lanczos3))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32, shift: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 2 + shift) as u8, (y * 2) as u8, ((x + y + shift) / 2) as u8])))
    }

    /// A document with `n` photo-like 800 × 600 images, each drawn at full page size.
    fn photos(n: usize) -> Document {
        let mut doc = Document::with_version("1.7");
        let pages = doc.new_object_id();
        let mut xobjects = Dictionary::new();
        let mut content = String::new();
        for i in 0..n {
            let px = gradient(800, 600, i as u32 * 7);
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
            z.write_all(px.as_bytes()).unwrap();
            let mut s = Stream::new(
                lopdf::dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 800, "Height" => 600, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8, "Filter" => "FlateDecode" },
                z.finish().unwrap(),
            );
            s.allows_compression = false;
            xobjects.set(format!("I{i}"), doc.add_object(s));
            content.push_str(&format!("q 40 0 0 30 0 0 cm /I{i} Do Q "));
        }
        let c = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
        let page = doc.add_object(lopdf::dictionary! { "Type" => "Page", "Parent" => pages, "Contents" => c, "Resources" => lopdf::dictionary! { "XObject" => xobjects } });
        doc.objects.insert(pages, Object::Dictionary(lopdf::dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }));
        doc
    }

    // One test for both, since they share the pretend machine and the counters.
    #[test]
    fn little_free_memory_means_one_image_at_a_time() {
        let policy = ImagePolicy { max_dpi: 150.0, jpeg_quality: 75 };
        let run = |total: u64, free: u64| {
            *crate::system::PRETEND.lock().unwrap() = Some((total, free));
            MOST_AT_ONCE.store(0, std::sync::atomic::Ordering::SeqCst);
            let mut doc = photos(12);
            let stats = recompress(&mut doc, &policy, &HashMap::new(), (40.0, 30.0), None, &|_| {});
            assert_eq!(stats.seen, 12);
            MOST_AT_ONCE.load(std::sync::atomic::Ordering::SeqCst)
        };
        const GB: u64 = 1 << 30;
        let short = run(16 * GB, 300 << 20);
        let plenty = run(36 * GB, 20 * GB);
        *crate::system::PRETEND.lock().unwrap() = None;
        assert_eq!(short, 1, "with 300 MB free, images must go one at a time");
        if std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) > 1 {
            assert!(plenty > 1, "with memory to spare, images should be worked on together");
        }
    }

    #[test]
    fn image_memory_follows_the_computer() {
        const GB: u64 = 1 << 30;
        assert_eq!(budget_for((16 * GB, 8 * GB)), IMAGE_MEMORY);
        assert_eq!(budget_for((4 * GB, 3 * GB)), GB / 2);
        // Little free now: a quarter of what is free.
        assert_eq!(budget_for((16 * GB, GB)), GB / 4);
        // Very little: one image at a time.
        assert_eq!(budget_for((16 * GB, 300 << 20)), 0);
        assert_eq!(budget_for((512 << 20, 0)), 0);
        // Unknown: a cautious 256 MB.
        assert_eq!(budget_for((0, 0)), 256 << 20);
    }

    #[test]
    fn new_images_are_read_back_before_they_are_used() {
        let picture = gradient(120, 80, 0);
        let good = jpeg(&picture, 60).unwrap();
        assert!(reads_back(&good, true, &picture));
        // A JPEG of another picture, or of the wrong size, is refused.
        assert!(!reads_back(&jpeg(&gradient(120, 80, 128), 60).unwrap(), true, &picture));
        assert!(!reads_back(&jpeg(&gradient(119, 80, 0), 60).unwrap(), true, &picture));
        // Lossless must match exactly.
        let exact = deflate(picture.as_bytes()).unwrap();
        assert!(reads_back(&exact, false, &picture));
        let mut off = picture.as_bytes().to_vec();
        off[7] ^= 1;
        assert!(!reads_back(&deflate(&off).unwrap(), false, &picture));
    }
}
