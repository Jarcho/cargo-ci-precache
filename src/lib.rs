use anyhow::{Context, Error, Result};
use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::OsStr,
    fs, io,
    path::{self, Path, PathBuf},
    process::{Command, Stdio},
};

mod meta;
use crate::meta::{Metadata, PackageSet};
mod fingerprint;
use crate::fingerprint::Fingerprint;

macro_rules! path {
    ($($c:expr),*) => {{
        let mut p = PathBuf::new();
        { $(p.push($c));* }
        p
    }};
}

pub struct MetadataCommand(Command);
impl MetadataCommand {
    pub fn new() -> Self {
        let mut c = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        c.arg("metadata")
            .arg("--format-version")
            .arg("1")
            .stdout(Stdio::piped())
            .stdin(Stdio::null());
        Self(c)
    }

    pub fn current_dir<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.0.current_dir(path);
        self
    }

    pub fn manifest_path<P: AsRef<Path>>(&mut self, path: Option<P>) -> &mut Self {
        if let Some(path) = path {
            self.0.arg("--manifest-path").arg(path.as_ref());
        }
        self
    }

    pub fn features<S: AsRef<str>>(&mut self, f: Option<S>) -> &mut Self {
        if let Some(f) = f {
            self.0.arg("--features").arg(f.as_ref());
        }
        self
    }

    pub fn filter_platform<S: AsRef<str>>(&mut self, p: Option<S>) -> &mut Self {
        if let Some(p) = p {
            self.0.arg("--filter-platform").arg(p.as_ref());
        }
        self
    }

    pub fn all_features(&mut self, b: bool) -> &mut Self {
        if b {
            self.0.arg("--all-features");
        }
        self
    }

    pub fn no_default_features(&mut self, b: bool) -> &mut Self {
        if b {
            self.0.arg("--no-default-features");
        }
        self
    }

    pub fn exec(&mut self) -> Result<Metadata> {
        let output = self.0.output().context("error running cargo metadata")?;
        if !output.status.success() {
            return Err(Error::msg(format!(
                "cargo metadata failed: exit code {:?}",
                output.status.code()
            )));
        }

        serde_json::from_slice(&output.stdout).context("error parsing cargo metadata")
    }
}

fn extract_meta_hash(p: &OsStr) -> Option<&str> {
    p.to_str()?.rsplitn(2, "-").next()
}

/// Calls delete for every item in the global cargo cache not referenced by the given metadata.
///
/// Notes: Only items in ~/.cargo/registry/src and ~/.cargo/git/checkouts are considered.
/// Items in ~/.cargo/registry/cache and ~/.cargo/git/ are not deleted.
pub fn clear_cargo_cache(meta: Metadata, delete: &mut dyn FnMut(&Path)) -> Result<()> {
    let cargo_home = home::cargo_home()?;
    let git_checkout_dir = path!(&cargo_home, "git", "checkouts");
    let registry_src_dir = path!(&cargo_home, "registry", "src");

    match git_checkout_dir.read_dir() {
        Ok(iter) => {
            for e in iter.filter_map(|e| e.ok()) {
                let path = e.path();
                match meta.packages.git.get(path.file_name().unwrap_or_default()) {
                    Some(revs) => {
                        for e in e
                            .path()
                            .read_dir()
                            .with_context(|| format!("error reading directory {}", path.display()))?
                            .filter_map(|e| e.ok())
                        {
                            if !revs.contains(&e.file_name()) {
                                delete(&e.path());
                            }
                        }
                    }
                    None => delete(&path),
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("error reading dir: {}", git_checkout_dir.display()))
        }
    }

    match registry_src_dir.read_dir() {
        Ok(iter) => {
            for e in iter.filter_map(|e| e.ok()) {
                let path = e.path();
                match meta
                    .packages
                    .registry
                    .get(path.file_name().unwrap_or_default())
                {
                    Some(packages) => {
                        for e in e
                            .path()
                            .read_dir()
                            .with_context(|| format!("error reading directory {}", path.display()))?
                            .filter_map(|e| e.ok())
                        {
                            if !packages.contains(&e.file_name()) {
                                delete(&e.path());
                            }
                        }
                    }
                    None => delete(&path),
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("error reading dir: {}", registry_src_dir.display()))
        }
    }

    Ok(())
}

// Gets the first dependency, which should be the root source file for the library. e.g. lib.rs
fn read_first_dep(file: &str) -> Option<PathBuf> {
    let line = file.lines().next()?;
    let mut iter = line.splitn(2, ": ");
    iter.next()?;

    // paths are space separated, but may contain escaped spaces.
    let mut path = String::new();
    for s in iter.next()?.trim().split(" ") {
        if s.ends_with(' ') {
            path.push_str(&s[..s.len() - 1]);
            path.push(' ');
        } else {
            path.push_str(s);
            break;
        }
    }
    Some(path.into())
}

fn is_current_dep(cargo_home: &Path, current_deps: &PackageSet, dep: &Path) -> bool {
    if let Some(dep) = dep.strip_prefix(cargo_home).ok() {
        let mut c = dep.components();
        match c.next() {
            Some(path::Component::Normal(x)) if x == "git" => {
                match (c.next(), c.next(), c.next()) {
                    (
                        Some(_), // checkouts
                        Some(path::Component::Normal(repo)),
                        Some(path::Component::Normal(rev)),
                    ) => current_deps
                        .git
                        .get(repo)
                        .map_or(false, |x| x.contains(rev)),
                    _ => false,
                }
            }
            Some(path::Component::Normal(x)) if x == "registry" => {
                match (c.next(), c.next(), c.next()) {
                    (
                        Some(_), // registry
                        Some(path::Component::Normal(registry)),
                        Some(path::Component::Normal(package)),
                    ) => current_deps
                        .registry
                        .get(registry)
                        .map_or(false, |x| x.contains(package)),
                    _ => false,
                }
            }
            _ => false,
        }
    } else {
        false
    }
}

pub fn clear_target(meta: Metadata, delete: &mut dyn FnMut(&Path)) -> Result<()> {
    let cargo_home = home::cargo_home()?;

    let build_dir = path!(&meta.target_directory, "debug", "build");
    let deps_dir = path!(&meta.target_directory, "debug", "deps");
    let fingerprint_dir = path!(&meta.target_directory, "debug", ".fingerprint");

    // Get a list of metadata hashes for either local packages, or downloaded packages which are no
    // longer depended on.
    let mut outdated_meta_hashes = HashSet::<String>::new();
    for e in deps_dir
        .read_dir()
        .with_context(|| format!("error reading dir: {}", deps_dir.display()))?
    {
        let path = e
            .with_context(|| format!("error reading dir: {}", deps_dir.display()))?
            .path();

        if path.extension() != Some(OsStr::new("d")) {
            continue;
        }

        let s = fs::read_to_string(&path)
            .with_context(|| format!("error reading file: {}", path.display()))?;

        let dep = read_first_dep(&s)
            .ok_or_else(|| Error::msg(format!("error parsing file: {}", path.display())))?;

        if !is_current_dep(&cargo_home, &meta.packages, &dep) {
            let hash =
                extract_meta_hash(path.file_stem().unwrap_or_default()).ok_or_else(|| {
                    Error::msg(format!(
                        "error extracting metadata hash from: {}",
                        path.display()
                    ))
                })?;
            outdated_meta_hashes.insert(hash.into());
        }
    }
    let outdated_meta_hashes = outdated_meta_hashes;

    // Collect a list of fingerprints and their associated metadata hash.
    let mut fingerprints = Vec::<(String, Fingerprint)>::new();
    for e in fingerprint_dir
        .read_dir()
        .with_context(|| format!("error reading dir: {}", fingerprint_dir.display()))?
    {
        let unit_path = e
            .with_context(|| format!("error reading dir: {}", fingerprint_dir.display()))?
            .path();
        for e in unit_path
            .read_dir()
            .with_context(|| format!("error reading dir: {}", unit_path.display()))?
        {
            let file_path = e
                .with_context(|| format!("error reading dir: {}", unit_path.display()))?
                .path();
            if file_path.extension() != Some(OsStr::new("json")) {
                continue;
            }
            let s = fs::read(&file_path)
                .with_context(|| format!("error reading file: {}", file_path.display()))?;
            let f = serde_json::from_slice::<Fingerprint>(&s)
                .with_context(|| format!("error parsing file: {}", file_path.display()))?;
            fingerprints.push((
                extract_meta_hash(unit_path.file_stem().unwrap_or_default())
                    .ok_or_else(|| {
                        Error::msg(format!(
                            "error extracting metadata hash from: {}",
                            unit_path.display()
                        ))
                    })?
                    .into(),
                f,
            ));
            break;
        }
    }
    let fingerprints = fingerprints;

    dbg!(&fingerprints);

    // Make a map of fingerprint hashes to the actual fingerprint.
    let fingerprint_map: HashMap<u64, usize> = fingerprints
        .iter()
        .enumerate()
        .map(|(i, (_, f))| (f.get_hash(), i))
        .collect();

    // Make a reverse dependency list for each fingerprint.
    let mut rev_deps: Vec<Vec<usize>> = fingerprints.iter().map(|_| Vec::default()).collect();
    for (i, (_, f)) in fingerprints.iter().enumerate() {
        for dep in f
            .deps
            .iter()
            .filter_map(|d| fingerprint_map.get(&d.fingerprint).cloned())
        {
            rev_deps[dep].push(i);
        }
    }
    let rev_deps = rev_deps;

    // Flag all fingerprints which have a metadata hash we are removing. Then propagate that flag
    // through all the reverse dependencies.
    let mut flagged_deps: Vec<_> = fingerprints.iter().map(|_| false).collect();
    let mut deps_to_flag: Vec<_> = fingerprints
        .iter()
        .enumerate()
        .filter(|(_, (h, _))| outdated_meta_hashes.contains(h))
        .map(|(i, _)| i)
        .collect();

    while let Some(i) = deps_to_flag.pop() {
        if flagged_deps[i] {
            continue;
        }
        flagged_deps[i] = true;
        deps_to_flag.extend_from_slice(&rev_deps[i]);
    }

    // From the list of flagged fingerprints we now have the full list of metadata hashes which
    // have to be removed.
    let meta_hashes_to_remove: HashSet<_> = flagged_deps
        .iter()
        .enumerate()
        .filter(|(_, f)| **f)
        .map(|(i, _)| fingerprints[i].0.as_str())
        .collect();

    let dirs = [&build_dir, &deps_dir, &fingerprint_dir];
    for dir in &dirs {
        for e in dir
            .read_dir()
            .with_context(|| format!("error reading dir: {}", dir.display()))?
        {
            let path = e
                .with_context(|| format!("error reading dir: {}", dir.display()))?
                .path();
            if let Some(hash) = extract_meta_hash(path.file_stem().unwrap_or_default()) {
                if meta_hashes_to_remove.contains(hash) {
                    delete(&path);
                }
            }
        }
    }

    Ok(())
}
