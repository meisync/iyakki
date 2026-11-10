//! Where a person's files live in a cloud tenant (docs/development/CLOUD-MULTITENANT-PLAN.md, section 3b).
//!
//! DECISION: a tenant uses [`Isolation::Folders`] only (the safest option: one set of checks, no claim or worker pod per namespace to get
//! wrong). What keeps a namespace or a person from filling the shared disk is the explicit size limits of [`crate::cloudquota`]
//! (`storage.quota`: per folder and per tenant); what keeps them out of each other's files is the rights check plus the path jail below.
//! The `volumes` code in this file is kept as a tested extension point, but it is DEPRECATED: it is not deployed, and
//! [`Isolation::deployable`] refuses it on purpose.
//!
//! Everything that touches a tenant's files asks [`StorageLayout::locate`] for a [`Place`] (which volume, which folder on it) and never
//! builds a path itself. With [`Isolation::Folders`] (the first version) every place is a folder of the tenant's one volume; with
//! [`Isolation::Volumes`] a namespace gets its own volume (claim `<tenant volume>-ns-<namespace>`, mounted at `<root>/volumes/<claim>`), and only
//! this module and the manifest generator change. What is built for `volumes`: the resolver (every place of a namespace is on that namespace's
//! claim; the person's `personal` folders stay on the tenant's main volume), the claim's manifest ([`volume_claim_manifest`]), and a refusal when
//! a namespace's volume cannot be created or is not really a separate mount ([`VolumeProvisioner`]: [`Mounted`] checks, [`Provision`] creates
//! through a caller-supplied apply function, a fake in tests). Not built: the worker pod per namespace and the service's own manifest mounting the
//! claims (so `TenantConfig` still refuses `volumes`, see [`Isolation::deployable`]); moving a namespace's folder onto a new volume.
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
    /// A volume per namespace (and, later, a worker pod that mounts only that volume).
    Volumes,
}

impl Isolation {
    /// Both settings work in the storage layer.
    pub fn check(self) -> Result<(), String> {
        Ok(())
    }

    /// Whether a tenant can be DEPLOYED with this setting: the manifests do not yet mount a claim per namespace into the service or run a
    /// worker pod per namespace, so `volumes` is refused where a tenant is configured, with a message that says so.
    pub fn deployable(self) -> Result<(), String> {
        match self {
            Isolation::Folders => Ok(()),
            Isolation::Volumes => Err("isolation \"volumes\" (a volume per namespace) works in the storage layer, but it is not deployed, by decision (a tenant uses folders with size limits, see storage.quota; a claim and a worker pod per namespace are not built); use \"folders\"".into()),
        }
    }
}

/// The claim of a namespace under `isolation: volumes`. The namespace must be a valid claim name part (lower-case letters, digits, `-`).
fn namespace_claim(main: &str, ns: &str) -> Result<String, String> {
    if !crate::cloudtenant::dns_label(ns) || ns.len() > 40 {
        return Err(format!("namespace \"{ns}\" cannot have a volume of its own: use lower-case letters, digits and - (at most 40) in its name"));
    }
    Ok(format!("{main}-ns-{ns}"))
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
        if !plain_name(person) {
            return Err(format!("\"{person}\" is not a name that can be a folder"));
        }
        let ns = |n: &str| if plain_name(n) { Ok(n.to_string()) } else { Err(format!("\"{n}\" is not a name that can be a folder")) };
        let (volume, subpath): (String, PathBuf) = match (self.isolation, scope) {
            (Isolation::Folders, Scope::Person { namespace }) => (self.volume.clone(), ["ns", &ns(namespace)?, "people", person].iter().collect()),
            (Isolation::Folders, Scope::Shared { namespace }) => (self.volume.clone(), ["ns", &ns(namespace)?, "shared"].iter().collect()),
            // the namespace's own claim holds `people/<person>` and `shared`: the namespace is the volume
            (Isolation::Volumes, Scope::Person { namespace }) => (namespace_claim(&self.volume, &ns(namespace)?)?, ["people", person].iter().collect()),
            (Isolation::Volumes, Scope::Shared { namespace }) => (namespace_claim(&self.volume, &ns(namespace)?)?, PathBuf::from("shared")),
            (_, Scope::Personal) => (self.volume.clone(), ["personal", person].iter().collect()),
        };
        Ok(Place { volume, subpath })
    }
}

impl StorageLayout {
    /// THE EXTENSION POINT for `isolation: volumes`. The directory on this process's disk where `place`'s volume is mounted, given `root`
    /// (where the tenant's data volume is mounted). With [`Isolation::Folders`] every place lives on the one volume, so this is `root`.
    /// With a volume per namespace (later) the namespace's own claim is mounted next to the others (for example `root/volumes/<claim>`)
    /// and this is the only function that learns it; callers never join `root` and `place.subpath` themselves, they call [`resolve`]
    /// (or [`StorageLayout::mount`] plus [`ensure`]).
    pub fn mount(&self, root: &Path, place: &Place) -> Result<PathBuf, String> {
        if place.volume == self.volume {
            return Ok(root.to_path_buf());
        }
        let own = self.isolation == Isolation::Volumes && place.volume.strip_prefix(&format!("{}-ns-", self.volume)).is_some_and(|ns| namespace_claim(&self.volume, ns).is_ok());
        if !own {
            return Err(format!("the volume \"{}\" is not mounted here", place.volume));
        }
        Ok(root.join("volumes").join(&place.volume))
    }
}

/// Makes sure a namespace's volume exists and is mounted at `mount` before anything is written there. An error is a refusal: nothing is
/// written to a folder that is not on the namespace's own volume (that would put its files on the tenant's main volume, silently).
pub trait VolumeProvisioner {
    fn ensure(&self, claim: &str, mount: &Path, root: &Path) -> Result<(), String>;
}

/// Checks only: the claim is mounted at `mount` and is another file system than `root`'s. The default for [`resolve`].
pub struct Mounted;

impl VolumeProvisioner for Mounted {
    fn ensure(&self, claim: &str, mount: &Path, root: &Path) -> Result<(), String> {
        use std::os::unix::fs::MetadataExt;
        let md = std::fs::symlink_metadata(mount).map_err(|_| format!("the volume \"{claim}\" is not mounted at {}: the service has no such volume", mount.display()))?;
        if md.file_type().is_symlink() || !md.is_dir() {
            return Err(format!("{} is not a folder: the volume \"{claim}\" is not mounted there", mount.display()));
        }
        let main = std::fs::metadata(root).map_err(|e| format!("{}: {e}", root.display()))?;
        if main.dev() == md.dev() {
            return Err(format!("{} is on the tenant's main volume, not on the volume \"{claim}\": refusing to put the namespace's files there", mount.display()));
        }
        Ok(())
    }
}

/// The claim's manifest: one `ReadWriteOnce` claim per namespace, labelled with the tenant. `size` is a quantity such as `5Gi`.
pub fn volume_claim_manifest(tenant: &str, claim: &str, size: &str, class: Option<&str>) -> Result<String, String> {
    use crate::cloudtenant::{dns_label, dns_subdomain};
    let digits = size.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    if !dns_label(tenant) || !dns_subdomain(claim) || digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) || !matches!(&size[digits.len()..], "Mi" | "Gi" | "Ti") {
        return Err(format!("the claim \"{claim}\" of tenant \"{tenant}\" with size \"{size}\" is not valid (a size looks like 5Gi)"));
    }
    if class.is_some_and(|c| !dns_subdomain(c)) {
        return Err("the storage class is not a name".into());
    }
    let sc = class.map(|c| format!("  storageClassName: {c}\n")).unwrap_or_default();
    Ok(format!(
        "apiVersion: v1\nkind: PersistentVolumeClaim\nmetadata:\n  name: {claim}\n  namespace: iyakki-{tenant}\n  labels:\n    app.kubernetes.io/name: iyakki\n    iyakki.dev/tenant: {tenant}\n    iyakki.dev/volume: namespace\nspec:\n  accessModes: [\"ReadWriteOnce\"]\n{sc}  resources:\n    requests:\n      storage: {size}\n"
    ))
}

/// Creates a namespace's claim through `apply` (for example `kubectl apply -f -`; a fake in tests), then checks that it is mounted. When
/// the claim cannot be created, or is not mounted afterwards, the answer is an error and nothing is written.
pub struct Provision<'a> {
    pub tenant: &'a str,
    pub size: &'a str,
    pub class: Option<&'a str>,
    pub apply: &'a dyn Fn(&str) -> Result<(), String>,
}

impl VolumeProvisioner for Provision<'_> {
    fn ensure(&self, claim: &str, mount: &Path, root: &Path) -> Result<(), String> {
        let yaml = volume_claim_manifest(self.tenant, claim, self.size, self.class)?;
        (self.apply)(&yaml).map_err(|e| format!("the volume \"{claim}\" could not be created: {e}"))?;
        Mounted.ensure(claim, mount, root)
    }
}

/// The one way the file routes get a real folder: locate the place of `person` in `scope`, find where its volume is mounted, make the
/// folder (mode 0700, no links) and jail `rel` (a path the person gave, may be empty for the folder itself) inside it.
pub fn resolve(layout: &StorageLayout, root: &Path, person: &str, scope: &Scope, rel: &str) -> Result<PathBuf, String> {
    resolve_with(layout, root, &Mounted, person, scope, rel)
}

/// [`resolve`] with the way a namespace's volume is made sure of (only used with `isolation: volumes`).
pub fn resolve_with(layout: &StorageLayout, root: &Path, volumes: &dyn VolumeProvisioner, person: &str, scope: &Scope, rel: &str) -> Result<PathBuf, String> {
    let place = layout.locate(person, scope)?;
    let mount = layout.mount(root, &place)?;
    if place.volume != layout.volume {
        std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
        volumes.ensure(&place.volume, &mount, root)?;
    }
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
        // under `folders` a namespace's own claim is not served by this mount either
        let v = StorageLayout { isolation: Isolation::Volumes, ..Default::default() };
        assert!(l.mount(&root, &v.locate("asha", &sc).unwrap()).unwrap_err().contains("not mounted"));
        assert!(Isolation::Volumes.check().is_ok() && Isolation::Folders.check().is_ok());
        assert!(Isolation::Folders.deployable().is_ok() && Isolation::Volumes.deployable().unwrap_err().contains("not deployed"));
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

    fn volumes() -> StorageLayout {
        StorageLayout { isolation: Isolation::Volumes, ..Default::default() }
    }

    /// A provisioner that makes the mount folder (a stand-in for a claim that was created and mounted), or refuses; it remembers the claims.
    struct Fake {
        fail: Option<&'static str>,
        asked: std::cell::RefCell<Vec<String>>,
    }
    impl VolumeProvisioner for Fake {
        fn ensure(&self, claim: &str, mount: &Path, _root: &Path) -> Result<(), String> {
            self.asked.borrow_mut().push(claim.to_string());
            match self.fail {
                Some(m) => Err(format!("the volume \"{claim}\" could not be created: {m}")),
                None => std::fs::create_dir_all(mount).map_err(|e| e.to_string()),
            }
        }
    }
    fn fake(fail: Option<&'static str>) -> Fake {
        Fake { fail, asked: Default::default() }
    }

    #[test]
    fn with_volumes_each_namespace_is_its_own_claim_and_personal_folders_stay_on_the_main_volume() {
        let l = volumes();
        let ann = l.locate("ann", &Scope::Person { namespace: "web".into() }).unwrap();
        assert_eq!(ann, Place { volume: "iyakki-data-ns-web".into(), subpath: PathBuf::from("people/ann") });
        let shared = l.locate("ann", &Scope::Shared { namespace: "web".into() }).unwrap();
        assert_eq!((shared.volume.as_str(), shared.subpath.to_str()), ("iyakki-data-ns-web", Some("shared")));
        assert_ne!(l.locate("ann", &Scope::Shared { namespace: "db".into() }).unwrap().volume, shared.volume, "another namespace, another volume");
        assert_eq!(l.locate("ann", &Scope::Personal).unwrap(), Place { volume: "iyakki-data".into(), subpath: PathBuf::from("personal/ann") });
        // a name that cannot be part of a claim name has no volume
        for bad in ["Web", "my_ns", "-x", &"a".repeat(41)] {
            let e = l.locate("ann", &Scope::Shared { namespace: bad.into() }).unwrap_err();
            assert!(e.contains("cannot have a volume"), "{bad}: {e}");
        }
        // the folders layout is unchanged, and names are still checked first
        assert_eq!(StorageLayout::default().locate("ann", &Scope::Shared { namespace: "Web".into() }).unwrap().volume, "iyakki-data");
        assert!(l.locate("../x", &Scope::Personal).is_err() && l.locate("ann", &Scope::Shared { namespace: "..".into() }).is_err());
        // mounts: the namespace's claim under root/volumes, the main one at root, strangers refused
        let root = dir("vol-mount");
        assert_eq!(l.mount(&root, &ann).unwrap(), root.join("volumes/iyakki-data-ns-web"));
        assert_eq!(l.mount(&root, &l.locate("ann", &Scope::Personal).unwrap()).unwrap(), root);
        for foreign in ["other-claim", "iyakki-data-ns-", "iyakki-data-ns-Web", "iyakki-data-ns-../x"] {
            assert!(l.mount(&root, &Place { volume: foreign.into(), subpath: ann.subpath.clone() }).is_err(), "{foreign}");
        }
    }

    #[test]
    fn resolve_with_volumes_puts_files_on_the_namespaces_volume_inside_the_same_jail() {
        let (root, l) = (dir("vol-resolve"), volumes());
        let (f, ann) = (fake(None), Scope::Person { namespace: "web".into() });
        let p = resolve_with(&l, &root, &f, "ann", &ann, "docs/a.txt").unwrap();
        assert_eq!(p, root.join("volumes/iyakki-data-ns-web/people/ann/docs/a.txt"));
        assert!(root.join("volumes/iyakki-data-ns-web/people/ann").is_dir() && !root.join("ns").exists(), "nothing on the main volume's namespace folders");
        assert_eq!(f.asked.borrow().as_slice(), ["iyakki-data-ns-web"]);
        // the personal folder is on the main volume and asks nobody
        let f2 = fake(None);
        assert_eq!(resolve_with(&l, &root, &f2, "ann", &Scope::Personal, "d.txt").unwrap(), root.join("personal/ann/d.txt"));
        assert!(f2.asked.borrow().is_empty());
        // the jail is the same
        for bad in ["../x", "/etc/passwd", "a/../../b"] {
            assert!(resolve_with(&l, &root, &fake(None), "ann", &ann, bad).is_err(), "{bad}");
        }
        #[cfg(unix)]
        {
            let outside = dir("vol-outside");
            std::os::unix::fs::symlink(&outside, root.join("volumes/iyakki-data-ns-web/people/ann/out")).unwrap();
            assert!(resolve_with(&l, &root, &fake(None), "ann", &ann, "out/x").unwrap_err().contains("link"));
        }
        // a person of another namespace cannot name this one's folder: the scope picks the volume, not the path
        assert!(resolve_with(&l, &root, &fake(None), "bob", &Scope::Person { namespace: "db".into() }, "../../iyakki-data-ns-web/people/ann/docs/a.txt").is_err());
    }

    #[test]
    fn a_volume_that_cannot_be_created_or_is_not_a_separate_mount_is_a_refusal_that_writes_nothing() {
        let (root, l) = (dir("vol-refuse"), volumes());
        let sc = Scope::Shared { namespace: "web".into() };
        let e = resolve_with(&l, &root, &fake(Some("quota exceeded")), "ann", &sc, "x.txt").unwrap_err();
        assert!(e.contains("could not be created") && e.contains("quota exceeded"), "{e}");
        assert!(!root.join("volumes").exists(), "no folder was made on the main volume instead");
        // the default check: nothing mounted
        let e = resolve(&l, &root, "ann", &sc, "x.txt").unwrap_err();
        assert!(e.contains("not mounted"), "{e}");
        assert!(!root.join("volumes").exists());
        // a folder on the main volume where the claim should be is not the claim
        std::fs::create_dir_all(root.join("volumes/iyakki-data-ns-web")).unwrap();
        let e = resolve(&l, &root, "ann", &sc, "x.txt").unwrap_err();
        assert!(e.contains("main volume") && e.contains("refusing"), "{e}");
        assert!(!root.join("volumes/iyakki-data-ns-web/shared").exists(), "nothing was put there");
        // a link in the place of the mount is not a mount
        #[cfg(unix)]
        {
            let other = dir("vol-other");
            std::fs::remove_dir(root.join("volumes/iyakki-data-ns-web")).unwrap();
            std::os::unix::fs::symlink(&other, root.join("volumes/iyakki-data-ns-web")).unwrap();
            assert!(resolve(&l, &root, "ann", &sc, "x.txt").unwrap_err().contains("not a folder"));
            assert!(std::fs::read_dir(&other).unwrap().next().is_none());
        }
        // `Mounted` accepts a real other file system (Linux: /proc is one)
        #[cfg(target_os = "linux")]
        assert!(Mounted.ensure("c", Path::new("/proc"), &root).is_ok());
    }

    #[test]
    fn the_claim_manifest_is_one_labelled_claim_and_bad_values_are_refused_before_anything_is_applied() {
        let y = volume_claim_manifest("acme", "iyakki-data-ns-web", "5Gi", Some("fast")).unwrap();
        for want in ["kind: PersistentVolumeClaim", "name: iyakki-data-ns-web", "namespace: iyakki-acme", "iyakki.dev/tenant: acme", "storage: 5Gi", "storageClassName: fast", "ReadWriteOnce"] {
            assert!(y.contains(want), "{want}\n{y}");
        }
        assert!(!volume_claim_manifest("acme", "c", "1Gi", None).unwrap().contains("storageClassName"));
        for (t, c, sz, cl) in [("Acme", "c", "1Gi", None), ("acme", "C x", "1Gi", None), ("acme", "c\nkind: x", "1Gi", None), ("acme", "c", "1Gi\nx", None), ("acme", "c", "lots", None), ("acme", "c", "1Gi", Some("a b"))] {
            assert!(volume_claim_manifest(t, c, sz, cl).is_err(), "{t} {c} {sz:?}");
        }
        // Provision applies the manifest, and a failing apply is a "could not be created" refusal
        let (root, l) = (dir("vol-prov"), volumes());
        let sc = Scope::Shared { namespace: "web".into() };
        let seen = std::cell::RefCell::new(Vec::new());
        let ok = |y: &str| -> Result<(), String> {
            seen.borrow_mut().push(y.to_string());
            Ok(())
        };
        let prov = Provision { tenant: "acme", size: "5Gi", class: None, apply: &ok };
        // applied, but never mounted here: still refused
        let e = resolve_with(&l, &root, &prov, "ann", &sc, "").unwrap_err();
        assert!(e.contains("not mounted"), "{e}");
        assert!(seen.borrow()[0].contains("name: iyakki-data-ns-web"));
        let bad = |_: &str| -> Result<(), String> { Err("forbidden".into()) };
        let prov = Provision { tenant: "acme", size: "5Gi", class: None, apply: &bad };
        assert!(resolve_with(&l, &root, &prov, "ann", &sc, "").unwrap_err().contains("could not be created: forbidden"));
        let prov = Provision { tenant: "acme", size: "huge", class: None, apply: &ok };
        let n = seen.borrow().len();
        assert!(resolve_with(&l, &root, &prov, "ann", &sc, "").is_err());
        assert_eq!(seen.borrow().len(), n, "a bad size applies nothing");
        assert!(!root.join("volumes/iyakki-data-ns-web/shared").exists());
    }

    #[test]
    fn the_setting_round_trips_and_a_tenant_still_cannot_be_deployed_with_volumes() {
        assert_eq!(serde_json::to_string(&Isolation::Volumes).unwrap(), "\"volumes\"");
        assert_eq!(serde_json::from_str::<Isolation>("\"folders\"").unwrap(), Isolation::Folders);
        assert!(serde_json::from_str::<Isolation>("\"disks\"").is_err());
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
