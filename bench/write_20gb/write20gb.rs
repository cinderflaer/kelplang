// Writes 20 GiB to a fresh file in 1 MiB chunks, syncs it to storage, verifies
// the size, then deletes it.
//
// The last four lines are the machine-readable contract shared by all four
// implementations: BYTES, WORK_NS, FLUSH_NS, DELETE_NS.
use std::fs::{remove_file, File};
use std::io::Write;
use std::time::Instant;

const MIB: usize = 1 << 20;
const TOTAL: u64 = 20 * 1024 * 1024 * 1024;

fn main() {
    let path = std::env::args().nth(1).expect("usage: write20gb <path>");
    let _ = remove_file(&path);

    let chunk = vec![b'x'; MIB];
    let mut file = File::create(&path).expect("create");
    let work_start = Instant::now();
    let mut written: u64 = 0;
    while written < TOTAL {
        file.write_all(&chunk).expect("write");
        written += MIB as u64;
    }
    let work_ns = work_start.elapsed().as_nanos() as u64;

    let flush_start = Instant::now();
    file.sync_all().expect("sync");
    let flush_ns = flush_start.elapsed().as_nanos() as u64;
    drop(file);

    let size = std::fs::metadata(&path).expect("stat").len();
    let delete_start = Instant::now();
    remove_file(&path).expect("remove");
    let delete_ns = delete_start.elapsed().as_nanos() as u64;

    println!("BYTES {size}");
    println!("WORK_NS {work_ns}");
    println!("FLUSH_NS {flush_ns}");
    println!("DELETE_NS {delete_ns}");
}
