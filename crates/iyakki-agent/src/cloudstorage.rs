//! Where a person's files live in a cloud tenant (docs/development/CLOUD-MULTITENANT-PLAN.md, section 3b).
//!
//! Everything that touches a tenant's files asks [`StorageLayout::locate`] for a [`Place`] (which volume, which folder on it) and never
//! builds a path itself. With [`Isolation::Folders`] (the first version) every place is a folder of the tenant's one volume; with
//! [`Isolation::Volumes`] (later) a namespace gets its own volume, and only this module and the manifest generator change.
//!
//! [`jail`] is the path jail: it maps a path a person gave (relative, to a place) onto the real folder, refusing `..`, absolute paths and
//! any symlink on the way, so a clone that plants a link to `/` or to another person's folder cannot lead out.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How the tenant's files are separated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Isolation {
    /// Folders of one volume, kept apart by the path jail and the membership checks.
    #[default]
    Folders,
    /// A volume (and a worker pod) per namespace. Not built yet: [`Isolation::check`] refuses it.
    Volumes,
}

impl Isolation {
    pub fn check(self) -> Result<(), String> {
        match self {
            Isolation::Folders => Ok(()),
            Isolation::Volumes => Err("isolation \"volumes\" (a volume per namespace) is planned and not built yet; use \"folders\"".into()),
        }
    }
}

/// What a folder is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Work of one person in one namespace.
    Person { namespace: String },
    /// What the members of one namespace share.
    Shared { namespace: String },
    /// The person's own, outside every namespace.
    Personal,
}

/// A folder on a volume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// The claim that holds it (`iyakki-data` for every place with [`Isolation::Folders`]).
    pub volume: String,
    /// The folder, relative to the volume's mount.
    pub subpath: PathBuf,
}

#[derive(Clone, Debug)]
pub struct StorageLayout {
    pub isolation: Isolation,
    /// The tenant's data claim.
    pub volume: String,
}

impl Default for StorageLayout {
    fn default() -> StorageLayout {
        StorageLayout { isolation: Isolation::Folders, volume: "iyakki-data".into() }
    }
}

/// A name that is one plain path component: letters, digits, `-` and `_`, not empty, not starting with a dot.
pub fn plain_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && !s.starts_with('.')
}

impl StorageLayout {
    /// The place for `person` in `scope`. The names come from the signed-in session, and are checked all the same.
    pub fn locate(&self, person: &str, scope: &Scope) -> Result<Place, String> {
        self.isolation.check()?;
        if !plain_name(person) {
            return Err(format!("\"{person}\" is not a name that can be a folder"));
        }
        let ns = |n: &str| if plain_name(n) { Ok(n.to_string()) } else { Err(format!("\"{n}\" is not a name that can be a folder")) };
        let subpath: PathBuf = match scope {
            Scope::Person { namespace } => ["ns", &ns(namespace)?, "people", person].iter().collect(),
            Scope::Shared { namespace } => ["ns", &ns(namespace)?, "shared"].iter().collect(),
            Scope::Personal => ["personal", person].iter().collect(),
        };
        Ok(Place { volume: self.volume.clone(), subpath })
    }
}

impl StorageLayout {
    /// THE EXTENSION POINT for `isolation: volumes`. The directory on this process's disk where `place`'s volume is mounted, given `root`
    /// (where the tenant's data volume is mounted). With [`Isolation::Folders`] every place lives on the one volume, so this is `root`.
    /// With a volume per namespace (later) the namespace's own claim is mounted next to the others (for example `root/volumes/<claim>`)
    /// and this is the only function that learns it; callers never join `root` and `place.subpath` themselves, they call [`resolve`]
    /// (or [`StorageLayout::mount`] plus [`ensure`]).
    pub fn mount(&self, root: &Path, place: &Place) -> Result<PathBuf, String> {
        self.isolation.check()?;
        if place.volume != self.volume {
            return Err(format!("the volume \"{}\" is not mounted here", place.volume));
        }
        Ok(root.to_path_buf())
    }
}

/// The one way the file routes get a real folder: locate the place of `person` in `scope`, find where its volume is mounted, make the
/// folder (mode 0700, no links) and jail `rel` (a path the person gave, may be empty for the folder itself) inside it.
pub fn resolve(layout: &StorageLayout, root: &Path, person: &str, scope: &Scope, rel: &str) -> Result<PathBuf, String> {
    let place = layout.locate(person, scope)?;
    let mount = layout.mount(root, &place)?;
    std::fs::create_dir_all(&mount).map_err(|e| format!("{}: {e}", mount.display()))?;
    let base = ensure(&mount, &place)?;
    jail(&base, rel)
}

/// The real folder of `place` under the volume's mount `root`, made (mode 0700) if it is not there.
pub fn ensure(root: &Path, place: &Place) -> Result<PathBuf, String> {
    let mut at = root.to_path_buf();
    for c in place.subpath.components() {
        let Component::Normal(n) = c else { return Err("a place is made of plain names".into()) };
        at.push(n);
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_symlink() => return Err(format!("{} is a link; places are never links", at.display())),
            Ok(m) if !m.is_dir() => return Err(format!("{} is not a folder", at.display())),
            Ok(_) => {}
            Err(_) => {
                std::fs::create_dir(&at).map_err(|e| format!("{}: {e}", at.display()))?;
                set_private(&at);
            }
        }
    }
    Ok(at)
}

#[cfg(unix)]
fn set_private(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700));
}
#[cfg(not(unix))]
fn set_private(_: &Path) {}

/// The path a person gave (`rel`, relative to their place) as a real path inside `base` (the folder [`ensure`] returned). Refused: an
/// absolute path, `..`, and a symlink anywhere along the path that exists (a clone can plant one). The path itself need not exist (a
/// file about to be written). The result is always `base` joined with plain names.
pub fn jail(base: &Path, rel: &str) -> Result<PathBuf, String> {
    let mut at = base.to_path_buf();
    for c in Path::new(rel).components() {
        match c {
            Component::CurDir => {}
            Component::Normal(n) => {
                at.push(n);
                if let Ok(m) = std::fs::symlink_metadata(&at) {
                    if m.file_type().is_symlink() {
                        return Err(format!("{rel}: a link on the way leaves the folder"));
                    }
                }
            }
            Component::ParentDir => return Err(format!("{rel}: \"..\" is not allowed")),
            Component::RootDir | Component::Prefix(_) => return Err(format!("{rel}: give a path inside your folder, not an absolute one")),
        }
    }
    Ok(at)
}

/// The place is inside the work area of this person or namespace for sure: the check the clone and file routes run again after a checkout
/// (a repository can contain links): no link inside `base` points outside it. Returns the first offender.
pub fn links_leaving(base: &Path) -> Option<PathBuf> {
    let root = std::fs::canonicalize(base).ok()?;
    let mut stack = vec![base.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
            if m.file_type().is_symlink() {
                match std::fs::canonicalize(&p) {
                    Ok(t) if t.starts_with(&root) => {}
                    _ => return Some(p),
                }
            } else if m.is_dir() {
                stack.push(p);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-cloudstorage-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn places_are_folders_of_the_one_volume_by_namespace_and_person() {
        let l = StorageLayout::default();
        let p = l.locate("asha", &Scope::Person { namespace: "web".into() }).unwrap();
        assert_eq!(p, Place { volume: "iyakki-data".into(), subpath: PathBuf::from("ns/web/people/asha") });
        assert_eq!(l.locate("asha", &Scope::Shared { namespace: "web".into() }).unwrap().subpath, PathBuf::from("ns/web/shared"));
        assert_eq!(l.locate("asha", &Scope::Personal).unwrap().subpath, PathBuf::from("personal/asha"));
        assert_ne!(l.locate("ravi", &Scope::Personal).unwrap(), l.locate("asha", &Scope::Personal).unwrap());
    }

    #[test]
    fn mount_is_the_extension_point_and_resolve_goes_through_locate_and_the_jail() {
        let root = dir("resolve");
        let l = StorageLayout::default();
        let sc = Scope::Shared { namespace: "web".into() };
        let place = l.locate("asha", &sc).unwrap();
        assert_eq!(l.mount(&root, &place).unwrap(), root, "folders: one volume, one mount");
        let foreign = Place { volume: "other-claim".into(), subpath: place.subpath.clone() };
        assert!(l.mount(&root, &foreign).unwrap_err().contains("not mounted"), "a place on another volume is not served by this mount");
        let p = resolve(&l, &root, "asha", &sc, "docs/a.txt").unwrap();
        assert_eq!(p, root.join("ns/web/shared/docs/a.txt"));
        for bad in ["../x", "/etc/passwd"] {
            assert!(resolve(&l, &root, "asha", &sc, bad).is_err());
        }
        // volumes stay refused by every entry point, with the same clear message
        let v = StorageLayout { isolation: Isolation::Volumes, ..Default::default() };
        for r in [v.mount(&root, &place).unwrap_err(), resolve(&v, &root, "asha", &sc, "").unwrap_err()] {
            assert!(r.contains("not built yet"), "{r}");
        }
        assert!(Isolation::Volumes.check().unwrap_err().contains("planned"));
        assert!(Isolation::Folders.check().is_ok());
        // the setting round-trips by name
        assert_eq!(serde_json::from_str::<Isolation>("\"volumes\"").unwrap(), Isolation::Volumes);
    }

    #[test]
    fn a_name_that_is_not_one_plain_component_is_refused() {
        let l = StorageLayout::default();
        for bad in ["", "..", ".hidden", "a/b", "a b", "a\\b", "x\0y", "../ns/other"] {
            assert!(l.locate(bad, &Scope::Personal).is_err(), "person {bad:?}");
            assert!(l.locate("asha", &Scope::Shared { namespace: bad.into() }).is_err(), "namespace {bad:?}");
        }
    }

    #[test]
    fn a_volume_per_namespace_is_refused_with_a_clear_message_until_it_is_built() {
        let l = StorageLayout { isolation: Isolation::Volumes, ..Default::default() };
        let e = l.locate("asha", &Scope::Personal).unwrap_err();
        assert!(e.contains("not built yet"), "{e}");
    }

    #[test]
    fn ensure_makes_private_folders_and_refuses_a_link_in_the_way() {
        let root = dir("ensure");
        let l = StorageLayout::default();
        let place = l.locate("asha", &Scope::Person { namespace: "web".into() }).unwrap();
        let at = ensure(&root, &place).unwrap();
        assert!(at.ends_with("ns/web/people/asha") && at.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&at).unwrap().permissions().mode() & 0o777, 0o700);
        }
        // the namespace folder replaced by a link to somewhere else
        let other = dir("ensure-other");
        let evil = root.join("ns/mal");
        std::fs::create_dir_all(evil.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&other, &evil).unwrap();
        let place = l.locate("asha", &Scope::Person { namespace: "mal".into() }).unwrap();
        assert!(ensure(&root, &place).unwrap_err().contains("link"));
        assert!(std::fs::read_dir(&other).unwrap().next().is_none(), "nothing was made behind the link");
    }

    #[test]
    fn the_jail_refuses_parent_absolute_and_linked_paths_and_allows_new_files() {
        let base = dir("jail");
        std::fs::create_dir_all(base.join("repo/src")).unwrap();
        assert_eq!(jail(&base, "repo/src/new.rs").unwrap(), base.join("repo/src/new.rs"));
        assert_eq!(jail(&base, "./repo").unwrap(), base.join("repo"));
        for bad in ["../x", "repo/../../x", "/etc/passwd", "repo/../.."] {
            assert!(jail(&base, bad).is_err(), "{bad}");
        }
        #[cfg(unix)]
        {
            let outside = dir("jail-outside");
            std::fs::write(outside.join("secret"), "s").unwrap();
            std::os::unix::fs::symlink(&outside, base.join("repo/escape")).unwrap();
            assert!(jail(&base, "repo/escape/secret").unwrap_err().contains("link"));
            assert!(jail(&base, "repo/escape").is_err(), "the link itself too");
        }
    }

    #[test]
    fn a_checkout_with_a_link_that_leaves_the_folder_is_found() {
        let base = dir("links");
        std::fs::create_dir_all(base.join("repo/a")).unwrap();
        std::fs::write(base.join("repo/a/f"), "x").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(base.join("repo/a"), base.join("repo/inside")).unwrap();
            assert!(links_leaving(&base).is_none(), "a link that stays inside is fine");
            std::os::unix::fs::symlink("/", base.join("repo/out")).unwrap();
            assert_eq!(links_leaving(&base), Some(base.join("repo/out")));
        }
    }
}
