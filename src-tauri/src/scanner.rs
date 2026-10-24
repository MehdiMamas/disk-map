//! Parallel directory scanner. Builds a full in-memory tree of directories
//! (files below `keep_file_min` bytes are folded into one "(small files)"
//! entry per directory), then `prune` produces the view the UI shows.
use crate::hogs::{match_hog, Hog};
use jwalk::WalkDirGeneric;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const SMALL_FILES: &str = "(small files)";
pub const OTHER: &str = "(other)";
pub const PRUNE_FRACTION: f64 = 0.005;

#[derive(Debug, Clone, Default)]
pub struct Node {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub files: u64,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ViewNode {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
    pub files: u64,
    pub other: bool,
    pub hog: Option<Hog>,
    pub children: Vec<ViewNode>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Progress {
    pub files: u64,
    pub bytes: u64,
}

#[derive(Debug, Default)]
pub struct ScanOutput {
    pub tree: Node,
    pub errors: u64,
    pub permission_denied: u64,
    pub other_fs_skipped: u64,
    pub denied_examples: Vec<String>,
}

pub struct Options {
    pub keep_file_min: u64,
}

impl Default for Options {
    fn default() -> Self {
        Options { keep_file_min: 1 << 20 }
    }
}

#[derive(Debug, Clone, Default)]
struct Meta {
    size: u64,
    dev: u64,
    ino: u64,
    nlink: u64,
    ok: bool,
}

#[cfg(unix)]
fn meta_of(m: &std::fs::Metadata) -> Meta {
    use std::os::unix::fs::MetadataExt;
    Meta { size: m.blocks() * 512, dev: m.dev(), ino: m.ino(), nlink: m.nlink(), ok: true }
}
#[cfg(not(unix))]
fn meta_of(m: &std::fs::Metadata) -> Meta {
    Meta { size: m.len(), dev: 0, ino: 0, nlink: 1, ok: true }
}

struct Dir {
    name: String,
    parent: usize,
    own: u64,
    files: u64,
    big: Vec<Node>,
    small_size: u64,
    small_count: u64,
}

/// On macOS the Data volume is mounted at /System/Volumes/Data and its folders
/// (/Users, /Applications, /private, ...) are firmlinked into /, on the same
/// device and inode. Scanning above it would count every file twice, so the
/// mount itself is skipped and its content is counted at the firmlinked paths.
fn firmlink_targets(root: &Path) -> Vec<PathBuf> {
    let data = Path::new("/System/Volumes/Data");
    if cfg!(target_os = "macos") && data.starts_with(root) && data != root {
        vec![data.to_path_buf()]
    } else {
        Vec::new()
    }
}

pub fn scan_dir(root: &Path, opts: &Options, mut progress: impl FnMut(Progress)) -> ScanOutput {
    let root_dev = std::fs::symlink_metadata(root).map(|m| meta_of(&m).dev).unwrap_or(0);
    let skipped = Arc::new(AtomicU64::new(0));
    let skipped2 = skipped.clone();
    let skip_paths = firmlink_targets(root);
    let walker = WalkDirGeneric::<((), Meta)>::new(root)
        .skip_hidden(false)
        .follow_links(false)
        .process_read_dir(move |_, _, _, children| {
            for c in children.iter_mut().flatten() {
                if let Ok(m) = c.metadata() {
                    c.client_state = meta_of(&m);
                }
            }
            children.retain(|c| match c {
                Ok(e) if e.file_type().is_dir() && cfg!(unix) && e.client_state.ok && e.client_state.dev != root_dev => {
                    skipped2.fetch_add(1, Ordering::Relaxed);
                    false
                }
                Ok(e) if e.file_type().is_dir() && skip_paths.iter().any(|p| e.path() == *p) => false,
                _ => true,
            });
        });

    let mut out = ScanOutput::default();
    let mut dirs: Vec<Dir> = Vec::new();
    let mut index: HashMap<PathBuf, usize> = HashMap::new();
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let (mut files, mut bytes) = (0u64, 0u64);
    let mut last = Instant::now();

    let note_err = |out: &mut ScanOutput, e: &std::io::Error, p: &Path| {
        out.errors += 1;
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            out.permission_denied += 1;
            if out.denied_examples.len() < 5 {
                out.denied_examples.push(p.display().to_string());
            }
        }
    };

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                let p = e.path().map(|p| p.to_path_buf()).unwrap_or_default();
                if let Some(io) = e.io_error() {
                    note_err(&mut out, io, &p);
                } else {
                    out.errors += 1;
                }
                continue;
            }
        };
        let path = entry.path();
        if let Some(err) = &entry.read_children_error {
            if let Some(io) = err.io_error() {
                note_err(&mut out, io, &path);
            } else {
                out.errors += 1;
            }
        }
        let is_dir = entry.file_type().is_dir();
        let m = if entry.depth() == 0 {
            std::fs::symlink_metadata(&path).map(|m| meta_of(&m)).unwrap_or_default()
        } else {
            entry.client_state.clone()
        };
        if !m.ok && entry.depth() > 0 {
            out.errors += 1;
        }
        // Count hard-linked files once.
        let mut size = m.size;
        if !is_dir && m.nlink > 1 && !seen.insert((m.dev, m.ino)) {
            size = 0;
        }
        if is_dir {
            let name = if entry.depth() == 0 {
                root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string())
            } else {
                entry.file_name().to_string_lossy().into_owned()
            };
            let parent = if entry.depth() == 0 { usize::MAX } else { index.get(entry.parent_path()).copied().unwrap_or(0) };
            index.insert(path, dirs.len());
            dirs.push(Dir { name, parent, own: size, files: 0, big: vec![], small_size: 0, small_count: 0 });
        } else if let Some(&p) = index.get(entry.parent_path()) {
            let d = &mut dirs[p];
            d.files += 1;
            if size >= opts.keep_file_min && size > 0 {
                d.big.push(Node {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    size,
                    is_dir: false,
                    files: 1,
                    children: vec![],
                });
            } else {
                d.small_size += size;
                d.small_count += 1;
            }
        }
        files += 1;
        bytes += size;
        if last.elapsed() >= Duration::from_millis(200) {
            last = Instant::now();
            progress(Progress { files, bytes });
        }
    }
    progress(Progress { files, bytes });
    out.other_fs_skipped = skipped.load(Ordering::Relaxed);

    // Fold bottom-up: parents always have a lower index than their children.
    let mut nodes: Vec<Option<Node>> = (0..dirs.len()).map(|_| None).collect();
    for i in (0..dirs.len()).rev() {
        let d = &mut dirs[i];
        let mut children = std::mem::take(&mut d.big);
        if d.small_count > 0 {
            children.push(Node { name: SMALL_FILES.into(), size: d.small_size, is_dir: false, files: d.small_count, children: vec![] });
        }
        let mut node = Node { name: d.name.clone(), size: d.own, is_dir: true, files: d.files, children };
        node.size += node.children.iter().map(|c| c.size).sum::<u64>();
        node.children.retain(|c| c.size > 0 || c.name != SMALL_FILES);
        nodes[i] = Some(node);
    }
    // Second pass attaches directory children (reverse order keeps totals correct).
    for i in (1..dirs.len()).rev() {
        let child = nodes[i].take().unwrap();
        let p = dirs[i].parent;
        let parent = nodes[p].as_mut().unwrap();
        parent.size += child.size;
        parent.files += child.files;
        parent.children.push(child);
    }
    out.tree = nodes.into_iter().next().flatten().unwrap_or_default();
    sort_tree(&mut out.tree);
    out
}

fn sort_tree(n: &mut Node) {
    n.children.sort_by(|a, b| b.size.cmp(&a.size));
    n.children.iter_mut().for_each(sort_tree);
}

/// Turn `node` (located at `path`) into a view `depth` levels deep. Children below
/// `frac` of the parent size are summed into one "(other)" entry.
pub fn prune(node: &Node, path: &Path, frac: f64, depth: usize) -> ViewNode {
    let hog = if node.is_dir { match_hog(&path.to_string_lossy(), dirs::home_dir().as_deref()) } else { None };
    let mut view = ViewNode {
        name: node.name.clone(),
        path: path.to_string_lossy().into_owned(),
        size: node.size,
        is_dir: node.is_dir,
        files: node.files,
        other: false,
        hog,
        children: vec![],
    };
    if depth == 0 {
        return view;
    }
    let limit = (node.size as f64 * frac) as u64;
    let (mut o_size, mut o_files, mut o_n) = (0u64, 0u64, 0u64);
    for c in &node.children {
        if c.size >= limit && c.size > 0 {
            if c.name == SMALL_FILES {
                let mut v = prune(c, &path.join(&c.name), frac, 0);
                v.other = true;
                view.children.push(v);
            } else {
                view.children.push(prune(c, &path.join(&c.name), frac, depth - 1));
            }
        } else {
            o_size += c.size;
            o_files += c.files.max(1);
            o_n += 1;
        }
    }
    if o_n > 0 {
        view.children.push(ViewNode {
            name: OTHER.into(),
            path: path.to_string_lossy().into_owned(),
            size: o_size,
            is_dir: false,
            files: o_files,
            other: true,
            hog: None,
            children: vec![],
        });
    }
    view
}

/// Find the node at `target` below `root_path`.
pub fn find<'a>(root: &'a Node, root_path: &Path, target: &Path) -> Option<&'a Node> {
    let rel = target.strip_prefix(root_path).ok()?;
    let mut cur = root;
    for comp in rel.components() {
        let name = comp.as_os_str().to_string_lossy();
        cur = cur.children.iter().find(|c| c.name == name)?;
    }
    Some(cur)
}

/// Remove `target` from the tree and subtract its size from all ancestors.
pub fn remove(root: &mut Node, root_path: &Path, target: &Path) -> bool {
    let Ok(rel) = target.strip_prefix(root_path) else { return false };
    let names: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    if names.is_empty() {
        return false;
    }
    fn go(n: &mut Node, names: &[String]) -> Option<(u64, u64)> {
        let idx = n.children.iter().position(|c| c.name == names[0])?;
        let removed = if names.len() == 1 {
            let c = n.children.remove(idx);
            (c.size, c.files)
        } else {
            go(&mut n.children[idx], &names[1..])?
        };
        n.size = n.size.saturating_sub(removed.0);
        n.files = n.files.saturating_sub(removed.1);
        Some(removed)
    }
    go(root, &names).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmlink_target_skipped_only_above_it() {
        let data = PathBuf::from("/System/Volumes/Data");
        if cfg!(target_os = "macos") {
            assert_eq!(firmlink_targets(Path::new("/")), vec![data.clone()]);
            assert_eq!(firmlink_targets(Path::new("/System")), vec![data.clone()]);
        }
        assert!(firmlink_targets(&data).is_empty());
        assert!(firmlink_targets(Path::new("/Users")).is_empty());
    }
    use std::fs;

    fn write(p: &Path, n: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![1u8; n]).unwrap();
    }
    fn all() -> Options {
        Options { keep_file_min: 0 }
    }
    fn child<'a>(n: &'a Node, name: &str) -> &'a Node {
        n.children.iter().find(|c| c.name == name).unwrap()
    }

    #[test]
    fn sizes_add_up() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("a/x.bin"), 100_000);
        write(&t.path().join("a/b/y.bin"), 50_000);
        write(&t.path().join("z.bin"), 10_000);
        let out = scan_dir(t.path(), &all(), |_| {});
        assert_eq!(out.errors, 0);
        let a = child(&out.tree, "a");
        assert!(a.size >= 150_000, "a={}", a.size);
        assert!(child(a, "b").size >= 50_000);
        assert!(child(a, "b").size < a.size);
        assert!(out.tree.size >= a.size + 10_000);
        assert_eq!(out.tree.files, 3 + 0); // 3 files
        let sum: u64 = out.tree.children.iter().map(|c| c.size).sum();
        assert!(out.tree.size >= sum);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_not_followed() {
        let t = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(&outside.path().join("big.bin"), 5_000_000);
        let root = t.path().join("root");
        fs::create_dir(&root).unwrap();
        write(&root.join("small.bin"), 1000);
        std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();
        let out = scan_dir(&root, &all(), |_| {});
        assert!(out.tree.size < 1_000_000, "followed symlink: {}", out.tree.size);
    }

    #[cfg(unix)]
    #[test]
    fn hard_link_counted_once() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("one.bin"), 1_000_000);
        fs::hard_link(t.path().join("one.bin"), t.path().join("two.bin")).unwrap();
        let out = scan_dir(t.path(), &all(), |_| {});
        assert!(out.tree.size >= 1_000_000 && out.tree.size < 1_500_000, "size={}", out.tree.size);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_dir_counted() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let locked = t.path().join("locked");
        write(&locked.join("f.bin"), 1000);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let out = scan_dir(t.path(), &all(), |_| {});
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if unsafe { libc_geteuid() } != 0 {
            assert!(out.errors >= 1);
            assert!(out.permission_denied >= 1);
        }
    }
    #[cfg(unix)]
    unsafe fn libc_geteuid() -> u32 {
        extern "C" {
            fn geteuid() -> u32;
        }
        geteuid()
    }

    #[test]
    fn small_files_are_folded() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("a.txt"), 10);
        write(&t.path().join("b.txt"), 10);
        let out = scan_dir(t.path(), &Options::default(), |_| {});
        assert_eq!(out.tree.children.len(), 1);
        assert_eq!(out.tree.children[0].name, SMALL_FILES);
        assert_eq!(out.tree.children[0].files, 2);
    }

    fn leaf(name: &str, size: u64) -> Node {
        Node { name: name.into(), size, is_dir: false, files: 1, children: vec![] }
    }

    #[test]
    fn prune_groups_small_children_into_other() {
        let tree = Node {
            name: "r".into(),
            size: 1000,
            is_dir: true,
            files: 5,
            children: vec![leaf("big", 900), leaf("t1", 4), leaf("t2", 3), leaf("t3", 2), leaf("t4", 1)],
        };
        let v = prune(&tree, Path::new("/r"), PRUNE_FRACTION, 1);
        assert_eq!(v.children.len(), 2);
        assert_eq!(v.children[0].name, "big");
        let other = &v.children[1];
        assert_eq!(other.name, OTHER);
        assert_eq!(other.size, 10);
        assert_eq!(other.files, 4);
        assert!(other.other);
    }

    #[test]
    fn remove_updates_ancestors() {
        let mut tree = Node {
            name: "r".into(),
            size: 300,
            is_dir: true,
            files: 3,
            children: vec![Node {
                name: "d".into(),
                size: 300,
                is_dir: true,
                files: 3,
                children: vec![leaf("f", 100), leaf("g", 200)],
            }],
        };
        assert!(remove(&mut tree, Path::new("/r"), Path::new("/r/d/f")));
        assert_eq!(tree.size, 200);
        assert_eq!(tree.children[0].size, 200);
        assert!(find(&tree, Path::new("/r"), Path::new("/r/d/g")).is_some());
        assert!(find(&tree, Path::new("/r"), Path::new("/r/d/f")).is_none());
    }
}
