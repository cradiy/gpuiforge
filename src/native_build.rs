use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn workspace(manifest: &Path) -> Result<PathBuf> {
    let located = Command::new("cargo")
        .args(["locate-project", "--workspace", "--manifest-path"])
        .arg(manifest)
        .args(["--message-format", "plain"])
        .current_dir(manifest.parent().context("application directory missing")?)
        .output()
        .context("locating Cargo workspace")?;
    ensure!(
        located.status.success(),
        "cargo locate-project failed: {}",
        String::from_utf8_lossy(&located.stderr)
    );
    let workspace_manifest = String::from_utf8(located.stdout)?;
    Ok(Path::new(workspace_manifest.trim())
        .parent()
        .context("workspace directory missing")?
        .to_path_buf())
}

pub fn target_directory(workspace: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join("target"))
}

pub fn staging_directory(package: &Path, target: &Path, root: Option<&Path>) -> Result<PathBuf> {
    let root = root
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("GPUIFORGE_STAGING_ROOT").map(PathBuf::from))
        .unwrap_or_else(|| target.join("android"));
    ensure!(
        !root.as_os_str().is_empty(),
        "staging root must not be empty"
    );
    Ok(package.join(root))
}

pub fn gradle(args: &[OsString]) -> Result<()> {
    let java = crate::android_tools::java_development_kit()?
        .join("bin")
        .join(crate::host::executable("java"));
    let wrapper = crate::host::canonicalize(Path::new("gradle/wrapper/gradle-wrapper.jar"))?;
    let status = Command::new(java)
        .args(["-Xmx64m", "-Xms64m", "-Dorg.gradle.appname=gradlew", "-jar"])
        .arg(wrapper)
        .args(args)
        .status()
        .context("starting Gradle wrapper")?;
    ensure!(status.success(), "Gradle failed: {status}");
    Ok(())
}

pub fn build(
    manifest: &Path,
    output: &Path,
    library: &str,
    abi: &str,
    profile: &str,
    api: u32,
    staging_root: Option<&Path>,
) -> Result<()> {
    ensure!(
        library.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && library
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "invalid native library name: {library}"
    );
    ensure!(api >= 26, "Android minimum SDK must be at least 26");
    let profile = if profile == "debug" { "dev" } else { profile };
    crate::build_options::validate_rust_profile(profile)?;
    let target = crate::android_tools::target(abi)?;
    let manifest = crate::host::canonicalize(manifest)?;
    let package = manifest.parent().context("application directory missing")?;
    let workspace = workspace(&manifest)?;
    let ndk = crate::android_tools::ndk(package, target, api)?;
    let tools = crate::host::NdkTools::new(Path::new(&ndk), target, api)?;
    crate::android_tools::rust_target(package, target)?;
    let target_dir = target_directory(&workspace);
    fs::create_dir_all(&target_dir)?;
    let target_dir = crate::host::canonicalize(&target_dir)?;
    let prepared = staging_directory(package, &target_dir, staging_root)?
        .join(library)
        .join(abi);
    crate::android::prepare(&manifest, &workspace, &prepared, Some(&target_dir))?;
    let key = target.replace('-', "_");
    let mut cargo = Command::new("cargo");
    cargo
        .args(["rustc", "--manifest-path"])
        .arg(prepared.join("Cargo.toml"))
        .args(["--lib", "--target", target])
        .current_dir(package)
        .env("CARGO_TARGET_DIR", &target_dir)
        .env(
            format!("CARGO_TARGET_{}_LINKER", key.to_ascii_uppercase()),
            &tools.cc,
        )
        .env(format!("CC_{key}"), &tools.cc)
        .env(format!("CXX_{key}"), &tools.cxx)
        .env(format!("AR_{key}"), &tools.ar);
    cargo.args(["--profile", profile]);
    let status = cargo
        .args([
            "--",
            "-C",
            "link-arg=-Wl,-z,max-page-size=16384",
            "-C",
            "link-arg=-Wl,-z,common-page-size=16384",
        ])
        .status()
        .context("building Android native library")?;
    ensure!(status.success(), "Android Cargo build failed: {status}");
    let library = format!("lib{library}.so");
    let destination = output.join(abi);
    fs::create_dir_all(&destination)?;
    fs::copy(
        target_dir
            .join(target)
            .join(if profile == "dev" { "debug" } else { profile })
            .join(&library),
        destination.join(&library),
    )
    .with_context(|| format!("packaging {library} for {abi}"))?;
    Ok(())
}
