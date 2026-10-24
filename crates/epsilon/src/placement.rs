//! Where is each image drawn, and how big?
//!
//! Walks every page's content stream, tracking the current transformation matrix
//! through `q`, `Q` and `cm`, and records the drawn size of each image XObject at
//! each `Do`. Form XObjects are followed with their own matrix and resources.
//! The largest drawn size of each image is what its resolution is judged against.
//!
//! A page's `/UserUnit` scales everything on it. Each page's walk has a budget of drawing
//! operations, forms included, so a form drawn many times inside forms drawn many times cannot
//! make the walk run for hours; a page over budget counts as unreadable.

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
    let mut budget = crate::limits::PAGE_OPERATIONS;
    let u = user_unit(doc, page_id);
    walk(doc, &content, &resources, Matrix([u, 0.0, 0.0, u, 0.0, 0.0]), visit, &mut visiting, &mut budget, 0)
}

/// How many points one unit on this page is (`/UserUnit`, PDF 1.6). Large-format drawings use
/// it to go beyond the 200-inch page limit.
pub fn user_unit(doc: &Document, page_id: ObjectId) -> f32 {
    doc.get_dictionary(page_id)
        .ok()
        .and_then(|p| p.get(b"UserUnit").ok())
        .and_then(|o| number(deref(doc, o)))
        .filter(|u| u.is_finite() && *u > 0.0 && *u <= 75_000.0)
        .unwrap_or(1.0)
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
    budget: &mut usize,
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
    if content.operations.len() > *budget {
        *budget = 0;
        return false;
    }
    *budget -= content.operations.len();
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
                            walk(doc, &data, &form_resources, matrix.then(ctm), visit, visiting, budget, depth + 1);
                        }
                        visiting.remove(&id);
                        if *budget == 0 {
                            return false;
                        }
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
        let u = user_unit(doc, page_id);
        let (w, h) = ((rect[2] - rect[0]).abs() * u, (rect[3] - rect[1]).abs() * u);
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
/// In points, like the matrices `visit_page` reports (so `/UserUnit` is applied).
pub fn visible_box(doc: &Document, page_id: ObjectId) -> Option<[f32; 4]> {
    let b = page_box(doc, page_id, b"CropBox").or_else(|| page_box(doc, page_id, b"MediaBox"))?;
    let u = user_unit(doc, page_id);
    Some([b[0].min(b[2]) * u, b[1].min(b[3]) * u, b[0].max(b[2]) * u, b[1].max(b[3]) * u])
}

/// Images drawn somewhere `visit_page` does not measure: annotation appearances, tiling
/// patterns, soft-mask groups and Type 3 glyphs. Their size there is unknown, so they must not
/// be judged by the size found on the page alone.
pub fn images_outside_content(doc: &Document) -> HashSet<ObjectId> {
    let mut g = Gather { out: HashSet::new(), seen: HashSet::new(), nodes: 0 };
    for (_, page_id) in doc.get_pages() {
        for res in page_resources(doc, page_id) {
            g.resources(doc, &res, false, 0);
        }
        let annots = doc.get_dictionary(page_id).ok().and_then(|p| p.get(b"Annots").ok()).and_then(|o| deref(doc, o).as_array().ok());
        for a in annots.into_iter().flatten() {
            let Some(ap) = deref(doc, a).as_dict().ok().and_then(|a| a.get(b"AP").ok()).and_then(|o| deref(doc, o).as_dict().ok()) else { continue };
            for (_, v) in ap.iter() {
                match deref(doc, v) {
                    Object::Stream(_) => g.stream(doc, v, true, 0),
                    Object::Dictionary(states) => states.iter().for_each(|(_, s)| g.stream(doc, s, true, 0)),
                    _ => {}
                }
            }
        }
    }
    g.out
}

struct Gather {
    out: HashSet<ObjectId>,
    seen: HashSet<(ObjectId, bool)>,
    nodes: usize,
}

impl Gather {
    /// A form, pattern, group or glyph stream: its own resources, `outside` the page content
    /// when it is reached from somewhere `visit_page` does not walk.
    fn stream(&mut self, doc: &Document, obj: &Object, outside: bool, depth: u32) {
        let Ok(id) = obj.as_reference() else { return };
        self.nodes += 1;
        if depth > 16 || self.nodes > 1_000_000 || !self.seen.insert((id, outside)) {
            return;
        }
        let res = match doc.get_object(id) {
            Ok(Object::Stream(s)) => s.dict.get(b"Resources").ok(),
            Ok(Object::Dictionary(d)) => d.get(b"Resources").ok(),
            _ => None,
        };
        if let Some(res) = res.and_then(|r| deref(doc, r).as_dict().ok()) {
            let res = res.clone();
            self.resources(doc, &res, outside, depth + 1);
        }
    }

    fn resources(&mut self, doc: &Document, res: &Dictionary, outside: bool, depth: u32) {
        let entries = |kind: &[u8]| -> Vec<Object> {
            res.get(kind).ok().and_then(|o| deref(doc, o).as_dict().ok()).map(|d| d.iter().map(|(_, v)| v.clone()).collect()).unwrap_or_default()
        };
        for v in entries(b"XObject") {
            let Ok(Object::Stream(s)) = v.as_reference().and_then(|id| doc.get_object(id)) else { continue };
            match s.dict.get(b"Subtype").and_then(|o| o.as_name()) {
                Ok(b"Image") if outside => {
                    self.out.insert(v.as_reference().unwrap());
                }
                Ok(b"Form") => self.stream(doc, &v, outside, depth),
                _ => {}
            }
        }
        for v in entries(b"Pattern") {
            self.stream(doc, &v, true, depth);
        }
        for v in entries(b"ExtGState") {
            let group = deref(doc, &v).as_dict().ok().and_then(|g| g.get(b"SMask").ok()).and_then(|m| deref(doc, m).as_dict().ok()).and_then(|m| m.get(b"G").ok()).cloned();
            if let Some(g) = group {
                self.stream(doc, &g, true, depth);
            }
        }
        for v in entries(b"Font") {
            let type3 = matches!(deref(doc, &v).as_dict().ok().and_then(|f| f.get(b"Subtype").ok()).and_then(|o| o.as_name().ok()), Some(b"Type3"));
            if type3 {
                self.stream(doc, &v, true, depth);
            }
        }
    }
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
