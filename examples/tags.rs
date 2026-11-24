//! Prints a model file's identity and every owner's tags, after setting any tags given, as `owner`:
//!
//! ```text
//! cargo run --example tags -- <owner> <version> <file> [tag]...
//! ```
//!
//! The cache folder is the environment's (TENSTAG_CACHE_DIR). Used to check that implementations in other languages
//! read and write the same files.

fn main() -> Result<(), tenstag::Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [owner, version, file, tags @ ..] = args.as_slice() else {
        eprintln!("usage: tags <owner> <version> <file> [tag]...");
        std::process::exit(2);
    };
    let mut library = tenstag::Library::new(tenstag::Locations::bare());
    library.set_owner(owner, version.parse().unwrap_or(0))?;
    let entry = library.resolve(file)?;
    for tag in tags {
        library.tag(&entry, tag)?;
    }
    println!("{}", library.identity(&entry)?);
    for (owner, tags) in library.tags(&entry)?.owners() {
        let list: Vec<String> = tags.iter().map(|(k, v)| v.map_or(k.to_string(), |v| format!("{k}={v}"))).collect();
        println!("{owner} v{}: {}", tags.version(), list.join(" "));
    }
    Ok(())
}
