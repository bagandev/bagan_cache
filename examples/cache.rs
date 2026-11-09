use bagan_cache::BaganCache;
use std::io::{self, Write};

fn main() -> io::Result<()> {
    let entry_dir = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let mut cache = BaganCache::new(&entry_dir)?;
    cache.start_watching()?;

    println!("Watching {entry_dir}. Press Enter to stop.");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;

    cache.stop_watching();
    Ok(())
}
