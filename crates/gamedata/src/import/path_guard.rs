//! Path containment that links, letter case and not-yet-existing folders cannot fool. It decides
//! whether a cache location is inside the game install, which is never written to.

use std::path::{Component, Path, PathBuf};

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    }
}

/// Removes `.` and applies each `..` to the component before it, without looking at the disk.
/// This is how Windows itself reads a path before opening it.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The canonical form of the longest prefix of `path` that exists (links resolved, real letter
/// case), with the components that do not exist yet appended unchanged.
fn resolve_existing_prefix(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    let mut tail = Vec::new();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(&current) {
            return tail.iter().rev().fold(canonical, |p, name| p.join(name));
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                current = parent.to_path_buf();
            }
            // Nothing along the path exists; leave it as written.
            _ => return path.to_path_buf(),
        }
    }
}

/// Follows the path one component at a time, resolving each link as it is met, so `link/..` is
/// the parent of the link's target. This is how POSIX systems read a path.
fn physical(path: &Path) -> PathBuf {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                current.pop();
            }
            Component::Normal(name) => {
                current.push(name);
                if let Ok(canonical) = std::fs::canonicalize(&current) {
                    current = canonical;
                }
            }
            // A drive or root on its own is not looked up: a bare `C:` would mean the current
            // folder on that drive.
            other => current.push(other.as_os_str()),
        }
    }
    current
}

/// Lower-cased, so the comparison ignores letter case as Windows file systems do.
fn folded(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

/// Whether `path` is `root` or anywhere under it. `path` need not exist yet.
///
/// The two ways of reading `..` after a link (Windows applies it to the text of the path, POSIX
/// to the link's target) can name different folders, so both readings are checked and either
/// one landing inside `root` counts.
pub(crate) fn is_inside(path: &Path, root: &Path) -> bool {
    let path = absolute(path);
    let root = folded(&resolve_existing_prefix(&lexical(&absolute(root))));
    [
        resolve_existing_prefix(&lexical(&path)),
        resolve_existing_prefix(&physical(&path)),
    ]
    .iter()
    .any(|candidate| folded(candidate).starts_with(&root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{link_dir, TempDir};

    #[test]
    fn plain_containment() {
        let tmp = TempDir::new();
        let root = tmp.path().join("Game");
        std::fs::create_dir_all(root.join("Sub")).unwrap();
        std::fs::create_dir_all(tmp.path().join("Game2")).unwrap();

        assert!(is_inside(&root, &root));
        assert!(is_inside(&root.join("Sub"), &root));
        assert!(is_inside(
            &root.join("brand").join("new").join("sub"),
            &root
        ));
        assert!(is_inside(&root.join("Sub").join("..").join("x"), &root));
        // Letter case does not matter.
        let upper = PathBuf::from(root.to_string_lossy().to_uppercase());
        assert!(is_inside(&upper.join("NEW"), &root));

        assert!(!is_inside(tmp.path(), &root));
        assert!(!is_inside(&tmp.path().join("Game2"), &root)); // a sibling sharing the prefix
        assert!(!is_inside(&tmp.path().join("Game2").join("new"), &root));
        assert!(!is_inside(&root.join("..").join("elsewhere"), &root));
    }

    #[test]
    fn links_do_not_hide_the_root() {
        let tmp = TempDir::new();
        let root = tmp.path().join("Game");
        std::fs::create_dir_all(root.join("WillowGame")).unwrap();
        let outside = tmp.path().join("Outside");
        std::fs::create_dir_all(outside.join("Deep")).unwrap();

        let link = tmp.path().join("link");
        let link2 = tmp.path().join("link2");
        let out_link = root.join("out");
        if !link_dir(&root, &link)
            || !link_dir(&root.join("WillowGame"), &link2)
            || !link_dir(&outside.join("Deep"), &out_link)
        {
            eprintln!("skipped: cannot create directory links here");
            return;
        }
        assert!(is_inside(&link, &root));
        assert!(is_inside(&link.join("newdir").join("deeper"), &root));
        // POSIX reading: link2/.. is the game folder.
        assert!(is_inside(&link2.join("..").join("new"), &root));
        // Windows reading: Game/out/.. is the game folder, though the link points outside.
        assert!(is_inside(&out_link.join("..").join("new"), &root));
        // A link from inside the game to outside really is outside.
        assert!(!is_inside(&out_link.join("cache"), &root));
        assert!(!is_inside(&outside.join("cache"), &root));
    }
}
