use serde::{Deserialize, Deserializer};
use std::{
    hash::{Hash, Hasher},
    path::PathBuf,
};

// from cargo/core/compiler/fingerprint.rs
#[derive(Debug, Deserialize)]
pub struct Fingerprint {
    pub rustc: u64,
    pub features: String,
    pub target: u64,
    pub profile: u64,
    pub path: u64,
    pub deps: Vec<DepFingerprint>,
    pub local: Vec<LocalFingerprint>,
    pub rustflags: Vec<String>,
    pub metadata: u64,
    pub config: u64,
}
impl Fingerprint {
    pub fn get_hash(&self) -> u64 {
        #[allow(deprecated)]
        let mut hasher = core::hash::SipHasher::default();
        self.hash(&mut hasher);
        hasher.finish()
    }
}
impl Hash for Fingerprint {
    fn hash<H: Hasher>(&self, h: &mut H) {
        (
            self.rustc,
            &self.features,
            self.target,
            self.path,
            self.profile,
            &self.local,
            self.metadata,
            self.config,
            &self.rustflags,
        )
            .hash(h);

        h.write_usize(self.deps.len());
        for dep in &self.deps {
            dep.pkg_id.hash(h);
            dep.name.hash(h);
            dep.public.hash(h);
            h.write_u64(dep.fingerprint);
        }
    }
}

#[derive(Debug)]
pub struct DepFingerprint {
    pub pkg_id: u64,
    pub name: String,
    pub public: bool,
    pub fingerprint: u64,
}
impl<'d> Deserialize<'d> for DepFingerprint {
    fn deserialize<D: Deserializer<'d>>(d: D) -> Result<Self, D::Error> {
        let (pkg_id, name, public, fingerprint) = <(u64, String, bool, u64)>::deserialize(d)?;
        Ok(Self {
            pkg_id,
            name,
            public,
            fingerprint,
        })
    }
}

#[derive(Debug, Deserialize, Hash)]
pub enum LocalFingerprint {
    Precalculated(String),
    CheckDepInfo {
        dep_info: PathBuf,
    },
    RerunIfChanged {
        output: PathBuf,
        paths: Vec<PathBuf>,
    },
    RerunIfEnvChanged {
        var: String,
        val: Option<String>,
    },
}

#[cfg(test)]
mod test {
    // Hash result changes based on the target.
    // Will rustc version also change the result?

    #[allow(unused)]
    static FILE: &str = r#"{
            "rustc": 5115962679530443550,
            "features": "[]",
            "target": 16343417806311904822,
            "profile": 16668067249205866872,
            "path": 16210749786564134395,
            "deps": [
                [
                    17671881657559241013,
                    "winapi",
                    false,
                    17268406378410745745
                ]
            ],
            "local": [
                {
                    "CheckDepInfo": {
                        "dep_info": "debug\\.fingerprint\\home-ce6f4bfb9c7db56a\\dep-lib-home"
                    }
                }
            ],
            "rustflags": [],
            "metadata": 2057089606025779430,
            "config": 0
        }"#;

    #[test]
    #[cfg(all(
        target_arch = "x86_64",
        target_vendor = "pc",
        target_os = "windows",
        target_env = "msvc"
    ))]
    fn fingerprint_hash() {
        let f: super::Fingerprint = serde_json::from_str(FILE).unwrap();
        assert_eq!(f.get_hash(), 15480347459326620707);
    }

    #[test]
    #[cfg(all(
        target_arch = "x86",
        target_vendor = "pc",
        target_os = "windows",
        target_env = "msvc"
    ))]
    fn fingerprint_hash() {
        let f: super::Fingerprint = serde_json::from_str(FILE).unwrap();
        assert_eq!(f.get_hash(), 10502132094877413932);
    }

    #[test]
    #[cfg(all(
        target_arch = "x86_64",
        target_vendor = "unknown",
        target_os = "linux",
        target_env = "gnu"
    ))]
    fn fingerprint_hash() {
        let f: super::Fingerprint = serde_json::from_str(FILE).unwrap();
        assert_eq!(f.get_hash(), 16826414366161678886);
    }
}
