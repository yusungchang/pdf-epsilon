//! Developer tool: cargo run -p epsilon --release --example inspect -- in.pdf [--pages] [--json]
//!
//! Prints what the analyzer found: where the bytes go, what each preset saves,
//! findings, and with `--pages` the issues on every page. `--json` prints the raw analysis.
//! Finding and issue texts come from the interface's English file, ui/locales/en.json.

use std::path::Path;

use epsilon::{analyze_file, Arg, Message, Preset, Status};
use serde_json::Value;

/// Renders engine messages with English plural rules: "one" for a count of 1, else "other".
struct Text(serde_json::Map<String, Value>);

impl Text {
    fn load() -> Self {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui/locales/en.json");
        let json = std::fs::read_to_string(path).expect("ui/locales/en.json");
        match serde_json::from_str(&json).expect("en.json is valid JSON") {
            Value::Object(map) => Text(map),
            _ => panic!("en.json must be an object"),
        }
    }

    fn key(&self, key: &str, count: Option<f64>) -> String {
        match self.0.get(key) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Object(forms)) => {
                let form = if count == Some(1.0) { "one" } else { "other" };
                forms.get(form).or_else(|| forms.get("other")).and_then(Value::as_str).unwrap_or(key).to_string()
            }
            _ => key.rsplit('.').next().unwrap_or(key).to_string(),
        }
    }

    fn say(&self, m: &Message) -> String {
        let count = match m.args.get("count") {
            Some(Arg::Number(n)) => Some(*n),
            _ => None,
        };
        let mut out = self.key(m.key, count);
        for (name, arg) in &m.args {
            let value = match arg {
                Arg::Number(n) => format!("{n}"),
                Arg::Text(t) => t.clone(),
                Arg::List(items) => items.join(", "),
                Arg::Bytes { bytes } => size(*bytes),
                Arg::Term { term } => self.key(term, None),
            };
            out = out.replace(&format!("{{{name}}}"), &value);
        }
        out
    }
}

fn size(n: u64) -> String {
    match n {
        0..=1023 => format!("{n} B"),
        1024..=1_048_575 => format!("{:.1} KB", n as f64 / 1024.0),
        _ => format!("{:.2} MB", n as f64 / 1_048_576.0),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("usage: inspect <in.pdf> [--pages] [--json]");
        std::process::exit(2);
    };
    let a = match analyze_file(Path::new(path)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    if args.iter().any(|a| a == "--json") {
        println!("{}", serde_json::to_string_pretty(&a).unwrap());
        return;
    }
    let text = Text::load();
    let f = &a.file;
    println!("{path}");
    println!(
        "{} · {} page{} · PDF {} · {} objects · {} revision{} · analysed in {} ms",
        size(f.bytes),
        f.pages,
        if f.pages == 1 { "" } else { "s" },
        f.version,
        f.objects,
        f.revisions,
        if f.revisions == 1 { "" } else { "s" },
        a.millis
    );
    if let Some(p) = &f.producer {
        println!("Producer: {p}");
    }

    println!("\nWhere the bytes go");
    for c in &a.categories {
        println!("  {:<16} {:>10}  {:>5.1}%  {} objects", format!("{:?}", c.category), size(c.bytes), 100.0 * c.bytes as f64 / f.bytes as f64, c.objects);
    }

    println!("\nMeasured by running each preset");
    for p in &a.presets {
        let saved = f.bytes - p.output_bytes;
        print!("  {:<9} {:>10}  -{:>4.1}%", format!("{:?}", p.preset), size(p.output_bytes), 100.0 * saved as f64 / f.bytes as f64);
        if let Some(e) = &p.error {
            print!("  failed: {e}");
        }
        let parts: Vec<String> = p
            .savings
            .iter()
            .map(|s| format!("{:?} {}{}", s.kind, if s.bytes < 0 { "-" } else { "" }, size(s.bytes.unsigned_abs())))
            .collect();
        println!("  {}", parts.join(", "));
    }

    println!("\nFindings");
    for x in &a.findings {
        let best = Preset::ALL.iter().zip(&x.savings).filter(|(_, s)| **s > 0).last();
        let status = match x.status {
            Status::Fixed => best.map(|(p, s)| format!("{p:?} saves {}", size(*s))).unwrap_or_else(|| "fixed".into()),
            Status::NotYet => "not handled yet".into(),
            Status::Kept => "kept".into(),
            Status::Warning => "warning".into(),
        };
        println!("  [{:?}] {} — {} ({status})", x.severity, text.say(&x.title), size(x.bytes));
        let detail: Vec<String> = x.detail.iter().map(|m| text.say(m)).collect();
        println!("      {}", detail.join(" "));
        if !x.pages.is_empty() {
            let shown: Vec<String> = x.pages.iter().take(12).map(u32::to_string).collect();
            println!("      pages {}{}", shown.join(", "), if x.pages.len() > 12 { ", …" } else { "" });
        }
    }

    if args.iter().any(|a| a == "--pages") {
        println!("\nPages (share of file · saving with Balanced)");
        for p in &a.pages {
            println!(
                "  {:>4}  {:>10}  content {} · images {} · fonts {} · graphics {} · other {}  → -{}",
                p.number,
                size(p.bytes),
                size(p.content),
                size(p.images),
                size(p.fonts),
                size(p.graphics),
                size(p.other),
                size(p.savings[2])
            );
            for i in &p.issues {
                println!("          · {}", text.say(&i.message));
            }
        }
    }
}
