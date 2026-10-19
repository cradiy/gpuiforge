mod android;
mod android_features;
mod android_tools;
mod build_options;
mod clean;
mod config;
mod devices;
mod execute;
mod generate;
mod host;
mod info;
mod logs;
mod native_build;
mod schema;
mod screenshot;
include!(concat!(env!("OUT_DIR"), "/android_files.rs"));

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use config::Project;
use std::{fs, io::Write, path::PathBuf};

#[derive(Parser)]
#[command(version, about = "Generate, build and run GPUI applications")]
struct Cli {
    #[arg(long, global = true, default_value = "gpuiforge.json")]
    config: PathBuf,
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Print the JSON Schema for application configuration or platform recipes.
    Schema {
        #[arg(long)]
        recipe: bool,
    },
    /// Create configuration for an existing Cargo application.
    Init {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        android_recipe: Option<PathBuf>,
        /// Configure Android using the bundled template.
        #[arg(long, conflicts_with = "android_recipe")]
        android: bool,
        #[arg(long)]
        application_id: Option<String>,
    },
    /// Generate a managed platform project.
    Generate { platform: String },
    /// Synchronize managed native sources from configuration without building.
    Sync {
        #[arg(default_value = "android")]
        platform: String,
        /// Fail if synchronization is needed, without modifying any files.
        #[arg(long)]
        check: bool,
    },
    /// Build a configured platform (prompts when omitted).
    Build {
        platform: Option<String>,
        #[command(flatten)]
        options: build_options::BuildOptions,
        /// Build one enabled Android ABI instead of all configured ABIs.
        #[arg(long, value_parser = ["arm64-v8a", "x86_64"])]
        abi: Option<String>,
    },
    /// Build and run on a platform and device (prompts when omitted).
    Run {
        platform: Option<String>,
        #[arg(long)]
        device: Option<String>,
        /// Follow Android application logs after the configured run steps finish.
        #[arg(long)]
        logs: bool,
        #[command(flatten)]
        options: build_options::BuildOptions,
    },
    /// Show resolved configuration and output paths without building.
    Info {
        #[arg(default_value = "android")]
        platform: String,
        #[command(flatten)]
        options: build_options::BuildOptions,
    },
    /// Remove managed Android build caches, optionally including Rust staging.
    Clean {
        #[arg(default_value = "android", value_parser = ["android"])]
        platform: String,
        /// Also remove prepared Rust projects for the current native library.
        #[arg(long)]
        staging: bool,
        /// List directories that would be removed without changing files.
        #[arg(long)]
        dry_run: bool,
    },
    /// Stop and relaunch the installed Android application without building.
    Restart {
        #[arg(long)]
        device: Option<String>,
        #[arg(long)]
        logs: bool,
    },
    /// Start the installed Android application without building or installing.
    Launch {
        #[arg(long)]
        device: Option<String>,
        /// Follow application logs after a successful launch.
        #[arg(long)]
        logs: bool,
    },
    /// Check local build tools and configured project paths.
    Doctor,
    /// List Android devices, architectures and connection status.
    Devices,
    /// Save the selected Android device's screen as a local PNG.
    Screenshot {
        /// Destination file; defaults to screenshot-<timestamp>.png in the current directory.
        output: Option<PathBuf>,
        #[arg(long)]
        device: Option<String>,
    },
    /// Follow the configured Android application's logs (Ctrl+C to stop).
    Logs {
        #[command(flatten)]
        options: logs::LogOptions,
    },
    #[command(hide = true)]
    PrepareAndroid {
        #[arg(long)]
        manifest_path: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    #[command(hide = true)]
    BuildAndroidNative {
        #[arg(long)]
        manifest_path: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        library: String,
        #[arg(long, value_parser = ["arm64-v8a", "x86_64"])]
        abi: String,
        #[arg(long, default_value = "dev")]
        profile: String,
        #[arg(long, default_value = "26")]
        min_sdk: u32,
        #[arg(long)]
        staging_root: Option<PathBuf>,
    },
    #[command(hide = true)]
    Gradle {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<std::ffi::OsString>,
    },
    /// Manage native project ownership.
    Platform {
        #[command(subcommand)]
        command: PlatformAction,
    },
}

#[derive(Subcommand)]
enum PlatformAction {
    /// Export generated sources and switch configuration to manual management.
    Eject {
        platform: String,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    if let Action::Gradle { args } = cli.command {
        return native_build::gradle(&args);
    }
    if let Action::BuildAndroidNative {
        manifest_path,
        output,
        library,
        abi,
        profile,
        min_sdk,
        staging_root,
    } = cli.command
    {
        return native_build::build(
            &manifest_path,
            &output,
            &library,
            &abi,
            &profile,
            min_sdk,
            staging_root.as_deref(),
        );
    }
    if matches!(cli.command, Action::Devices) {
        return devices::print();
    }
    if let Action::Screenshot { output, device } = cli.command {
        println!(
            "{}",
            screenshot::save(device.as_deref(), output.as_deref())?.display()
        );
        return Ok(());
    }
    if let Action::PrepareAndroid {
        manifest_path,
        workspace,
        output,
    } = cli.command
    {
        return android::prepare(&manifest_path, &workspace, &output, None);
    }
    if let Action::Init {
        name,
        android_recipe,
        android,
        application_id,
    } = cli.command
    {
        return init(&cli.config, name, android_recipe, android, application_id);
    }
    if let Action::Schema { recipe } = cli.command {
        println!("{}", schema::text(recipe)?);
        return Ok(());
    }
    let project = Project::load(&cli.config)?;
    match cli.command {
        Action::Sync { platform, check } => {
            let directory = if check {
                generate::check(&project, &platform)?
            } else {
                generate::generate(&project, &platform)?
            };
            if check {
                println!("{platform} project is up to date");
            } else {
                println!("Synchronized {platform} project");
            }
            println!("  Directory: {}", directory.display());
        }
        Action::Generate { platform } => {
            println!("{}", generate::generate(&project, &platform)?.display())
        }
        Action::Build {
            platform,
            options,
            abi,
        } => {
            let platform = platform.or_else(|| abi.as_ref().map(|_| "android".into()));
            let name = execute::platform(&project, platform)?;
            execute::build(&project, &name, &options, abi.as_deref())?;
        }
        Action::Run {
            platform,
            device,
            logs,
            options,
        } => {
            let platform =
                platform.or_else(|| (device.is_some() || logs).then(|| "android".into()));
            let name = execute::platform(&project, platform)?;
            execute::run(&project, &name, &options, device.as_deref(), logs)?;
        }
        Action::Info { platform, options } => info::print(&project, &platform, &options)?,
        Action::Clean {
            platform,
            staging,
            dry_run,
        } => clean::run(&project, &platform, staging, dry_run)?,
        Action::Restart { device, logs } => execute::restart(&project, device.as_deref(), logs)?,
        Action::Launch { device, logs } => execute::launch(&project, device.as_deref(), logs)?,
        Action::Doctor => execute::doctor(&project)?,
        Action::Logs { options } => logs::run(&project, &options)?,
        Action::Platform {
            command: PlatformAction::Eject { platform, output },
        } => {
            let destination =
                output.unwrap_or_else(|| PathBuf::from(format!("platforms/{platform}")));
            println!(
                "{}",
                generate::eject(&project, &platform, &destination)?.display()
            );
        }
        Action::Schema { .. }
        | Action::Init { .. }
        | Action::PrepareAndroid { .. }
        | Action::BuildAndroidNative { .. }
        | Action::Gradle { .. }
        | Action::Devices => unreachable!(),
        Action::Screenshot { .. } => unreachable!(),
    }
    Ok(())
}

fn init(
    path: &std::path::Path,
    name: Option<String>,
    recipe: Option<PathBuf>,
    android: bool,
    id: Option<String>,
) -> Result<()> {
    ensure!(
        !path.exists(),
        "configuration already exists: {}",
        path.display()
    );
    let root = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let manifest: toml::Value = toml::from_str(
        &fs::read_to_string(root.join("Cargo.toml"))
            .context("init requires an existing Cargo application")?,
    )?;
    let package = manifest
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .context("Cargo.toml must describe an application package")?;
    let mut doc = serde_json::json!({
        "$schema": "./gpuiforge.schema.json",
        "app": {"name": name.unwrap_or_else(|| package.to_owned())},
        "platforms": {"desktop": {}}
    });
    for (kind, command) in [("build", "build"), ("run", "run")] {
        doc["platforms"]["desktop"][kind] = serde_json::json!([{
            "program": "cargo",
            "args": [command, "-p", package, "--profile", "{{cargo_profile}}"]
        }]);
    }
    let android = android || recipe.is_some();
    ensure!(
        android || id.is_none(),
        "--application-id requires --android or --android-recipe"
    );
    if android {
        doc["platforms"]["android"] = serde_json::json!({});
        let id = id.context("--application-id is required for Android")?;
        let defaults: serde_json::Value = if let Some(recipe) = &recipe {
            fs::read_to_string(recipe)?.parse()?
        } else {
            include_str!("../android.json").parse()?
        };
        if let Some(recipe) = recipe {
            doc["platforms"]["android"]["recipe"] = serde_json::Value::String(
                recipe
                    .canonicalize()?
                    .to_str()
                    .context("recipe path must be UTF-8")?
                    .to_owned(),
            );
        }
        doc["platforms"]["android"]["application-id"] = serde_json::Value::String(id);
        for field in [
            "abis",
            "features",
            "inset-handling",
            "permissions",
            "build",
            "run",
        ] {
            if let Some(value) = defaults.get(field) {
                doc["platforms"]["android"][field] = value.clone();
            }
        }
    }
    let schema_path = root.join("gpuiforge.schema.json");
    let schema_text = schema::text(false)? + "\n";
    if schema_path.exists() {
        ensure!(
            fs::read_to_string(&schema_path)? == schema_text,
            "schema already exists with different contents: {}",
            schema_path.display()
        );
    } else {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&schema_path)?
            .write_all(schema_text.as_bytes())?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all((serde_json::to_string_pretty(&doc)? + "\n").as_bytes())?;
    println!("{}", path.display());
    Ok(())
}
