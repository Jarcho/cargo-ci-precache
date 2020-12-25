use anyhow::Context;
use std::{
    collections::{HashMap, HashSet},
    env,
    fmt::Write,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn cargo_build(target: &Path) {
    let res = Command::new(option_env!("CARGO").unwrap_or("cargo"))
        .current_dir(&target)
        .arg("build")
        .output()
        .context("error running cargo build")
        .unwrap()
        .status;
    if !res.success() {
        panic!("error running cargo build, exit code {:?}", res.code());
    }
}

fn gather_items(target_dir: &Path) -> Vec<PathBuf> {
    let meta = cargo_ci_precache::MetadataCommand::new()
        .current_dir(target_dir)
        .exec()
        .unwrap();
    let mut items = Vec::new();
    cargo_ci_precache::clear_target(meta, &mut |path| items.push(PathBuf::from(path))).unwrap();
    items
}

fn split_name_hash(s: &str) -> Option<(String, &str)> {
    let mut iter = s.rsplitn(2, '-');
    let (hash, name) = (iter.next()?, iter.next()?);
    Some((
        name.strip_prefix("lib").unwrap_or(name).replace("-", "_"),
        hash,
    ))
}

fn make_list<T: AsRef<str>>(mut iter: impl Iterator<Item = T>) -> String {
    let mut res = String::new();
    if let Some(s) = iter.next() {
        res.push_str(s.as_ref());
    }
    for s in iter {
        res.push_str(", ");
        res.push_str(s.as_ref())
    }
    res
}

macro_rules! map {
    ($(($key:expr, $value:expr)),*) => {{
        #[allow(unused_mut)]
        let mut map = HashMap::new();
        $(map.insert($key, $value);)*
        map
    }}
}

struct Args {
    /// Name of the test project from the manifest file
    project_name: &'static str,
    /// Subdirectory in the target directory to use for this test.
    target_name: &'static str,
    /// The manifest file for the first build.
    manifest: &'static [u8],
    /// The manifest file for the second build.
    manifest_update: &'static [u8],
    /// The set of crates which are expected to be removed after the second build, and the number
    /// of unique metadata hashes expected for each one.
    ///
    /// Note that because the metadata hash changes with each version of rustc, these values can't
    /// be hardcoded.
    /// Also note that a crate with a build script will generate two different metadata hashes, one
    /// for the build script, and one for the crate.
    expected_removals: HashMap<&'static str, usize>,
}
impl Args {
    fn run_test(&self) {
        // Technically wrong, works for this crate.
        let mut target_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        target_dir.push("target");
        target_dir.push(self.target_name);
        let target_dir = target_dir;
        let manifest_path = target_dir.join("Cargo.toml");
        let src_path = target_dir.join("src");
        let config_path = target_dir.join(".cargo");

        // Make sure the target folder is empty before starting the test.
        rm_rf::ensure_removed(&target_dir).unwrap();

        // Create the directory structure in the target folder.
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(&manifest_path, self.manifest).unwrap();
        fs::create_dir(&src_path).unwrap();
        fs::write(src_path.join("lib.rs"), b"").unwrap();
        fs::create_dir(&config_path).unwrap();
        fs::write(
            config_path.join("config"),
            b"[build]\nincremental = false\n",
        )
        .unwrap();

        // First build. There should be no items to remove other than the local crate.
        cargo_build(&target_dir);
        for item in gather_items(&target_dir) {
            let name = item.file_name().unwrap().to_str().unwrap();
            let name = name.strip_prefix("lib").unwrap_or(name);
            if !name.starts_with(self.project_name) {
                panic!("unexpected crate removal on first build: {}", name);
            }
        }

        // Update the manifest file and rebuild.
        fs::write(&manifest_path, self.manifest_update).unwrap();
        cargo_build(&target_dir);

        let mut unexpected_removals = HashSet::<String>::new();
        let mut removed_crates = HashMap::<_, HashSet<String>>::new();
        for item in gather_items(&target_dir) {
            let file_name = item.file_stem().unwrap().to_str().unwrap();
            let (name, hash) = match split_name_hash(file_name) {
                Some(x) => x,
                None => continue,
            };

            if self.expected_removals.contains_key(name.as_str()) {
                removed_crates.entry(name).or_default().insert(hash.into());
            } else if name != self.project_name {
                unexpected_removals.insert(file_name.into());
            }
        }

        // Build a useful error message.
        let mut msg = String::new();

        // List of all unexpected crates being removed.
        if !unexpected_removals.is_empty() {
            writeln!(
                msg,
                "unexpected crate removals: {}",
                make_list(unexpected_removals.iter())
            )
            .unwrap();
        }

        // List of all crates which are expected to be removed, but there are the wrong number of
        // metadata hashes for that crate being removed.
        msg.extend(self.expected_removals.iter().filter_map(|(&name, &count)| {
            match removed_crates.get(name) {
                Some(hash) if hash.len() == count => None,
                Some(hash) => Some(format!(
                    "Wrong number of versions removed for {}, found {}, expected {}\n",
                    name,
                    hash.len(),
                    count,
                )),
                None => Some(format!(
                    "Wrong number of versions removed for {}, found {}, expected {}\n",
                    name, count, 0,
                )),
            }
        }));

        // If the message is still empty then everything is good.
        if !msg.is_empty() {
            panic!("{}", msg)
        }
    }
}

macro_rules! args {
    ($project:literal => $dir:literal {
        $($dep:literal $count:literal,)*
    }) => {
        Args {
            project_name: $project,
            target_name: $dir,
            manifest: include_bytes!(concat!($project, "/Cargo.toml")),
            manifest_update: include_bytes!(concat!($project, "/Cargo.toml.update")),
            expected_removals: map!($(($dep, $count)),*),
        }
    };
}

#[test]
fn one_dep_update() {
    args!("single_dep" => "single_dep" {
        "cfg_if" 1,
    })
    .run_test()
}

#[test]
fn two_deps_one_update() {
    args!("two_deps" => "two_deps" {
        "cfg_if" 1,
    })
    .run_test()
}

#[test]
fn one_dep_feature_change() {
    args!("feature_change" => "feature_change" {
        "itoa" 1,
    })
    .run_test()
}

#[test]
fn nested_dep_propagate() {
    args!("nested_dep" => "nested_dep" {
        "cfg_if" 1,
        "log" 1,
    })
    .run_test()
}

#[test]
fn build_script_update() {
    args!("build_script" => "build_script" {
        "bitflags" 3,
    })
    .run_test()
}

// Tests for the testing code.
#[test]
#[should_panic]
fn one_dep_update_wrong_count() {
    args!("single_dep" => "single_dep_wrong_count" {
        "cfg_if" 2,
    })
    .run_test()
}

#[test]
#[should_panic]
fn one_dep_update_missing_removal() {
    args!("single_dep" => "single_dep_missing_removal" {
    })
    .run_test()
}
