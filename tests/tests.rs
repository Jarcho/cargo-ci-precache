use anyhow::Context;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn cargo() -> Command {
    Command::new(option_env!("CARGO").unwrap_or("cargo"))
}

fn assert_success(name: &str, c: &mut Command) {
    let res = c
        .output()
        .with_context(|| format!("error running {}", name))
        .unwrap()
        .status;
    if !res.success() {
        panic!("error running {}: exit code {:?}", name, res.code());
    }
}

fn gather_items(target_dir: &Path) -> Vec<PathBuf> {
    let meta = ci_precache::MetadataCommand::new()
        .current_dir(target_dir)
        .exec()
        .unwrap();
    let mut items = Vec::new();
    ci_precache::clear_target(meta, &mut |path| items.push(PathBuf::from(path))).unwrap();
    items
}

#[test]
fn update_nested_dep() {
    let manifest = include_str!("nested_dep/Cargo.toml");
    let lockfile_update = include_str!("nested_dep/Cargo.lock.update");
    let lockfile = include_str!("nested_dep/Cargo.lock");
    let config = "[build]\nincremental = false\n";

    let target_dir = env::current_dir()
        .unwrap()
        .join("target")
        .join("nested_dep");

    // Ensure target dir is cleared.
    rm_rf::ensure_removed(&target_dir).unwrap();

    // Create test project.
    fs::create_dir(&target_dir).unwrap();
    fs::create_dir(target_dir.join("src")).unwrap();
    fs::create_dir(target_dir.join(".cargo")).unwrap();
    fs::write(target_dir.join("Cargo.toml"), manifest).unwrap();
    fs::write(target_dir.join("Cargo.lock"), lockfile).unwrap();
    fs::write(target_dir.join("src").join("lib.rs"), "").unwrap();
    fs::write(target_dir.join(".cargo").join("config"), config).unwrap();

    // Fresh build should have no deps to clear.
    assert_success("cargo build", cargo().current_dir(&target_dir).arg("build"));
    let items = gather_items(&target_dir);
    for item in &items {
        let name = item.file_name().unwrap().to_str().unwrap();
        if !(name.starts_with("nested_dep") || name.starts_with("libnested_dep")) {
            panic!("unexpected dep {}", item.display());
        }
    }

    // Update dependecy and rebuild. Should have old dependecies.
    fs::write(target_dir.join("Cargo.lock"), lockfile_update).unwrap();
    assert_success("cargo build", cargo().current_dir(&target_dir).arg("build"));
    let items = gather_items(&target_dir);

    let mut nested_dep = false;
    let mut cfg_if = false;
    let mut stacker = false;
    let mut rm_rf = false;

    for item in &items {
        let name = item.file_name().unwrap().to_str().unwrap();
        if name.starts_with("cfg-if")
            || name.starts_with("libcfg-if")
            || name.starts_with("cfg_if")
            || name.starts_with("libcfg_if")
        {
            cfg_if = true;
        } else if name.starts_with("stacker") || name.starts_with("libstacker") {
            stacker = true;
        } else if name.starts_with("rm_rf") || name.starts_with("librm_rf") {
            rm_rf = true;
        } else if name.starts_with("nested_dep") || name.starts_with("libnested_dep") {
            nested_dep = true;
        } else {
            panic!("unexpected dep {}", item.display());
        }
    }
    assert!(nested_dep);
    assert!(cfg_if);
    assert!(stacker);
    assert!(rm_rf);
}

