use anyhow::{Context, Error, Result};
use clap::Clap;
use ci_precache::MetadataCommand;
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Clap)]
pub enum Mode {
    /// Clears the global cargo cache
    CargoCache,
    /// Clears the projects target directory
    Target,
}

#[derive(Clap)]
#[clap(version = "1.0", author = "Jason Newcomb <jsnewcomb@pm.me>")]
struct Args {
    /// Path to Cargo.toml
    #[clap(long, parse(from_os_str))]
    pub manifest_path: Option<PathBuf>,

    /// Comma separated list of features to activate
    #[clap(long)]
    pub features: Option<String>,

    /// Only include dependencies matching the given target-triple
    #[clap(long)]
    pub filter_platform: Option<String>,

    /// Activate all available features
    #[clap(long)]
    pub all_features: bool,

    /// Do not activate the `default` feature
    #[clap(long)]
    pub no_default_features: bool,

    /// Do not make any changes, but show a list of files to be deleted
    #[clap(long)]
    pub dry_run: bool,

    /// Temporary directory to move directories into, will default to $TEMP.
    #[clap(long)]
    pub temp: Option<PathBuf>,

    /// Whether to clear the global cargo cache, or the projects target directory.
    #[clap(arg_enum)]
    pub mode: Mode,
}

fn remove_item(path: &Path, counter: &mut u32, temp: &Path) -> io::Result<()> {
    let meta = match path.symlink_metadata() {
        Ok(m) => m,
        // If the file was not found then it's removed.
        // This also shouldn't happen.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };

    if !meta.is_dir() {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),

            // Read-only files on windows will fail with PermissionDenied.
            // Remove the read-only flag if that happens, and try again.
            #[cfg(windows)]
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                let mut perm = meta.permissions();
                perm.set_readonly(false);
                fs::set_permissions(path, perm)?;
                fs::remove_file(path)
            }
            Err(e) => Err(e),
        }
    } else {
        // Just need a random unique name for the directory.
        // Incrementing counter it is.
        let target_name = counter.to_string();
        *counter += 1;
        let target_dir = temp.join(target_name);

        // Can only move a directory to another empty directory on unix.
        #[cfg(unix)]
        {
            fs::create_dir(&target_dir)?;
        }
        fs::rename(path, &target_dir)
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    let meta = MetadataCommand::new()
        .manifest_path(args.manifest_path)
        .features(args.features)
        .filter_platform(args.filter_platform)
        .all_features(args.all_features)
        .no_default_features(args.no_default_features)
        .exec()?;

    let mut delete: Box<dyn FnMut(&Path)> = if args.dry_run {
        Box::new(|p| println!("{}", p.display()))
    } else {
        let mut temp = args
            .temp
            .or_else(|| env::var_os("TEMP").map(PathBuf::from))
            .ok_or_else(|| Error::msg("no temp dir"))?;

        // Directories moved into the temp folder are named only from an incrementing counter to
        // avoid name collisions on a single run, but this would mean multiple runs would certainly
        // have a collision. Working in a directory named after the current time should avoid this.
        temp.push(
            match SystemTime::UNIX_EPOCH.elapsed() {
                Ok(x) => x,
                Err(e) => e.duration(),
            }
            .as_nanos()
            .to_string(),
        );

        fs::create_dir_all(&temp)
            .with_context(|| format!("error creating temp dir: {}", temp.display()))?;

        let mut counter = 0u32;

        Box::new(move |path| match remove_item(path, &mut counter, &temp) {
            Ok(()) => (),
            Err(e) => {
                eprintln!("error removing {}\n{}", path.display(), e);
            }
        })
    };

    match args.mode {
        Mode::CargoCache => ci_precache::clear_cargo_cache(meta, &mut delete),
        Mode::Target => ci_precache::clear_target(meta, &mut delete),
    }
}
