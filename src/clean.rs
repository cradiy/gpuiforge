use crate::{
    config::{Project, relative},
    generate, host, native_build,
};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

fn existing_directory(path: &Path) -> Result<bool> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => ensure!(
                !metadata.file_type().is_symlink(),
                "refusing cleanup through symlink: {}",
                ancestor.display()
            ),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", ancestor.display()));
            }
        }
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_dir(),
                "cleanup path is not a directory: {}",
                path.display()
            );
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

pub fn run(project: &Project, name: &str, staging: bool, dry_run: bool) -> Result<()> {
    let mut paths = generate::build_directories(project, name)?;
    let target = if let Some(target) = std::env::var_os("CARGO_TARGET_DIR") {
        Some(project.root.join(target))
    } else if project.root.join("Cargo.toml").is_file() {
        Some(native_build::target_directory(&native_build::workspace(
            &project.root.join("Cargo.toml"),
        )?))
    } else {
        None
    };
    let directory = project.directory(name)?;
    let mut protected = vec![project.root.clone()];
    if directory.exists() {
        protected.push(host::canonicalize(&directory)?);
    }
    if let Some(target) = target.as_ref().filter(|path| path.exists()) {
        protected.push(host::canonicalize(target)?);
    }
    if project.root.join("src").exists() {
        protected.push(host::canonicalize(&project.root.join("src"))?);
    }
    if staging {
        let platform = project.platform(name)?;
        let library = platform
            .variables
            .get("native_library")
            .context("staging cleanup requires android.variables.native_library")?;
        relative(Path::new(library))?;
        ensure!(
            Path::new(library).components().count() == 1,
            "native library must be a single directory name"
        );
        let target = target
            .as_ref()
            .context("staging cleanup requires a Cargo application or CARGO_TARGET_DIR")?;
        let root = native_build::staging_directory(
            &project.root,
            target,
            platform.staging_root.as_deref(),
        )?;
        for abi in ["arm64-v8a", "x86_64"] {
            let path = root.join(library).join(abi);
            if !existing_directory(&path)? {
                continue;
            }
            let manifest_path = path.join("Cargo.toml");
            ensure!(
                !fs::symlink_metadata(&manifest_path)
                    .with_context(|| format!(
                        "reading staging manifest {}",
                        manifest_path.display()
                    ))?
                    .file_type()
                    .is_symlink(),
                "staging manifest must not be a symlink: {}",
                manifest_path.display()
            );
            let manifest: toml::Value =
                toml::from_str(&fs::read_to_string(&manifest_path).with_context(|| {
                    format!("reading staging manifest {}", manifest_path.display())
                })?)?;
            let source = manifest
                .get("lib")
                .and_then(|lib| lib.get("path"))
                .and_then(toml::Value::as_str)
                .context("staging manifest has no library source path")?;
            ensure!(
                Path::new(source).is_absolute()
                    && host::canonicalize(Path::new(source))?
                        == host::canonicalize(&project.root.join("src/main.rs"))?
                    && manifest.get("lib").and_then(|lib| lib.get("crate-type"))
                        == Some(&toml::Value::Array(vec![toml::Value::String(
                            "cdylib".into()
                        )]))
                    && manifest
                        .get("package")
                        .and_then(|package| package.get("autobins"))
                        .and_then(toml::Value::as_bool)
                        == Some(false),
                "staging directory does not belong to this application: {}",
                path.display()
            );
            paths.push(path);
        }
    }
    let mut existing = Vec::<PathBuf>::new();
    for path in paths {
        if existing_directory(&path)? {
            let canonical = host::canonicalize(&path)?;
            ensure!(
                !protected
                    .iter()
                    .any(|source| source.starts_with(&canonical)),
                "cleanup directory must not contain application sources, the native project or Cargo target: {}",
                canonical.display()
            );
            existing.push(canonical);
        }
    }
    existing.sort();
    let mut selected = Vec::<PathBuf>::new();
    for path in existing {
        if !selected.iter().any(|parent| path.starts_with(parent)) {
            selected.push(path);
        }
    }
    if selected.is_empty() {
        println!("Nothing to clean");
    }
    for path in selected {
        if dry_run {
            println!("Would remove {}", path.display());
        } else {
            fs::remove_dir_all(&path).with_context(|| format!("removing {}", path.display()))?;
            println!("Removed {}", path.display());
        }
    }
    Ok(())
}
