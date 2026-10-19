//! Remove optional data that does not change how a page looks or works.

use lopdf::{Document, Object};

use crate::Options;

pub fn strip(doc: &mut Document, options: &Options) {
    let page_ids: Vec<_> = doc.get_pages().into_values().collect();

    if options.strip_thumbnails {
        for id in &page_ids {
            if let Ok(page) = doc.get_dictionary_mut(*id) {
                page.remove(b"Thumb");
            }
        }
    }

    if options.strip_private_data || options.strip_metadata {
        for obj in doc.objects.values_mut() {
            let dict = match obj {
                Object::Dictionary(d) => d,
                Object::Stream(s) => &mut s.dict,
                _ => continue,
            };
            if options.strip_private_data {
                dict.remove(b"PieceInfo");
            }
            // XMP lives under /Metadata on the catalog, pages, images and fonts.
            // Never on an XMP stream itself, so this does not remove a stream's own type.
            if options.strip_metadata {
                dict.remove(b"Metadata");
            }
        }
    }
    // Unreferenced thumbnails, private data and XMP streams are dropped by pruning.
}
