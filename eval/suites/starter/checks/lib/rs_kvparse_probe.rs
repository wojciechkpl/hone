// Harness binary for c07 (copied into the work directory as src/bin/probe.rs). Reads one
// hex-encoded line per case, prints `N` or `S<TAB>hex(key)<TAB>hex(value)`. No expectations.
use std::io::BufRead;

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> String {
    let bytes: Vec<u8> = (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap_or(b'?'))
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn main() {
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let input = unhex(line.unwrap_or_default().trim());
        match kvparse::parse_kv(&input) {
            None => println!("N"),
            Some((k, v)) => println!("S\t{}\t{}", hex(&k), hex(&v)),
        }
    }
}
