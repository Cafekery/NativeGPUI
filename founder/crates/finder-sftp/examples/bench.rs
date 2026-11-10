//! `bench sftp://user@host/path/to/file [local_out]`: downloads the file and prints the rate.

use std::path::PathBuf;
use std::time::Instant;

use finder_sftp::{Address, Server};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(address) = args.next().as_deref().and_then(Address::parse) else {
        eprintln!("usage: bench sftp://user@host/path/to/file [local_out]");
        std::process::exit(2);
    };
    let local = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("finder-sftp-bench"));
    let server = Server::connect(&address, None).unwrap_or_else(|error| {
        eprintln!("connect: {error}");
        std::process::exit(1);
    });
    let started = Instant::now();
    let mut bytes = 0;
    let downloaded = server.download(&address.path, &local, &mut |done, _| {
        bytes = done;
        true
    });
    let seconds = started.elapsed().as_secs_f64();
    if let Err(error) = downloaded {
        eprintln!("download: {error}");
        std::process::exit(1);
    }
    println!(
        "{bytes} bytes in {seconds:.3} s: {:.1} MB/s",
        bytes as f64 / seconds / 1e6
    );
}
