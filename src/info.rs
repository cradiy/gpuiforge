use crate::{
    build_options::BuildOptions,
    config::{Management, Project, expand},
    native_build,
};
use anyhow::Result;

pub fn print(project: &Project, name: &str, options: &BuildOptions) -> Result<()> {
    let platform = project.platform(name)?;
    let selection = options.resolve(name, platform)?;
    let vars = project.variables(name, options, None)?;
    let directory = project.directory(name)?;
    println!("Application: {}", project.name);
    println!("Platform: {name}");
    println!("Config: {}", project.config_path.display());
    println!("Application directory: {}", project.root.display());
    println!("Project directory: {}", directory.display());
    println!(
        "Management: {}",
        match platform.management {
            Management::Managed => "managed",
            Management::Manual => "manual",
        }
    );
    println!("Rust profile: {}", selection.rust_profile);
    if name == "android" {
        println!(
            "Application ID: {}",
            platform
                .application_id
                .as_deref()
                .unwrap_or("(not configured)")
        );
        println!(
            "Activity: {}",
            platform.activity.as_deref().unwrap_or("(not configured)")
        );
        println!("ABIs: {}", platform.abis.join(", "));
        println!(
            "Android build type: {}",
            selection.android_build_type.name()
        );
        println!("Signing mode: {}", selection.signing.name());
        let library = platform.variables.get("native_library");
        println!(
            "Native library: {}",
            library.map(String::as_str).unwrap_or("(not configured)")
        );
        let target = if let Some(target) = std::env::var_os("CARGO_TARGET_DIR") {
            Ok(project.root.join(target))
        } else {
            native_build::workspace(&project.root.join("Cargo.toml"))
                .map(|workspace| native_build::target_directory(&workspace))
        };
        match target {
            Ok(target) => {
                println!("Cargo target directory: {}", target.display());
                let staging = native_build::staging_directory(
                    &project.root,
                    &target,
                    platform.staging_root.as_deref(),
                )?;
                println!("Staging root: {}", staging.display());
                if let Some(library) = library {
                    for abi in &platform.abis {
                        println!(
                            "Prepared project ({abi}): {}",
                            staging.join(library).join(abi).display()
                        );
                    }
                }
            }
            Err(error) => {
                println!("Cargo target directory: unresolved ({error:#})");
                if let Some(staging) = platform.staging_root.as_ref() {
                    println!("Staging root: {}", staging.display());
                } else if let Some(staging) = std::env::var_os("GPUIFORGE_STAGING_ROOT") {
                    println!("Staging root: {}", project.root.join(staging).display());
                } else {
                    println!("Staging root: unresolved (Cargo target directory unavailable)");
                }
            }
        }
    }
    if let Some(artifact) = &platform.artifact {
        let artifact = directory.join(expand(artifact, &vars)?);
        println!("Artifact: {}", artifact.display());
        println!(
            "Artifact status: {}",
            if artifact.is_file() {
                "present"
            } else {
                "missing"
            }
        );
    } else {
        println!("Artifact: (not configured)");
    }
    Ok(())
}
