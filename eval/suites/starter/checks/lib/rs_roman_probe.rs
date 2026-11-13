// Harness binary for c08 (copied into the work directory as src/bin/probe.rs). Reads one decimal
// number per line, prints `N` or `S<TAB>hex(numeral)`. No expectations.
use std::io::BufRead;

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn main() {
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let n: u32 = line.unwrap_or_default().trim().parse().unwrap_or(0);
        match numerals::to_roman(n) {
            None => println!("N"),
            Some(s) => println!("S\t{}", hex(&s)),
        }
    }
}
