//! Known space hogs: path patterns with a plain-words note and a "safe to clear?" value.
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Hog {
    pub id: &'static str,
    pub label: &'static str,
    /// "yes", "mostly" or "careful"
    pub safe: &'static str,
    pub note: &'static str,
}

enum Pat {
    /// Normalized path (forward slashes) ends with this suffix.
    Suffix(&'static str),
    /// Last path segment equals this.
    Name(&'static str),
    /// Last path segment equals this and a sibling file exists.
    NameWithSibling(&'static str, &'static str),
    /// Last segment equals `name` and the path contains `within`.
    NameWithin(&'static str, &'static str),
}

struct Rule {
    hog: Hog,
    pats: &'static [Pat],
}

macro_rules! hog {
    ($id:expr, $label:expr, $safe:expr, $note:expr) => {
        Hog { id: $id, label: $label, safe: $safe, note: $note }
    };
}

static RULES: &[Rule] = &[
    Rule {
        hog: hog!("xcode-derived", "Xcode DerivedData", "yes", "Build output. Safe to clear; Xcode rebuilds it."),
        pats: &[Pat::Suffix("/Library/Developer/Xcode/DerivedData")],
    },
    Rule {
        hog: hog!("coresimulator", "iOS simulators", "careful", "Simulator devices. Remove old ones with: xcrun simctl delete unavailable"),
        pats: &[Pat::Suffix("/Library/Developer/CoreSimulator")],
    },
    Rule {
        hog: hog!("docker", "Docker data", "careful", "Images, containers and volumes. Clean with: docker system prune"),
        pats: &[
            Pat::Suffix("/Library/Containers/com.docker.docker"),
            Pat::Suffix("/.docker"),
            Pat::Suffix("/AppData/Local/Docker"),
            Pat::Suffix("/AppData/Local/Docker Desktop"),
        ],
    },
    Rule {
        hog: hog!("node-modules", "node_modules", "yes", "Installed packages. Safe to delete; reinstall with your package manager."),
        pats: &[Pat::Name("node_modules")],
    },
    Rule {
        hog: hog!("cargo-target", "Cargo build output", "yes", "Rust build output. Safe to clear; cargo rebuilds it."),
        pats: &[Pat::NameWithSibling("target", "Cargo.toml"), Pat::Suffix("/.cargo/registry")],
    },
    Rule {
        hog: hog!("gradle", "Gradle cache", "mostly", "Downloaded dependencies and build caches. Re-downloaded on the next build."),
        pats: &[Pat::Suffix("/.gradle")],
    },
    Rule {
        hog: hog!("npm", "npm cache", "yes", "Download cache. Safe to clear (npm cache clean --force)."),
        pats: &[Pat::Suffix("/.npm"), Pat::Suffix("/AppData/Local/npm-cache")],
    },
    Rule {
        hog: hog!("pnpm-store", "pnpm store", "yes", "Shared package store. Safe to clear; run pnpm store prune or reinstall."),
        pats: &[
            Pat::Suffix("/Library/pnpm/store"),
            Pat::Suffix("/.local/share/pnpm/store"),
            Pat::Suffix("/AppData/Local/pnpm/store"),
            Pat::Name(".pnpm-store"),
        ],
    },
    Rule {
        hog: hog!("caches", "App caches", "mostly", "Caches apps rebuild on demand. Mostly safe; quit the app first."),
        pats: &[
            Pat::Suffix("/Library/Caches"),
            Pat::Suffix("/.cache"),
            Pat::Suffix("/AppData/Local/Temp"),
            Pat::NameWithin("Cache", "/AppData/Local/"),
            Pat::NameWithin("Caches", "/AppData/Local/"),
            Pat::NameWithin("Code Cache", "/AppData/Local/"),
        ],
    },
];

pub fn normalize(path: &str) -> String {
    let mut p = path.replace('\\', "/");
    while p.len() > 1 && p.ends_with('/') {
        p.pop();
    }
    p
}

/// Match a directory path against the known hog rules.
pub fn match_hog(path: &str, _home: Option<&Path>) -> Option<Hog> {
    let p = normalize(path);
    let name = p.rsplit('/').next().unwrap_or("");
    for r in RULES {
        for pat in r.pats {
            let hit = match pat {
                Pat::Suffix(s) => p.ends_with(s),
                Pat::Name(n) => name == *n,
                Pat::NameWithSibling(n, sib) => {
                    name == *n && Path::new(path).parent().map(|d| d.join(sib).exists()).unwrap_or(false)
                }
                Pat::NameWithin(n, w) => name == *n && p.contains(w),
            };
            if hit {
                return Some(r.hog.clone());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(p: &str) -> Option<&'static str> {
        match_hog(p, None).map(|h| h.id)
    }

    #[test]
    fn matches_known_paths() {
        assert_eq!(id("/Users/a/Library/Developer/Xcode/DerivedData"), Some("xcode-derived"));
        assert_eq!(id("/Users/a/Library/Developer/CoreSimulator"), Some("coresimulator"));
        assert_eq!(id("/Users/a/Library/Containers/com.docker.docker"), Some("docker"));
        assert_eq!(id("/Users/a/proj/node_modules"), Some("node-modules"));
        assert_eq!(id("/Users/a/Library/Caches"), Some("caches"));
        assert_eq!(id("/Users/a/.gradle"), Some("gradle"));
        assert_eq!(id("/Users/a/.npm"), Some("npm"));
        assert_eq!(id("/Users/a/Library/pnpm/store"), Some("pnpm-store"));
    }

    #[test]
    fn matches_windows_paths() {
        assert_eq!(id(r"C:\Users\a\AppData\Local\Temp"), Some("caches"));
        assert_eq!(id(r"C:\Users\a\AppData\Local\Docker"), Some("docker"));
        assert_eq!(id(r"C:\Users\a\AppData\Local\Google\Chrome\User Data\Default\Cache"), Some("caches"));
        assert_eq!(id(r"C:\Users\a\proj\node_modules\"), Some("node-modules"));
    }

    #[test]
    fn non_matches() {
        assert_eq!(id("/Users/a/Documents"), None);
        assert_eq!(id("/Users/a/node_modules_backup"), None);
        assert_eq!(id("/Users/a/Cache"), None);
    }

    #[test]
    fn cargo_target_needs_cargo_toml() {
        let t = tempfile::tempdir().unwrap();
        let target = t.path().join("target");
        std::fs::create_dir(&target).unwrap();
        assert_eq!(match_hog(target.to_str().unwrap(), None), None);
        std::fs::write(t.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(match_hog(target.to_str().unwrap(), None).map(|h| h.id), Some("cargo-target"));
    }
}
