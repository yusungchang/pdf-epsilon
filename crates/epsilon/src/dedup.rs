//! Merge byte-identical streams: one copy is kept and every reference is pointed at it.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use lopdf::{Document, Object, ObjectId};

/// Returns the ids of the copies that were removed.
pub fn merge_identical_streams(doc: &mut Document) -> Vec<ObjectId> {
    // Bucket by a hash, then confirm with a full comparison.
    let mut buckets: HashMap<u64, Vec<ObjectId>> = HashMap::new();
    for (id, obj) in &doc.objects {
        if let Object::Stream(s) = obj {
            let mut h = DefaultHasher::new();
            format!("{:?}", s.dict).hash(&mut h);
            s.content.hash(&mut h);
            buckets.entry(h.finish()).or_default().push(*id);
        }
    }

    let mut replace: HashMap<ObjectId, ObjectId> = HashMap::new();
    for ids in buckets.values().filter(|v| v.len() > 1) {
        let mut keepers: Vec<ObjectId> = Vec::new();
        for id in ids {
            let same = keepers.iter().copied().find(|k| same_stream(doc, *k, *id));
            match same {
                Some(k) => {
                    replace.insert(*id, k);
                }
                None => keepers.push(*id),
            }
        }
    }
    if replace.is_empty() {
        return Vec::new();
    }

    for obj in doc.objects.values_mut() {
        redirect(obj, &replace);
    }
    for (_, obj) in doc.trailer.iter_mut() {
        redirect(obj, &replace);
    }
    for id in replace.keys() {
        doc.objects.remove(id);
    }
    replace.into_keys().collect()
}

fn same_stream(doc: &Document, a: ObjectId, b: ObjectId) -> bool {
    match (doc.objects.get(&a), doc.objects.get(&b)) {
        (Some(Object::Stream(x)), Some(Object::Stream(y))) => {
            x.content == y.content && format!("{:?}", x.dict) == format!("{:?}", y.dict)
        }
        _ => false,
    }
}

fn redirect(obj: &mut Object, map: &HashMap<ObjectId, ObjectId>) {
    match obj {
        Object::Reference(id) => {
            if let Some(to) = map.get(id) {
                *id = *to;
            }
        }
        Object::Array(items) => items.iter_mut().for_each(|o| redirect(o, map)),
        Object::Dictionary(d) => d.iter_mut().for_each(|(_, o)| redirect(o, map)),
        Object::Stream(s) => s.dict.iter_mut().for_each(|(_, o)| redirect(o, map)),
        _ => {}
    }
}
