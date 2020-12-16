use serde::{
    de::{SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fmt,
    path::PathBuf,
};

#[derive(Deserialize)]
struct Package {
    source: Option<String>,
    manifest_path: PathBuf,
    id: String,
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
        let source = p.source.as_deref()?;
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
    pub registry: HashMap<OsString, HashMap<OsString, String>>,
    /// repository -> commit map.
    pub git: HashMap<OsString, HashMap<OsString, String>>,
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
                                .insert(name.into(), p.id);
                        }
                        Some(CachedPackage::Git { repo, rev }) => {
                            self.0
                                .git
                                .entry(repo.into())
                                .or_default()
                                .insert(rev.into(), p.id);
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
struct ResolveNode {
    id: String,
    features: Vec<String>,
}
fn build_feature_string(features: &[String]) -> String {
    let mut s =
        String::with_capacity(features.iter().map(|s| s.len()).sum::<usize>() + features.len() * 4);
    s.push('[');
    if let Some(f) = features.first() {
        s.push('"');
        s.push_str(f);
        s.push('"');

        for f in &features[1..] {
            s.push_str(", \"");
            s.push_str(f);
            s.push('"');
        }
    }
    s.push(']');
    s
}

#[derive(Default)]
struct ResolveNodes {
    package_features: HashMap<String, String>,
}
impl<'d> Deserialize<'d> for ResolveNodes {
    fn deserialize<D: Deserializer<'d>>(d: D) -> Result<Self, D::Error> {
        struct V(ResolveNodes);
        impl<'d> Visitor<'d> for V {
            type Value = ResolveNodes;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a resolve structure")
            }

            fn visit_seq<A: SeqAccess<'d>>(mut self, mut seq: A) -> Result<Self::Value, A::Error> {
                while let Some(n) = seq.next_element::<ResolveNode>()? {
                    self.0
                        .package_features
                        .insert(n.id, build_feature_string(&n.features));
                }
                Ok(self.0)
            }
        }

        d.deserialize_seq(V(Default::default()))
    }
}

fn deserialize_resolve<'d, D: Deserializer<'d>>(d: D) -> Result<HashMap<String, String>, D::Error> {
    #[derive(Deserialize)]
    struct X {
        nodes: ResolveNodes,
    }

    X::deserialize(d).map(|x| x.nodes.package_features)
}

#[derive(Deserialize)]
pub struct Metadata {
    pub packages: PackageSet,
    pub target_directory: PathBuf,

    #[serde(deserialize_with = "deserialize_resolve", rename = "resolve")]
    pub package_features: HashMap<String, String>,
}
