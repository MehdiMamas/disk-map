//! Dev tool: `cargo run --release --bin scan-cli -- <path>` prints the top folders and scan time.
use disk_map_lib::scanner::*;
use std::path::Path;

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let root = Path::new(&arg);
    let t = std::time::Instant::now();
    let out = scan_dir(root, &Options::default(), |_| {});
    let el = t.elapsed();
    let v = prune(&out.tree, root, PRUNE_FRACTION, 1);
    println!("total {:.2} GB, {} files, {} errors ({} permission), {} other-fs skipped, {:.2?}",
        v.size as f64 / 1e9, v.files, out.errors, out.permission_denied, out.other_fs_skipped, el);
    for c in v.children.iter().take(5) {
        println!("{:>8.2} GB  {}", c.size as f64 / 1e9, c.name);
    }
}
