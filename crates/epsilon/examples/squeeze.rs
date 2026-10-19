//! Developer tool: cargo run -p epsilon --release --example squeeze -- in.pdf out.pdf [preset]

use std::path::Path;

use epsilon::{compress_file, Preset};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: squeeze <in.pdf> <out.pdf> [lossless|print|balanced|small]");
        std::process::exit(2);
    }
    let preset = match args.get(3).map(String::as_str).unwrap_or("balanced") {
        "lossless" => Preset::Lossless,
        "print" => Preset::Print,
        "balanced" => Preset::Balanced,
        "small" => Preset::Small,
        other => {
            eprintln!("unknown preset {other}");
            std::process::exit(2);
        }
    };
    let started = std::time::Instant::now();
    match compress_file(Path::new(&args[1]), Path::new(&args[2]), &preset.options()) {
        Ok(r) => {
            let pct = 100.0 * (1.0 - r.output_bytes as f64 / r.input_bytes as f64);
            println!("{:?}", r);
            println!("{} -> {} bytes ({pct:.1}% smaller) in {:.2?}", r.input_bytes, r.output_bytes, started.elapsed());
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
