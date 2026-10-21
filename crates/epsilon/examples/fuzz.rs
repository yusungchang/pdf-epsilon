//! A mutation fuzzer for the engine that runs on stable Rust (cargo-fuzz needs nightly).
//!
//! It takes seed PDFs, damages copies of them at random (flipped bytes, huge or negative
//! numbers, PDF tokens in odd places, deleted, repeated and spliced ranges) and runs the whole
//! analysis, which compresses with every preset in memory, on each one. A finding is any input
//! that panics, takes longer than the time limit, overflows the stack or exhausts memory.
//!
//! The work runs on a thread with a 1 MiB stack, as in tests/hostile.rs, under a 3 GB memory cap.
//! Each input is written to `<out>/current.pdf` before it runs, so an input that kills the
//! process (stack overflow, memory cap) is still on disk afterwards. Panics are saved as
//! `<out>/panic-<n>.pdf` and the run goes on; a hang is saved as `<out>/hang.pdf` and ends it.
//!
//! Usage: cargo run --release -p epsilon --example fuzz -- <out dir> <iterations> <rng seed> seed.pdf...
//! Seeds work best uncompressed (`mutool clean -d in.pdf out.pdf`), so mutations reach the objects.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const MEMORY: usize = 3 << 30;
const TIME_LIMIT: Duration = Duration::from_secs(30);

struct Capped;
static USED: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Capped {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if USED.fetch_add(layout.size(), Ordering::Relaxed) + layout.size() > MEMORY {
            USED.fetch_sub(layout.size(), Ordering::Relaxed);
            return std::ptr::null_mut();
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        USED.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOC: Capped = Capped;

/// xorshift64*: deterministic for a given seed, so a run can be repeated.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

const TOKENS: &[&[u8]] = &[
    b"0", b"-1", b"4294967295", b"99999999999999999999", b"-2147483648", b"1e308", b"0.0000001",
    b"<<", b">>", b"[", b"]", b"(", b")", b"<", b">", b"/", b" R ", b"0 0 R", b"1 0 R", b"null",
    b"obj", b"endobj", b"stream\n", b"\nendstream", b"xref", b"trailer", b"startxref", b"%%EOF",
    b"/Length", b"/Filter /FlateDecode", b"/Predictor 15", b"/Columns 99999", b"/Type /ObjStm", b"/N 999999",
    b"/Kids [", b"/Parent", b"/Contents", b"/Resources", b"/XObject", b"/Font", b"/Subtype /Image",
    b"/Width 65535", b"/Height 65535", b"/BitsPerComponent 16", b"/ColorSpace /DeviceCMYK", b"/DCTDecode",
    b"/Prev", b"/XRefStm", b"/Encrypt", b"/Root", b"/Size 0", b"\\", b"\0", b"\xff\xd8\xff",
];

fn mutate(rng: &mut Rng, data: &mut Vec<u8>, seeds: &[Vec<u8>]) {
    for _ in 0..1 + rng.below(6) {
        let len = data.len().max(1);
        let at = rng.below(len).min(data.len());
        match rng.below(8) {
            0 => if let Some(b) = data.get_mut(at) { *b ^= 1 << rng.below(8) },
            1 => if let Some(b) = data.get_mut(at) { *b = rng.next() as u8 },
            2 => {
                let t = TOKENS[rng.below(TOKENS.len())];
                data.splice(at..at, t.iter().copied());
            }
            3 => {
                let end = (at + 1 + rng.below(256)).min(data.len());
                data.drain(at..end);
            }
            4 => {
                let end = (at + 1 + rng.below(512)).min(data.len());
                let copy = data[at..end].to_vec();
                let to = rng.below(data.len() + 1);
                data.splice(to..to, copy);
            }
            5 => {
                // Replace a run of digits with a hostile number.
                if let Some(start) = data[at..].iter().position(|b| b.is_ascii_digit()).map(|p| at + p) {
                    let end = start + data[start..].iter().take_while(|b| b.is_ascii_digit()).count();
                    let t = TOKENS[rng.below(6)];
                    data.splice(start..end, t.iter().copied());
                }
            }
            6 => {
                // Splice in a range from another seed.
                let other = &seeds[rng.below(seeds.len())];
                if !other.is_empty() {
                    let from = rng.below(other.len());
                    let end = (from + 1 + rng.below(1024)).min(other.len());
                    data.splice(at..at, other[from..end].iter().copied());
                }
            }
            _ => data.truncate(at.max(16)),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: fuzz <out dir> <iterations> <rng seed> seed.pdf...");
        std::process::exit(2);
    }
    let out = std::path::PathBuf::from(&args[1]);
    let iterations: usize = args[2].parse().unwrap();
    let mut rng = Rng(args[3].parse::<u64>().unwrap() | 1);
    let seeds: Vec<Vec<u8>> = args[4..].iter().filter_map(|p| std::fs::read(p).ok()).collect();
    std::fs::create_dir_all(&out).unwrap();
    std::panic::set_hook(Box::new(|_| {}));

    let (started, mut panics, mut ok, mut refused) = (Instant::now(), 0, 0, 0);
    for i in 0..iterations {
        let mut data = seeds[rng.below(seeds.len())].clone();
        mutate(&mut rng, &mut data, &seeds);
        std::fs::write(out.join("current.pdf"), &data).unwrap();
        let (tx, rx) = mpsc::channel();
        let input = data.clone();
        std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(move || {
                let result = std::panic::catch_unwind(|| epsilon::analyze_bytes(&input).is_ok());
                let _ = tx.send(result.map_err(|e| {
                    e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
                }));
            })
            .unwrap();
        match rx.recv_timeout(TIME_LIMIT) {
            Ok(Ok(true)) => ok += 1,
            Ok(Ok(false)) => refused += 1,
            Ok(Err(message)) => {
                panics += 1;
                std::fs::write(out.join(format!("panic-{i}.pdf")), &data).unwrap();
                std::fs::write(out.join(format!("panic-{i}.txt")), message).unwrap();
            }
            Err(_) => {
                std::fs::write(out.join("hang.pdf"), &data).unwrap();
                println!("HANG at iteration {i}, saved hang.pdf");
                std::process::exit(1);
            }
        }
    }
    let _ = std::fs::remove_file(out.join("current.pdf"));
    println!("{iterations} inputs in {:.0?}: {ok} analysed, {refused} refused, {panics} panics", started.elapsed());
}
