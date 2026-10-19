//! Where is each image drawn, and how big?
//!
//! Walks every page's content stream, tracking the current transformation matrix
//! through `q`, `Q` and `cm`, and records the drawn size of each image XObject at
//! each `Do`. Form XObjects are followed with their own matrix and resources.
//! The largest drawn size of each image is what its resolution is judged against.

use std::collections::{HashMap, HashSet};

use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId};

/// Drawn width and height, in PDF points (1/72 inch).
pub type Size = (f32, f32);

#[derive(Clone, Copy)]
pub struct Matrix([f32; 6]);

impl Matrix {
    const IDENTITY: Matrix = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    /// `self` applied first, then `then`.
    fn then(self, then: Matrix) -> Matrix {
        let [a, b, c, d, e, f] = self.0;
        let [na, nb, nc, nd, ne, nf] = then.0;
        Matrix([
            a * na + b * nc,
            a * nb + b * nd,
            c * na + d * nc,
            c * nb + d * nd,
            e * na + f * nc + ne,
            e * nb + f * nd + nf,
        ])
    }

    /// Axis-aligned bounds of the transformed unit square: x0, y0, x1, y1.
    pub fn unit_bounds(self) -> [f32; 4] {
        let [a, b, c, d, e, f] = self.0;
        let xs = [e, a + e, c + e, a + c + e];
        let ys = [f, b + f, d + f, b + d + f];
        let min = |v: [f32; 4]| v.iter().copied().fold(f32::INFINITY, f32::min);
        let max = |v: [f32; 4]| v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        [min(xs), min(ys), max(xs), max(ys)]
    }

    /// Lengths of the unit square's two sides once transformed.
    pub fn unit_size(self) -> Size {
        let [a, b, c, d, _, _] = self.0;
        ((a * a + b * b).sqrt(), (c * c + d * d).sqrt())
    }

    fn from_objects(values: &[Object]) -> Option<Matrix> {
        if values.len() != 6 {
            return None;
        }
        let mut m = [0.0f32; 6];
        for (slot, v) in m.iter_mut().zip(values) {
            *slot = number(v)?;
        }
        Some(Matrix(m))
    }
}

fn number(v: &Object) -> Option<f32> {
    match v {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r as f32),
        _ => None,
    }
}

/// Something drawn by a page, directly or inside a form XObject.
pub enum Draw<'a> {
    /// An image XObject, with the matrix that maps its unit square onto the page.
    Image(ObjectId, Matrix),
    /// A form XObject, reported once per `Do` before its own content is walked.
    Form(ObjectId),
    /// A font selected with `Tf`.
    Font(ObjectId),
    /// An inline image (`BI` ... `EI`) and its encoded bytes.
    InlineImage(&'a lopdf::Stream),
}

/// Largest drawn size of every image found in page content.
pub fn display_sizes(doc: &Document) -> HashMap<ObjectId, Size> {
    let mut sizes: HashMap<ObjectId, Size> = HashMap::new();
    for (_, page_id) in doc.get_pages() {
        visit_page(doc, page_id, &mut |draw| {
            if let Draw::Image(id, ctm) = draw {
                let (w, h) = ctm.unit_size();
                let entry = sizes.entry(id).or_insert((0.0, 0.0));
                entry.0 = entry.0.max(w);
                entry.1 = entry.1.max(h);
            }
        });
    }
    sizes
}

/// Walks one page's content, calling `visit` for everything it draws.
/// Returns false when the content could not be read or decoded.
pub fn visit_page(doc: &Document, page_id: ObjectId, visit: &mut dyn FnMut(Draw)) -> bool {
    // Bounded, so a hostile file cannot inflate without limit (see limits.rs).
    let Some(content) = crate::limits::page_content(doc, page_id) else { return false };
    let resources = page_resources(doc, page_id);
    let mut visiting = HashSet::new();
    walk(doc, &content, &resources, Matrix::IDENTITY, visit, &mut visiting, 0)
}

/// The page's resource dictionaries, nearest first (inherited ones follow).
pub(crate) fn page_resources(doc: &Document, page_id: ObjectId) -> Vec<Dictionary> {
    let mut out = Vec::new();
    if let Ok((own, inherited)) = doc.get_page_resources(page_id) {
        if let Some(d) = own {
            out.push(d.clone());
        }
        for id in inherited {
            if let Ok(d) = doc.get_dictionary(id) {
                out.push(d.clone());
            }
        }
    }
    out
}

pub(crate) fn deref<'a>(doc: &'a Document, obj: &'a Object) -> &'a Object {
    match obj {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(obj),
        _ => obj,
    }
}

pub(crate) fn lookup(doc: &Document, resources: &[Dictionary], kind: &[u8], name: &[u8]) -> Option<ObjectId> {
    for res in resources {
        let Ok(entries) = res.get(kind) else { continue };
        let Ok(entries) = deref(doc, entries).as_dict() else { continue };
        if let Ok(Object::Reference(id)) = entries.get(name) {
            return Some(*id);
        }
    }
    None
}

fn walk(
    doc: &Document,
    content: &[u8],
    resources: &[Dictionary],
    base: Matrix,
    visit: &mut dyn FnMut(Draw),
    visiting: &mut HashSet<ObjectId>,
    depth: u32,
) -> bool {
    if depth > 16 {
        return true;
    }
    // Content that does not decode (some inline images, broken producers) is skipped;
    // its images fall back to the page-size limit.
    if !crate::limits::operations_ok(content) {
        return false;
    }
    let Ok(content) = Content::decode(content) else { return false };
    let mut ctm = base;
    let mut stack: Vec<Matrix> = Vec::new();

    for op in &content.operations {
        match op.operator.as_str() {
            "q" => stack.push(ctm),
            "Q" => ctm = stack.pop().unwrap_or(base),
            "cm" => {
                if let Some(m) = Matrix::from_objects(&op.operands) {
                    ctm = m.then(ctm);
                }
            }
            "Tf" => {
                let Some(Ok(name)) = op.operands.first().map(|o| o.as_name()) else { continue };
                if let Some(id) = lookup(doc, resources, b"Font", name) {
                    visit(Draw::Font(id));
                }
            }
            "BI" => {
                if let Some(Object::Stream(s)) = op.operands.first() {
                    visit(Draw::InlineImage(s));
                }
            }
            "Do" => {
                let Some(Ok(name)) = op.operands.first().map(|o| o.as_name()) else { continue };
                let Some(id) = lookup(doc, resources, b"XObject", name) else { continue };
                let Ok(Object::Stream(stream)) = doc.get_object(id) else { continue };
                match stream.dict.get(b"Subtype").and_then(|s| s.as_name()) {
                    Ok(b"Image") => visit(Draw::Image(id, ctm)),
                    Ok(b"Form") => {
                        visit(Draw::Form(id));
                        if !visiting.insert(id) {
                            continue;
                        }
                        let matrix = stream
                            .dict
                            .get(b"Matrix")
                            .ok()
                            .and_then(|m| m.as_array().ok())
                            .and_then(|a| Matrix::from_objects(a))
                            .unwrap_or(Matrix::IDENTITY);
                        let mut form_resources = Vec::new();
                        if let Ok(r) = stream.dict.get(b"Resources") {
                            if let Ok(d) = deref(doc, r).as_dict() {
                                form_resources.push(d.clone());
                            }
                        }
                        // Forms without their own resources use the page's.
                        form_resources.extend_from_slice(resources);
                        if let Some(data) = crate::limits::decode(stream, crate::limits::CONTENT) {
                            walk(doc, &data, &form_resources, matrix.then(ctm), visit, visiting, depth + 1);
                        }
                        visiting.remove(&id);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    true
}

/// The largest page's width and height in points. Images whose placement could not
/// be found (patterns, annotations, Type 3 fonts) are limited to this.
pub fn largest_page(doc: &Document) -> Size {
    let mut best = (612.0f32, 792.0f32);
    let mut found = false;
    for (_, page_id) in doc.get_pages() {
        let Some(rect) = page_box(doc, page_id, b"MediaBox") else { continue };
        let (w, h) = ((rect[2] - rect[0]).abs(), (rect[3] - rect[1]).abs());
        if !found {
            best = (w, h);
            found = true;
        } else {
            best = (best.0.max(w), best.1.max(h));
        }
    }
    // Either orientation may be used for the image.
    let side = best.0.max(best.1);
    (side, side)
}

/// The visible area of a page: its crop box, or its media box when it has none.
pub fn visible_box(doc: &Document, page_id: ObjectId) -> Option<[f32; 4]> {
    let b = page_box(doc, page_id, b"CropBox").or_else(|| page_box(doc, page_id, b"MediaBox"))?;
    Some([b[0].min(b[2]), b[1].min(b[3]), b[0].max(b[2]), b[1].max(b[3])])
}

/// A page box, inherited from the page tree when the page has none of its own.
pub fn page_box(doc: &Document, mut id: ObjectId, key: &[u8]) -> Option<[f32; 4]> {
    for _ in 0..32 {
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(b) = dict.get(key) {
            let arr = deref(doc, b).as_array().ok()?;
            if arr.len() == 4 {
                let v: Vec<f32> = arr.iter().filter_map(|o| number(deref(doc, o))).collect();
                if v.len() == 4 {
                    return Some([v[0], v[1], v[2], v[3]]);
                }
            }
            return None;
        }
        id = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_composition_scales_then_translates() {
        let scale = Matrix([200.0, 0.0, 0.0, 100.0, 0.0, 0.0]);
        let shift = Matrix([1.0, 0.0, 0.0, 1.0, 50.0, 50.0]);
        let m = scale.then(shift);
        assert_eq!(m.unit_size(), (200.0, 100.0));
        assert_eq!(m.0[4], 50.0);
    }

    #[test]
    fn rotation_keeps_side_lengths() {
        // 90 degree rotation of a 300 x 150 placement.
        let m = Matrix([0.0, 300.0, -150.0, 0.0, 0.0, 0.0]);
        assert_eq!(m.unit_size(), (300.0, 150.0));
    }
}
