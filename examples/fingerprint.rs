//! Prints the tfp1 fingerprint of each file named, and how long it took:
//!
//! ```text
//! cargo run --release --example fingerprint -- <file>...
//! ```
//!
//! The cache is not used: every file is read. Used to measure the fingerprint's cost on a disk.

fn main() {
    for path in std::env::args_os().skip(1) {
        let start = std::time::Instant::now();
        match tenstag::fingerprint_file(std::path::Path::new(&path)) {
            Ok(id) => println!("{id}  {:.3}s  {}", start.elapsed().as_secs_f64(), path.to_string_lossy()),
            Err(e) => eprintln!("{e}"),
        }
    }
}
