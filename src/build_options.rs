use anyhow::{Result, ensure};
use clap::{Args, ValueEnum};
use serde::Deserialize;

use crate::config::Platform;

#[derive(Clone, Copy, Debug, Deserialize, schemars::JsonSchema, ValueEnum, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AndroidBuildType {
    /// Android development build, with debugging enabled by the bundled template.
    Debug,
    /// Android release build. Signing is selected independently.
    Release,
}

impl AndroidBuildType {
    pub fn name(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
        }
    }

    pub fn variant(self) -> &'static str {
        match self {
            Self::Debug => "Debug",
            Self::Release => "Release",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, schemars::JsonSchema, ValueEnum, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SigningMode {
    /// Debug builds use the development key. Release builds use configured credentials, or remain unsigned when none are configured.
    Auto,
    /// Use Android's automatically managed debug key, including for release builds. Intended for device testing, not store publication.
    Debug,
    /// Require the signing object and its password environment variables. Missing credentials fail the build.
    Configured,
    /// Produce an unsigned APK. It cannot be installed by gpuiforge run.
    Unsigned,
}

impl SigningMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Debug => "debug",
            Self::Configured => "configured",
            Self::Unsigned => "unsigned",
        }
    }
}

#[derive(Args, Default)]
pub struct BuildOptions {
    /// Select Rust release and Android release unless explicitly overridden.
    #[arg(long)]
    pub release: bool,
    /// Cargo profile: dev, release, or a profile defined by the application workspace.
    #[arg(long)]
    pub rust_profile: Option<String>,
    /// Android build type, independent of the Rust profile.
    #[arg(long, value_enum)]
    pub android_build_type: Option<AndroidBuildType>,
    /// Android signing policy, independent of the build type.
    #[arg(long, value_enum)]
    pub signing: Option<SigningMode>,
}

pub struct BuildSelection {
    pub rust_profile: String,
    pub android_build_type: AndroidBuildType,
    pub signing: SigningMode,
}

pub fn validate_rust_profile(profile: &str) -> Result<()> {
    ensure!(
        !profile.is_empty()
            && profile
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            && !matches!(profile, "debug" | "test" | "bench" | "build"),
        "invalid Rust profile {profile:?}; use dev, release, or an application-defined profile"
    );
    Ok(())
}

impl BuildOptions {
    pub fn resolve(&self, name: &str, platform: &Platform) -> Result<BuildSelection> {
        ensure!(
            name == "android" || (self.android_build_type.is_none() && self.signing.is_none()),
            "--android-build-type and --signing are Android-only options"
        );
        let rust_profile = self
            .rust_profile
            .as_deref()
            .or(self.release.then_some("release"))
            .or(platform.rust_profile.as_deref())
            .unwrap_or("dev")
            .to_owned();
        validate_rust_profile(&rust_profile)?;
        let android_build_type = self
            .android_build_type
            .or(self.release.then_some(AndroidBuildType::Release))
            .or(platform.build_type)
            .unwrap_or(AndroidBuildType::Debug);
        let signing = match self
            .signing
            .or(platform.signing_mode)
            .unwrap_or(SigningMode::Auto)
        {
            SigningMode::Auto if android_build_type == AndroidBuildType::Debug => {
                SigningMode::Debug
            }
            SigningMode::Auto if platform.signing.is_some() => SigningMode::Configured,
            SigningMode::Auto => SigningMode::Unsigned,
            mode => mode,
        };
        ensure!(
            signing != SigningMode::Configured || platform.signing.is_some(),
            "configured signing requires platforms.android.signing"
        );
        Ok(BuildSelection {
            rust_profile,
            android_build_type,
            signing,
        })
    }
}
