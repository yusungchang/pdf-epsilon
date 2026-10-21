//! Objects deleted by a later revision of the file.
//!
//! A PDF edited by appending changes ("incremental updates") can delete an object by listing it
//! as free in a newer cross-reference table. lopdf 0.45 ignores free entries, so it keeps the
//! older version of the object: a copy saved from it can bring back content the author deleted.
//! This reads the classic cross-reference tables itself, newest first along the `/Prev` chain,
//! and returns the object numbers whose newest entry is free.
//!
//! Every value comes from the untrusted file, so all reads are bounds-checked and the walk is
//! capped. A section that is not a classic table (a cross-reference stream) ends the walk: what
//! was read up to there still counts. So does a hybrid section (a trailer with `/XRefStm`): its
//! stream, unread here, can define any object, and the free entries in it and in older tables
//! are placeholders for those objects (Word writes thousands of them).

use std::collections::{BTreeSet, HashSet};

/// At most this many revisions are followed.
const MAX_REVISIONS: usize = 1000;
/// At most this many entries are read in all.
const MAX_ENTRIES: usize = 4_000_000;

pub fn deleted_objects(bytes: &[u8]) -> BTreeSet<u32> {
    let mut decided: HashSet<u32> = HashSet::new();
    let mut deleted = BTreeSet::new();
    let mut seen = HashSet::new();
    let mut budget = MAX_ENTRIES;
    let mut next = last_startxref(bytes);
    while let Some(offset) = next {
        if seen.len() >= MAX_REVISIONS || !seen.insert(offset) {
            break;
        }
        let Some(section) = read_section(bytes, offset, &mut budget) else { break };
        if section.hybrid {
            break;
        }
        for (num, free) in section.entries {
            // The newest revision that mentions an object decides it.
            if decided.insert(num) && free && num != 0 {
                deleted.insert(num);
            }
        }
        next = section.prev;
    }
    deleted
}

struct Section {
    /// Object number and whether its entry is free.
    entries: Vec<(u32, bool)>,
    prev: Option<usize>,
    hybrid: bool,
}

/// The offset after the last `startxref` keyword.
fn last_startxref(bytes: &[u8]) -> Option<usize> {
    const KEY: &[u8] = b"startxref";
    let at = bytes.windows(KEY.len()).rposition(|w| w == KEY)?;
    let mut p = at + KEY.len();
    skip_space(bytes, &mut p);
    number(bytes, &mut p).and_then(|n| usize::try_from(n).ok())
}

fn read_section(bytes: &[u8], offset: usize, budget: &mut usize) -> Option<Section> {
    let mut p = offset;
    skip_space(bytes, &mut p);
    if !bytes.get(p..)?.starts_with(b"xref") {
        return None;
    }
    p += 4;
    let mut entries = Vec::new();
    loop {
        skip_space(bytes, &mut p);
        if bytes.get(p..)?.starts_with(b"trailer") {
            p += 7;
            break;
        }
        let start = number(bytes, &mut p)?;
        skip_space(bytes, &mut p);
        let count = number(bytes, &mut p)?;
        for i in 0..count {
            if *budget == 0 {
                return None;
            }
            *budget -= 1;
            skip_space(bytes, &mut p);
            number(bytes, &mut p)?; // offset
            skip_space(bytes, &mut p);
            number(bytes, &mut p)?; // generation
            skip_space(bytes, &mut p);
            let kind = *bytes.get(p)?;
            p += 1;
            let num = u32::try_from(start.checked_add(i)?).ok()?;
            match kind {
                b'f' => entries.push((num, true)),
                b'n' => entries.push((num, false)),
                _ => return None,
            }
        }
    }
    // The trailer dictionary: only its /Prev and /XRefStm keys matter.
    let end = bytes.get(p..)?.windows(9).position(|w| w == b"startxref").map_or(bytes.len(), |e| p + e);
    let trailer = bytes.get(p..end)?;
    let prev = key_number(trailer, b"/Prev").and_then(|n| usize::try_from(n).ok());
    let hybrid = key_number(trailer, b"/XRefStm").is_some();
    Some(Section { entries, prev, hybrid })
}

fn key_number(dict: &[u8], key: &[u8]) -> Option<u64> {
    let at = dict.windows(key.len() + 1).position(|w| w.starts_with(key) && !w[key.len()].is_ascii_alphanumeric())?;
    let mut p = at + key.len();
    skip_space(dict, &mut p);
    number(dict, &mut p)
}

fn skip_space(bytes: &[u8], p: &mut usize) {
    while let Some(c) = bytes.get(*p) {
        match c {
            b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0' => *p += 1,
            b'%' => {
                while let Some(c) = bytes.get(*p) {
                    if *c == b'\r' || *c == b'\n' {
                        break;
                    }
                    *p += 1;
                }
            }
            _ => break,
        }
    }
}

/// A non-negative integer of at most 18 digits.
fn number(bytes: &[u8], p: &mut usize) -> Option<u64> {
    let start = *p;
    let mut n: u64 = 0;
    while let Some(c) = bytes.get(*p).filter(|c| c.is_ascii_digit()) {
        if *p - start >= 18 {
            return None;
        }
        n = n * 10 + u64::from(c - b'0');
        *p += 1;
    }
    (*p > start).then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "%PDF-1.3\n1 0 obj\n<< >>\nendobj\nxref\n0 3\n0000000000 65535 f \n0000000009 00000 n \n0000000009 00000 n \ntrailer << /Size 3 >>\nstartxref\n";

    fn file(update: &str, trailer_extra: &str) -> Vec<u8> {
        let first = BASE.find("xref").unwrap();
        let mut s = format!("{BASE}{first}\n%%EOF\n");
        let second = s.len();
        s.push_str(&format!("xref\n{update}trailer << /Size 3 /Prev {first} {trailer_extra} >>\nstartxref\n{second}\n%%EOF\n"));
        s.into_bytes()
    }

    #[test]
    fn a_newer_free_entry_deletes() {
        assert_eq!(deleted_objects(&file("2 1\n0000000000 00001 f \n", "")), BTreeSet::from([2]));
    }

    #[test]
    fn a_newer_in_use_entry_wins_over_an_older_free_one() {
        assert!(deleted_objects(&file("1 1\n0000000009 00000 n \n", "")).is_empty());
    }

    #[test]
    fn hybrid_free_entries_are_placeholders() {
        assert!(deleted_objects(&file("2 1\n0000000000 00001 f \n", "/XRefStm 9")).is_empty());
        // Word: an empty hybrid section on top of a table full of placeholders.
        let mut s = String::from_utf8(file("2 1\n0000000000 65535 f \n", "")).unwrap();
        let older = s.rfind("startxref\n").unwrap();
        let older = s[older + 10..].lines().next().unwrap().to_string();
        let at = s.len();
        s.push_str(&format!("xref\n0 0\ntrailer << /Size 3 /Prev {older} /XRefStm 9 >>\nstartxref\n{at}\n%%EOF\n"));
        assert!(deleted_objects(s.as_bytes()).is_empty());
    }

    #[test]
    fn hostile_input_ends_quietly() {
        assert!(deleted_objects(b"startxref\n0\n").is_empty());
        assert!(deleted_objects(b"xref\n0 999999999999\ntrailer startxref\n0").is_empty());
        let mut cycle = file("2 1\n0000000000 00001 f \n", "").to_vec();
        cycle.extend_from_slice(b"\nstartxref\n999999999999999999\n");
        assert!(deleted_objects(&cycle).is_empty());
    }
}
