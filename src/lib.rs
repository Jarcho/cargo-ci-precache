use anyhow::{Context, Error, Result};
use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::OsStr,
    fs, io, iter,
    path::{self, Path, PathBuf},
    process::{Command, Stdio},
};

mod meta;
use crate::meta::Metadata;
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
/// Notes: Only items in ~/.cargo/registry/cache and ~/.cargo/git/db are considered.
/// Items in ~/.cargo/registry/src and ~/.cargo/git/checkouts are not deleted.
pub fn clear_cargo_cache(meta: Metadata, delete: &mut dyn FnMut(&Path)) -> Result<()> {
    let cargo_home = home::cargo_home()?;
    let git_db_dir = path!(&cargo_home, "git", "db");
    let registry_cache_dir = path!(&cargo_home, "registry", "cache");

    match git_db_dir.read_dir() {
        Ok(iter) => {
            for e in iter.filter_map(|e| e.ok()) {
                let path = e.path();
                match meta.packages.git.get(path.file_name().unwrap_or_default()) {
                    Some(_) => (),
                    None => delete(&path),
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => {
            return Err(e).with_context(|| format!("error reading dir: {}", git_db_dir.display()))
        }
    }

    match registry_cache_dir.read_dir() {
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
                            if !packages.contains_key(&e.file_name()) {
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
                .with_context(|| format!("error reading dir: {}", registry_cache_dir.display()))
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

fn get_dep_features<'a>(cargo_home: &Path, meta: &'a Metadata, dep: &Path) -> Option<&'a str> {
    if let Some(dep) = dep.strip_prefix(cargo_home).ok() {
        let mut c = dep.components();
        match c.next() {
            Some(path::Component::Normal(x)) if x == "git" => {
                match (c.next(), c.next(), c.next()) {
                    (
                        Some(_), // checkouts
                        Some(path::Component::Normal(repo)),
                        Some(path::Component::Normal(rev)),
                    ) => meta.packages.git.get(repo).map_or(None, |x| {
                        x.get(rev)
                            .and_then(|id| meta.package_features.get(id).map(String::as_str))
                    }),
                    _ => None,
                }
            }
            Some(path::Component::Normal(x)) if x == "registry" => {
                match (c.next(), c.next(), c.next()) {
                    (
                        Some(_), // registry
                        Some(path::Component::Normal(registry)),
                        Some(path::Component::Normal(package)),
                    ) => meta.packages.registry.get(registry).map_or(None, |x| {
                        x.get(package)
                            .and_then(|id| meta.package_features.get(id).map(String::as_str))
                    }),
                    _ => None,
                }
            }
            _ => None,
        }
    } else {
        None
    }
}

fn read_dep_file<'a>(
    path: &Path,
    cargo_home: &Path,
    meta: &'a Metadata,
) -> Result<(String, Option<&'a str>)> {
    let s = fs::read_to_string(&path)
        .with_context(|| format!("error reading file: {}", path.display()))?;

    let dep = read_first_dep(&s)
        .ok_or_else(|| Error::msg(format!("error parsing file: {}", path.display())))?;

    let hash: String = extract_meta_hash(path.file_stem().unwrap_or_default())
        .ok_or_else(|| {
            Error::msg(format!(
                "error extracting metadata hash from: {}",
                path.display()
            ))
        })?
        .into();
    Ok((hash, get_dep_features(cargo_home, meta, &dep)))
}

pub fn clear_target(meta: Metadata, delete: &mut dyn FnMut(&Path)) -> Result<()> {
    let cargo_home = home::cargo_home()?;

    let target_dir = path!(&meta.target_directory, "debug");
    let build_dir = path!(&target_dir, "build");
    let deps_dir = path!(&target_dir, "deps");
    let fingerprint_dir = path!(&target_dir, ".fingerprint");

    match target_dir.read_dir() {
        Ok(iter) => {
            for item in iter {
                let item =
                    item.with_context(|| format!("error reading dir: {}", target_dir.display()))?;
                if item.file_type().map_or(false, |t| t.is_file()) {
                    let path = item.path();
                    if path.file_name().unwrap_or_default() != ".cargo-lock" {
                        delete(&path)
                    }
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(e).with_context(|| format!("error reading dir: {}", target_dir.display()))
        }
    }

    // Get a list of metadata hashes for either local packages, or downloaded packages which are no
    // longer depended on.
    let mut outdated_meta_hashes = HashSet::<String>::new();
    let mut meta_hash_features = HashMap::<String, &str>::new();
    for path in build_dir
        .read_dir()
        .with_context(|| format!("error reading dir: {}", build_dir.display()))?
        .map(|e| -> Result<_> {
            let e = e.with_context(|| format!("error reading dir: {}", build_dir.display()))?;
            Ok(e.path())
        })
        .chain(iter::once(Ok(deps_dir.clone())))
    {
        let path = path?;
        for e in path
            .read_dir()
            .with_context(|| format!("error reading dir: {}", path.display()))?
        {
            let path = e
                .with_context(|| format!("error reading dir: {}", path.display()))?
                .path();
            if path.extension() != Some(OsStr::new("d")) {
                continue;
            }
            let (hash, features) = read_dep_file(&path, &cargo_home, &meta)?;
            match features {
                None => {
                    outdated_meta_hashes.insert(hash);
                }
                Some(f) => {
                    meta_hash_features.insert(hash, f);
                }
            }
        }
    }
    let outdated_meta_hashes = outdated_meta_hashes;
    let meta_hash_features = meta_hash_features;

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
        .filter(|(_, (h, f))| {
            outdated_meta_hashes.contains(h)
                || meta_hash_features
                    .get(h)
                    .map_or(false, |&feat| feat != f.features)
        })
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
