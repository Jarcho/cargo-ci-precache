use serde::{
    de::{SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use std::{
    collections::{HashMap, HashSet},
    ffi::{OsStr, OsString},
    fmt,
    path::PathBuf,
};

#[derive(Deserialize)]
struct Package {
    source: Option<String>,
    manifest_path: PathBuf,
}
enum CachedPackage<'a> {
    Registry {
        registry: &'a OsStr,
        name: &'a OsStr,
    },
    Git {
        repo: &'a OsStr,
        rev: &'a OsStr,
    },
}
impl<'a> CachedPackage<'a> {
    fn new(p: &'a Package) -> Option<Self> {
        let source = p.source.as_ref().map(String::as_str)?;
        Some(if source.starts_with("registry+") {
            Self::Registry {
                registry: p.manifest_path.parent()?.parent()?.file_name()?,
                name: p.manifest_path.parent()?.file_name()?, 
            }
        } else if source.starts_with("git+") {
            Self::Git {
                repo: p.manifest_path.parent()?.parent()?.file_name()?,
                rev: p.manifest_path.parent()?.file_name()?,
            }
        } else {
            return None;
        })
    }
}

/// Directory names for packages in the global cargo cache, stored for lookup during filesystem
/// traversal.
#[derive(Default)]
pub struct PackageSet {
    /// registry -> package map. package has the form `{name}-{version}`.
    pub registry: HashMap<OsString, HashSet<OsString>>,
    /// repository -> commit map.
    pub git: HashMap<OsString, HashSet<OsString>>,
}
impl<'d> Deserialize<'d> for PackageSet {
    fn deserialize<D: Deserializer<'d>>(d: D) -> Result<Self, D::Error> {
        struct V(PackageSet);
        impl<'d> Visitor<'d> for V {
            type Value = PackageSet;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a sequence of package structures")
            }

            fn visit_seq<A: SeqAccess<'d>>(mut self, mut seq: A) -> Result<Self::Value, A::Error> {
                while let Some(p) = seq.next_element::<Package>()? {
                    match CachedPackage::new(&p) {
                        None => (),
                        Some(CachedPackage::Registry { registry, name }) => {
                            self.0
                                .registry
                                .entry(registry.into())
                                .or_default()
                                .insert(name.into());
                        }
                        Some(CachedPackage::Git { repo, rev }) => {
                            self.0
                                .git
                                .entry(repo.into())
                                .or_default()
                                .insert(rev.into());
                        }
                    }
                }
                Ok(self.0)
            }
        }

        d.deserialize_seq(V(Default::default()))
    }
}

#[derive(Deserialize)]
pub struct Metadata {
    pub packages: PackageSet,
    pub target_directory: PathBuf,
}
