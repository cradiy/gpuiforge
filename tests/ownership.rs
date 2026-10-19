use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "gpuiforge-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("template")).unwrap();
        fs::write(
            root.join("template/settings.txt.tmpl"),
            "{{app.name|xml}} {{var.message|kotlin}}",
        )
        .unwrap();
        fs::write(
            root.join("recipe.json"),
            r#"{
  "abis": [
    "x86_64"
  ],
  "application-id": "dev.example.app",
  "build": [
    {
      "args": [
        "--version"
      ],
      "program": "rustc"
    }
  ],
  "template": "template"
}"#,
        )
        .unwrap();
        fs::write(
            root.join("gpuiforge.json"),
            r#"{
  "app": {
    "name": "A & B"
  },
  "platforms": {
    "android": {
      "recipe": "recipe.json",
      "variables": {
        "message": "$value"
      }
    }
  }
}"#,
        )
        .unwrap();
        Self(root)
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_gpuiforge"))
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) {
        let result = self.run(args);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn bundled_android_generates_without_recipe_or_checkout() {
    let app = Fixture::new();
    fs::remove_file(app.0.join("gpuiforge.json")).unwrap();
    fs::remove_file(app.0.join("recipe.json")).unwrap();
    fs::remove_dir_all(app.0.join("template")).unwrap();
    fs::write(
        app.0.join("Cargo.toml"),
        "[package]\nname = 'fixture-app'\nversion = '0.1.0'\nedition = '2024'\n",
    )
    .unwrap();
    fs::create_dir(app.0.join("src")).unwrap();
    fs::write(app.0.join("src/main.rs"), "fn main() {}").unwrap();
    let binary = app
        .0
        .join(format!("gpuiforge{}", std::env::consts::EXE_SUFFIX));
    fs::copy(env!("CARGO_BIN_EXE_gpuiforge"), &binary).unwrap();
    for args in [
        vec!["init", "--android", "--application-id", "dev.example.app"],
        vec!["generate", "android"],
    ] {
        let result = Command::new(&binary)
            .args(args)
            .current_dir(&app.0)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let generated = app.0.join("target/gpuiforge/android");
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(app.0.join("gpuiforge.json")).unwrap()).unwrap();
    assert_eq!(config["$schema"], "./gpuiforge.schema.json");
    assert!(app.0.join("gpuiforge.schema.json").is_file());
    assert!(
        config["platforms"]["android"]["build"]
            .as_array()
            .is_some_and(|steps| !steps.is_empty())
    );
    assert!(
        config["platforms"]["android"]["run"]
            .as_array()
            .is_some_and(|steps| !steps.is_empty())
    );
    assert!(
        generated
            .join("host/src/main/kotlin/dev/gpui/android/GpuiView.kt")
            .is_file()
    );
    assert!(
        generated
            .join("gradle/wrapper/gradle-wrapper.jar")
            .is_file()
    );
    let activity =
        fs::read_to_string(generated.join("app/src/main/kotlin/dev/gpuiforge/app/MainActivity.kt"))
            .unwrap();
    assert!(activity.contains("fixture_app"));
    assert!(activity.contains("InsetHandling.APPLICATION"));
    let mut config = config;
    config["platforms"]["android"]["inset-handling"] = "host".into();
    config["platforms"]["android"]["staging-root"] = "native prep $cache".into();
    fs::write(
        app.0.join("gpuiforge.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    app.ok(&["sync", "android"]);
    let activity =
        fs::read_to_string(generated.join("app/src/main/kotlin/dev/gpuiforge/app/MainActivity.kt"))
            .unwrap();
    assert!(activity.contains("InsetHandling.HOST"));
    let gradle = fs::read_to_string(generated.join("app/build.gradle.kts")).unwrap();
    let configured_root = dunce::canonicalize(&app.0)
        .unwrap()
        .join("native prep $cache");
    let quoted = serde_json::to_string(&configured_root.display().to_string()).unwrap();
    assert!(gradle.contains(&format!(".getOrElse({})", quoted.replace('$', "\\$"))));
    assert!(gradle.contains("args(\"--staging-root\", gpuiStagingRoot)"));
    let host = generated.join("host/src/main/kotlin/dev/gpui/android/GpuiActivity.kt");
    fs::write(&host, "user-owned host").unwrap();
    assert!(!app.run(&["generate", "android"]).status.success());
    app.ok(&["platform", "eject", "android"]);
    assert_eq!(
        fs::read_to_string(
            app.0
                .join("platforms/android/host/src/main/kotlin/dev/gpui/android/GpuiActivity.kt")
        )
        .unwrap(),
        "user-owned host"
    );
}

#[test]
fn sync_prunes_disabled_modules_and_icons() {
    let app = Fixture::new();
    fs::write(
        app.0.join("Cargo.toml"),
        "[package]\nname = 'fixture-app'\nversion = '0.1.0'\n",
    )
    .unwrap();
    fs::write(app.0.join("icon.xml"), "<vector />").unwrap();
    let config = app.0.join("gpuiforge.json");
    fs::write(
        &config,
        r#"{
  "app": {
    "name": "Example"
  },
  "platforms": {
    "android": {
      "application-id": "dev.example.app",
      "features": [
        "network",
        "sharing",
        "notifications",
        "data-sync",
        "background-media"
      ],
      "cleartext-traffic": true,
      "permissions": ["android.permission.FOREGROUND_SERVICE", "android.permission.FOREGROUND_SERVICE_DATA_SYNC", "android.permission.FOREGROUND_SERVICE_MEDIA_PLAYBACK"],
      "icon": "icon.xml",
      "notification-icon": "icon.xml"
    }
  }
}"#,
    )
    .unwrap();
    let output = app.0.join("target/gpuiforge/android");
    let check = app.run(&["sync", "--check"]);
    assert!(!check.status.success());
    assert!(String::from_utf8_lossy(&check.stderr).contains("create host/"));
    assert!(!output.exists());
    app.ok(&["sync"]);
    let state = output.join(".gpuiforge-generated.json");
    let modified = fs::metadata(&state).unwrap().modified().unwrap();
    app.ok(&["sync", "--check"]);
    assert_eq!(fs::metadata(&state).unwrap().modified().unwrap(), modified);
    assert!(
        fs::read_to_string(output.join("host/build.gradle.kts"))
            .unwrap()
            .contains("org.rustls:rustls-platform-verifier")
    );
    assert!(
        fs::read_to_string(output.join("settings.gradle.kts"))
            .unwrap()
            .contains("maven-archive")
    );
    assert!(
        fs::read_to_string(output.join("app/src/main/AndroidManifest.xml"))
            .unwrap()
            .contains("android.permission.INTERNET")
    );
    assert!(
        fs::read_to_string(output.join("app/src/main/AndroidManifest.xml"))
            .unwrap()
            .contains("android:networkSecurityConfig=\"@xml/network_security_config\"")
    );
    assert!(
        fs::read_to_string(output.join("app/src/main/res/xml/network_security_config.xml"))
            .unwrap()
            .contains("<base-config cleartextTrafficPermitted=\"true\" />")
    );
    let mut restricted: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    restricted["platforms"]["android"]
        .as_object_mut()
        .unwrap()
        .remove("cleartext-traffic");
    restricted["platforms"]["android"]["cleartext-domains"] =
        serde_json::json!(["Cloud.Example.com", "192.168.1.10", "[2001:db8::1]"]);
    fs::write(&config, serde_json::to_vec(&restricted).unwrap()).unwrap();
    app.ok(&["sync"]);
    assert!(
        !output
            .join("app/src/main/res/xml/network_security_config.xml")
            .exists()
    );
    let policy = fs::read_to_string(output.join("app/network-security-config.xml")).unwrap();
    assert!(policy.contains("cleartextTrafficPermitted=\"false\""));
    assert!(policy.contains("includeSubdomains=\"false\">cloud.example.com</domain>"));
    assert!(policy.contains(">192.168.1.10</domain>"));
    assert!(policy.contains(">[2001:db8::1]</domain>"));
    assert!(
        fs::read_to_string(output.join("app/build.gradle.kts"))
            .unwrap()
            .contains("NetworkSecurity")
    );
    for invalid in [
        "http://example.com",
        "example.com:80",
        "*.example.com",
        "192.168.0.0/16",
        "example.com/<xml>",
    ] {
        restricted["platforms"]["android"]["cleartext-domains"] = serde_json::json!([invalid]);
        fs::write(&config, serde_json::to_vec(&restricted).unwrap()).unwrap();
        assert!(!app.run(&["sync"]).status.success());
        assert_eq!(
            fs::read_to_string(output.join("app/network-security-config.xml")).unwrap(),
            policy
        );
    }
    let modified = fs::metadata(&state).unwrap().modified().unwrap();
    let host = output.join("host/src/main/kotlin/dev/gpui/android");
    assert!(host.join("FileStore.kt").exists());
    assert!(host.join("ClipboardImage.kt").exists());
    assert!(host.join("NotificationStore.kt").exists());
    assert!(host.join("DataSyncService.kt").exists());
    assert!(host.join("MediaPlaybackService.kt").exists());
    assert!(host.join("MediaNotification.kt").exists());
    assert!(
        fs::read_to_string(output.join("host/src/main/AndroidManifest.xml"))
            .unwrap()
            .contains("android:foregroundServiceType=\"dataSync\"")
    );
    assert!(!host.join("MediaSession.kt").exists());
    assert!(!host.join("MediaTracks.kt").exists());
    assert!(!host.join("PictureInPictureHost.kt").exists());
    assert!(
        !fs::read_to_string(output.join("app/src/main/AndroidManifest.xml"))
            .unwrap()
            .contains("supportsPictureInPicture")
    );
    assert!(
        !fs::read_to_string(output.join("host/build.gradle.kts"))
            .unwrap()
            .contains("media3")
    );
    assert!(
        output
            .join("app/src/main/res/drawable-nodpi/gpui_notification_icon.xml")
            .exists()
    );
    assert!(
        fs::read_to_string(output.join("app/src/main/AndroidManifest.xml"))
            .unwrap()
            .contains("@drawable/gpui_app_icon")
    );
    fs::write(
        &config,
        r#"{
  "app": {
    "name": "Example"
  },
  "platforms": {
    "android": {
      "application-id": "dev.example.app",
      "features": []
    }
  }
}"#,
    )
    .unwrap();
    let check = app.run(&["sync", "android", "--check"]);
    assert!(!check.status.success());
    assert!(
        String::from_utf8_lossy(&check.stderr)
            .contains("remove host/src/main/kotlin/dev/gpui/android/NotificationStore.kt")
    );
    assert!(host.join("NotificationStore.kt").exists());
    assert_eq!(fs::metadata(&state).unwrap().modified().unwrap(), modified);
    app.ok(&["sync", "android"]);
    app.ok(&["sync", "--check"]);
    for file in [
        "FileStore.kt",
        "ClipboardImage.kt",
        "NotificationStore.kt",
        "ShareIntent.kt",
        "DataSyncService.kt",
        "MediaPlaybackService.kt",
        "MediaNotification.kt",
    ] {
        assert!(!host.join(file).exists());
    }
    assert!(
        !fs::read_to_string(output.join("host/build.gradle.kts"))
            .unwrap()
            .contains("rustls")
    );
    assert!(
        !fs::read_to_string(output.join("settings.gradle.kts"))
            .unwrap()
            .contains("maven-archive")
    );
    assert!(
        !fs::read_to_string(output.join("host/consumer-rules.pro"))
            .unwrap()
            .contains("org.rustls")
    );
    assert!(
        !fs::read_to_string(output.join("app/src/main/AndroidManifest.xml"))
            .unwrap()
            .contains("android.permission.INTERNET")
    );
    assert!(!output.join("app/network-security-config.xml").exists());
    assert!(!output.join("app/network-security.gradle.kts").exists());
    assert!(host.join("ClipboardSnapshot.kt").exists());
    assert!(
        !output
            .join("app/src/main/res/xml/network_security_config.xml")
            .exists()
    );
    assert!(
        !fs::read_to_string(output.join("app/src/main/AndroidManifest.xml"))
            .unwrap()
            .contains("android:networkSecurityConfig")
    );
    let manifest = fs::read_to_string(output.join("host/src/main/AndroidManifest.xml")).unwrap();
    assert!(!manifest.contains("<receiver"));
    assert!(!manifest.contains("<provider"));
    assert!(!manifest.contains("<service"));
    assert!(
        !output
            .join("app/src/main/res/drawable-nodpi/gpui_notification_icon.xml")
            .exists()
    );
    assert!(
        !output
            .join("app/src/main/res/raw/gpui_notification_keep.xml")
            .exists()
    );
    fs::write(host.join("GpuiView.kt"), "user edit").unwrap();
    let check = app.run(&["sync", "--check"]);
    assert!(!check.status.success());
    assert!(String::from_utf8_lossy(&check.stderr).contains("GpuiView.kt"));
    assert!(!app.run(&["sync"]).status.success());
    assert_eq!(
        fs::read_to_string(host.join("GpuiView.kt")).unwrap(),
        "user edit"
    );
}

#[test]
fn invalid_icon_does_not_modify_generated_project() {
    let app = Fixture::new();
    fs::write(
        app.0.join("Cargo.toml"),
        "[package]\nname = 'fixture-app'\nversion = '0.1.0'\n",
    )
    .unwrap();
    let config = app.0.join("gpuiforge.json");
    let mut base = serde_json::json!({"app":{"name":"Example"},"platforms":{"android":{"application-id":"dev.example.app"}}});
    fs::write(&config, base.to_string()).unwrap();
    app.ok(&["sync"]);
    let state = app
        .0
        .join("target/gpuiforge/android/.gpuiforge-generated.json");
    let modified = fs::metadata(&state).unwrap().modified().unwrap();
    base["platforms"]["android"]["icon"] = "missing.png".into();
    fs::write(&config, base.to_string()).unwrap();
    for args in [&["sync", "--check"][..], &["sync"][..]] {
        let result = app.run(args);
        assert!(!result.status.success());
        let message = String::from_utf8_lossy(&result.stderr);
        assert!(message.contains("platforms.android.icon") && message.contains("missing.png"));
        assert_eq!(fs::metadata(&state).unwrap().modified().unwrap(), modified);
    }
}

#[test]
fn published_schemas_match_configuration_types() {
    let app = Fixture::new();
    for (args, expected) in [
        (vec!["schema"], include_str!("../gpuiforge.schema.json")),
        (
            vec!["schema", "--recipe"],
            include_str!("../recipe.schema.json"),
        ),
    ] {
        let result = app.run(&args);
        assert!(result.status.success());
        let actual: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(
            actual,
            serde_json::from_str::<serde_json::Value>(expected).unwrap()
        );
    }
}

#[test]
fn missing_android_tools_stop_before_generating_or_starting_gradle() {
    let app = Fixture::new();
    fs::write(
        app.0.join("Cargo.toml"),
        "[package]\nname = 'fixture-app'\nversion = '0.1.0'\n",
    )
    .unwrap();
    fs::remove_file(app.0.join("gpuiforge.json")).unwrap();
    app.ok(&["init", "--android", "--application-id", "dev.example.app"]);
    let result = Command::new(env!("CARGO_BIN_EXE_gpuiforge"))
        .args(["build", "android"])
        .env("JAVA_HOME", app.0.join("missing-jdk"))
        .env("ANDROID_HOME", app.0.join("missing-sdk"))
        .env("ANDROID_NDK_HOME", app.0.join("missing-ndk"))
        .current_dir(&app.0)
        .output()
        .unwrap();
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("JAVA_HOME")
            && stderr.contains("SDK platform")
            && stderr.contains("NDK tool missing"),
        "{stderr}"
    );
    assert!(!stderr.contains("Running bash"));
    assert!(!app.0.join("target/gpuiforge/android").exists());
}

#[test]
fn edited_generated_sources_survive_eject_and_manual_build() {
    let app = Fixture::new();
    app.ok(&["generate", "android"]);
    let generated = app.0.join("target/gpuiforge/android/settings.txt");
    assert_eq!(
        fs::read_to_string(&generated).unwrap(),
        "A &amp; B \\$value"
    );
    app.ok(&["generate", "android"]);
    fs::write(&generated, "user-owned settings").unwrap();
    assert!(!app.run(&["generate", "android"]).status.success());
    assert_eq!(
        fs::read_to_string(&generated).unwrap(),
        "user-owned settings"
    );
    fs::write(
        app.0.join("target/gpuiforge/android/Custom.kt"),
        "custom service",
    )
    .unwrap();
    app.ok(&["platform", "eject", "android"]);
    let config = fs::read_to_string(app.0.join("gpuiforge.json")).unwrap();
    let config: serde_json::Value = serde_json::from_str(&config).unwrap();
    assert_eq!(config["platforms"]["android"]["management"], "manual");
    assert_eq!(config["app"]["name"], "A & B");
    assert_eq!(
        config["platforms"]["android"]["variables"]["message"],
        "$value"
    );
    let exported = app.0.join("platforms/android/settings.txt");
    assert_eq!(
        fs::read_to_string(&exported).unwrap(),
        "user-owned settings"
    );
    assert!(app.0.join("platforms/android/Custom.kt").is_file());
    fs::write(app.0.join("template/settings.txt.tmpl"), "upstream changes").unwrap();
    app.ok(&["build", "android"]);
    assert_eq!(
        fs::read_to_string(&exported).unwrap(),
        "user-owned settings"
    );
    assert!(!app.run(&["generate", "android"]).status.success());
    assert!(!app.run(&["platform", "eject", "android"]).status.success());
}

#[test]
fn invalid_configuration_cannot_overwrite_existing_project() {
    let app = Fixture::new();
    let generated = app.0.join("target/gpuiforge/android");
    fs::create_dir_all(&generated).unwrap();
    fs::write(generated.join("settings.txt"), "existing project").unwrap();
    assert!(!app.run(&["generate", "android"]).status.success());
    assert_eq!(
        fs::read_to_string(generated.join("settings.txt")).unwrap(),
        "existing project"
    );
    let config = app.0.join("gpuiforge.json");
    let original: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    let mut changed = original.clone();
    changed["platforms"]["android"]["project-dir"] = "../outside".into();
    fs::write(&config, changed.to_string()).unwrap();
    assert!(!app.run(&["generate", "android"]).status.success());
    let mut changed = original.clone();
    changed["platforms"]["android"]["managment"] = "manual".into();
    fs::write(&config, changed.to_string()).unwrap();
    assert!(!app.run(&["generate", "android"]).status.success());
    fs::write(&config, original.to_string()).unwrap();
    assert!(!app.run(&["run"]).status.success());
}

#[cfg(unix)]
#[test]
fn android_run_obeys_configured_steps_and_stops_on_adb_errors() {
    use std::os::unix::fs::PermissionsExt;
    let app = Fixture::new();
    let sdk = app.0.join("sdk/platform-tools");
    fs::create_dir_all(&sdk).unwrap();
    let adb = sdk.join("adb");
    fs::write(
        &adb,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$FORGE_LOG"
case "$*" in
  'devices -l')
    printf 'List of devices attached\nfake-device\tdevice model:Test_Phone\nlocked\tunauthorized\nsleeping\toffline\n'
    if [ "$FORGE_DEVICES" = multiple ]; then printf 'second\tdevice model:Second_Phone\n'; fi ;;
  *getprop*)
    if [ "$FORGE_DEVICES" = incompatible ]; then printf 'armeabi-v7a\n'; else printf 'x86_64\n'; fi ;;
  *install*) if [ "$FORGE_FAILURE" = install ]; then exit 1; fi ;;
  *start*) if [ "$FORGE_FAILURE" = launch ]; then printf 'Error: launch failed\n'; fi ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&adb, fs::Permissions::from_mode(0o755)).unwrap();
    let mut recipe: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(app.0.join("recipe.json")).unwrap()).unwrap();
    recipe["artifact"] = "fixture.apk".into();
    recipe["build"] = serde_json::json!([{
        "program": "sh",
        "args": ["-c", "printf apk > \"$1\"", "sh", "{{project_dir}}/fixture.apk"]
    }]);
    fs::write(app.0.join("recipe.json"), recipe.to_string()).unwrap();
    let mut config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(app.0.join("gpuiforge.json")).unwrap()).unwrap();
    let defaults: serde_json::Value =
        serde_json::from_str(include_str!("../android.json")).unwrap();
    config["platforms"]["android"]["activity"] = "dev.example.MainActivity".into();
    config["platforms"]["android"]["run"] = defaults["run"].clone();
    config["platforms"]["android"]["run"][1]["args"]
        .as_array_mut()
        .unwrap()
        .push("-S".into());
    fs::write(app.0.join("gpuiforge.json"), config.to_string()).unwrap();
    let log = app.0.join("adb.log");
    let listed = Command::new(env!("CARGO_BIN_EXE_gpuiforge"))
        .args(["--config", "missing.json", "devices"])
        .env("ANDROID_HOME", app.0.join("sdk"))
        .env("FORGE_LOG", &log)
        .current_dir(&app.0)
        .output()
        .unwrap();
    assert!(listed.status.success());
    let listing = String::from_utf8_lossy(&listed.stdout);
    assert!(
        listing.contains("Test Phone | x86_64 | ready")
            && listing.contains("unauthorized")
            && listing.contains("offline")
    );
    let calls = fs::read_to_string(&log).unwrap();
    assert!(!calls.contains("-s locked") && !calls.contains("-s sleeping"));
    for failure in ["", "install", "launch"] {
        fs::write(&log, "").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_gpuiforge"))
            .args(["run", "android", "--device", "fake-device"])
            .env("ANDROID_HOME", app.0.join("sdk"))
            .env("FORGE_LOG", &log)
            .env("FORGE_FAILURE", failure)
            .current_dir(&app.0)
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            failure.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let calls = fs::read_to_string(&log).unwrap();
        assert!(calls.contains("-s fake-device install -r "));
        assert_eq!(
            calls.contains("dev.example.app/dev.example.MainActivity -S"),
            failure != "install"
        );
    }
    for (state, device, succeeds, diagnostic) in [
        ("", None, true, ""),
        ("multiple", None, false, "--device SERIAL"),
        ("", Some("locked"), false, "unauthorized"),
        ("", Some("sleeping"), false, "offline"),
        ("incompatible", None, false, "no available Android device"),
    ] {
        fs::write(&log, "").unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_gpuiforge"));
        command.args(["run", "android"]);
        if let Some(device) = device {
            command.args(["--device", device]);
        }
        let output = command
            .env("ANDROID_HOME", app.0.join("sdk"))
            .env("FORGE_LOG", &log)
            .env("FORGE_DEVICES", state)
            .current_dir(&app.0)
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            succeeds,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains(diagnostic));
        let calls = fs::read_to_string(&log).unwrap();
        assert_eq!(calls.contains(" install -r "), succeeds);
    }
}

#[cfg(unix)]
#[test]
fn build_selection_reaches_commands_and_rejects_missing_signing_credentials() {
    let app = Fixture::new();
    let config_path = app.0.join("gpuiforge.json");
    let mut config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    config["platforms"]["android"]["build"] = serde_json::json!([{
        "program": "sh",
        "args": [
            "-c",
            "printf '%s\\n' \"$@\" \"$GPUIFORGE_RUST_PROFILE\" \"$GPUIFORGE_SIGNING_MODE\" \"$GPUIFORGE_SIGNING_ENABLED\" > selection.txt",
            "probe",
            "{{cargo_profile}}", "{{profile}}", "{{variant}}", "{{signing_mode}}", "{{apk_suffix}}"
        ]
    }]);
    fs::write(&config_path, config.to_string()).unwrap();
    let trace = app.0.join("selection.txt");
    for (options, expected) in [
        (vec![], "dev\ndebug\nDebug\ndebug\n\ndev\ndebug\n0\n"),
        (
            vec!["--rust-profile", "release"],
            "release\ndebug\nDebug\ndebug\n\nrelease\ndebug\n0\n",
        ),
        (
            vec!["--release", "--signing", "debug"],
            "release\nrelease\nRelease\ndebug\n\nrelease\ndebug\n0\n",
        ),
        (
            vec!["--release"],
            "release\nrelease\nRelease\nunsigned\n-unsigned\nrelease\nunsigned\n0\n",
        ),
        (
            vec![
                "--release",
                "--rust-profile",
                "inspect-fast",
                "--android-build-type",
                "debug",
            ],
            "inspect-fast\ndebug\nDebug\ndebug\n\ninspect-fast\ndebug\n0\n",
        ),
    ] {
        let mut args = vec!["build", "android"];
        args.extend(options);
        app.ok(&args);
        assert_eq!(fs::read_to_string(&trace).unwrap(), expected);
    }
    config["platforms"]["android"]["rust-profile"] = "inspect-fast".into();
    config["platforms"]["android"]["build-type"] = "release".into();
    config["platforms"]["android"]["signing-mode"] = "debug".into();
    config["platforms"]["android"]["signing"] = serde_json::json!({
        "keystore": "not-created.jks",
        "key-alias": "distribution",
        "store-password-env": "FORGE_TEST_STORE_PASSWORD",
        "key-password-env": "FORGE_TEST_KEY_PASSWORD"
    });
    fs::write(&config_path, config.to_string()).unwrap();
    app.ok(&["build", "android"]);
    assert_eq!(
        fs::read_to_string(&trace).unwrap(),
        "inspect-fast\nrelease\nRelease\ndebug\n\ninspect-fast\ndebug\n0\n"
    );
    app.ok(&["build", "android", "--release"]);
    assert!(
        fs::read_to_string(&trace)
            .unwrap()
            .starts_with("release\nrelease\n")
    );
    for args in [
        vec!["build", "android", "--signing", "configured"],
        vec!["run", "android", "--signing", "unsigned"],
        vec!["build", "android", "--rust-profile", "../invalid"],
    ] {
        fs::remove_file(&trace).unwrap();
        let result = app.run(&args);
        assert!(!result.status.success(), "{args:?}");
        assert!(
            !trace.exists(),
            "invalid options must not execute build steps"
        );
        fs::write(&trace, "").unwrap();
    }
}

#[cfg(unix)]
#[test]
fn native_staging_uses_configured_paths_and_excludes_cargo_outputs() {
    use std::os::unix::fs::PermissionsExt;

    let mut app = Fixture::new();
    app.0 = dunce::canonicalize(&app.0).unwrap();
    fs::write(
        app.0.join("Cargo.toml"),
        "[package]\nname = 'stage-probe'\nversion = '0.1.0'\n[workspace]\n",
    )
    .unwrap();
    fs::create_dir(app.0.join("src")).unwrap();
    fs::write(app.0.join("src/main.rs"), "fn main() {}\n").unwrap();
    let bin = app.0.join("tools/bin");
    let stdlib = app.0.join("tools/stdlib");
    let ndk = app.0.join("tools/ndk");
    let ndk_bin = ndk.join(format!(
        "toolchains/llvm/prebuilt/{}-x86_64/bin",
        if cfg!(target_os = "macos") {
            "darwin"
        } else {
            "linux"
        }
    ));
    for path in [&bin, &stdlib, &ndk_bin] {
        fs::create_dir_all(path).unwrap();
    }
    fs::write(stdlib.join("libstd-probe.rlib"), "").unwrap();
    for (path, source) in [
        (
            bin.join("cargo"),
            r#"#!/bin/sh
case "$1" in
  locate-project) printf '%s\n' "$STAGING_PROBE_WORKSPACE/Cargo.toml" ;;
  rustc)
    shift
    test "$1" = --manifest-path || exit 9
    printf '%s\n' "$2" > "$STAGING_PROBE_WORKSPACE/native-manifest"
    mkdir -p "$CARGO_TARGET_DIR/x86_64-linux-android/debug"
    printf compiled > "$CARGO_TARGET_DIR/x86_64-linux-android/debug/libstage_probe.so" ;;
  *) exit 9 ;;
esac
"#,
        ),
        (
            bin.join("rustc"),
            "#!/bin/sh\nprintf '%s\\n' \"$STAGING_PROBE_WORKSPACE/tools/stdlib\"\n",
        ),
        (
            ndk_bin.join("x86_64-linux-android26-clang"),
            "#!/bin/sh\nexit 0\n",
        ),
        (
            ndk_bin.join("x86_64-linux-android26-clang++"),
            "#!/bin/sh\nexit 0\n",
        ),
        (ndk_bin.join("llvm-ar"), "#!/bin/sh\nexit 0\n"),
    ] {
        fs::write(&path, source).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut path_entries = vec![bin];
    path_entries.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let path = std::env::join_paths(path_entries).unwrap();
    let cargo_target = app.0.join("compiler-cache");
    fs::create_dir(&cargo_target).unwrap();
    fs::write(cargo_target.join("artifact-marker"), "compile output").unwrap();
    let config_path = app.0.join("gpuiforge.json");
    let mut config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    config["platforms"]["android"]["build"] = serde_json::json!([{
        "program": "{{tool_path}}",
        "args": ["build-android-native", "--manifest-path", "{{app_dir}}/Cargo.toml",
            "--output", "{{project_dir}}/jni", "--library", "stage_probe", "--abi", "x86_64"]
    }]);
    for (setting, expected_root) in [
        (None, cargo_target.join("android")),
        (
            Some(serde_json::json!("native workspace")),
            app.0.join("native workspace"),
        ),
        (
            Some(serde_json::json!(app.0.join("absolute staging"))),
            app.0.join("absolute staging"),
        ),
    ] {
        if let Some(setting) = setting {
            config["platforms"]["android"]["staging-root"] = setting;
        }
        fs::write(&config_path, config.to_string()).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_gpuiforge"))
            .args(["build", "android"])
            .current_dir(&app.0)
            .env("PATH", &path)
            .env("ANDROID_NDK_HOME", &ndk)
            .env("CARGO_TARGET_DIR", &cargo_target)
            .env("STAGING_PROBE_WORKSPACE", &app.0)
            .env_remove("GPUIFORGE_STAGING_ROOT")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let prepared = expected_root.join("stage_probe/x86_64");
        assert_eq!(
            fs::read_to_string(app.0.join("native-manifest"))
                .unwrap()
                .trim(),
            prepared.join("Cargo.toml").to_str().unwrap()
        );
        assert!(prepared.join("src/main.rs").is_file());
        assert!(!prepared.join("compiler-cache").exists());
        assert_eq!(
            fs::read(
                app.0
                    .join("target/gpuiforge/android/jni/x86_64/libstage_probe.so")
            )
            .unwrap(),
            b"compiled"
        );
    }
    config["platforms"]["android"]["staging-root"] = "blocked".into();
    fs::write(&config_path, config.to_string()).unwrap();
    fs::remove_file(app.0.join("native-manifest")).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_gpuiforge"))
        .args(["build", "android"])
        .current_dir(&app.0)
        .env("PATH", &path)
        .env("ANDROID_NDK_HOME", &ndk)
        .env(
            "CARGO_TARGET_DIR",
            app.0.join("blocked/stage_probe/x86_64/cache"),
        )
        .env("STAGING_PROBE_WORKSPACE", &app.0)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("staging output must not contain"));
    assert!(!app.0.join("native-manifest").exists());
}

#[test]
fn info_resolves_profiles_and_paths_without_side_effects() {
    let mut app = Fixture::new();
    app.0 = dunce::canonicalize(&app.0).unwrap();
    fs::write(
        app.0.join("Cargo.toml"),
        "[package]\nname = 'info-probe'\nversion = '0.1.0'\n[workspace]\n",
    )
    .unwrap();
    fs::create_dir(app.0.join("src")).unwrap();
    fs::write(app.0.join("src/main.rs"), "fn main() {}\n").unwrap();
    let config_path = app.0.join("gpuiforge.json");
    let mut config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    let android = &mut config["platforms"]["android"];
    android["build"] = serde_json::json!([{"program": "command-must-not-execute"}]);
    android["project-dir"] = "native-project".into();
    android["staging-root"] = "native-sources".into();
    android["variables"]["native_library"] = "info_probe".into();
    android["artifact"] = "outputs/{{profile}}/package{{apk_suffix}}.apk".into();
    android["signing-mode"] = "configured".into();
    android["signing"] = serde_json::json!({
        "keystore": "unused-key.jks", "key-alias": "app-release",
        "store-password-env": "INFO_PROBE_PASSWORD", "key-password-env": "INFO_PROBE_PASSWORD"
    });
    fs::write(&config_path, config.to_string()).unwrap();
    let before = fs::read(&config_path).unwrap();
    let cargo_target = app.0.join("cargo-output");
    let output = Command::new(env!("CARGO_BIN_EXE_gpuiforge"))
        .args(["info", "--release", "--rust-profile", "inspect"])
        .current_dir(&app.0)
        .env("CARGO_TARGET_DIR", &cargo_target)
        .env("GPUIFORGE_STAGING_ROOT", app.0.join("other-staging"))
        .env("INFO_PROBE_PASSWORD", "credential-must-stay-private")
        .env("PATH", app.0.join("missing-tools"))
        .env("JAVA_HOME", app.0.join("missing-jdk"))
        .env("ANDROID_HOME", app.0.join("missing-sdk"))
        .env("ANDROID_NDK_HOME", app.0.join("missing-ndk"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Rust profile: inspect"));
    assert!(text.contains("Android build type: release"));
    assert!(text.contains("Signing mode: configured"));
    assert!(
        text.contains(&format!(
            "Cargo target directory: {}",
            cargo_target.display()
        )),
        "{text}"
    );
    assert!(text.contains(&format!(
        "Prepared project (x86_64): {}",
        app.0.join("native-sources/info_probe/x86_64").display()
    )));
    assert!(text.contains(&format!(
            "Artifact: {}",
            app.0
                .join("native-project/outputs/release/package.apk")
                .display()
        )));
    assert!(!text.contains("credential-must-stay-private"));
    assert!(!cargo_target.exists());
    assert!(!app.0.join("native-sources").exists());
    assert!(!app.0.join("native-project").exists());
    let output = Command::new(env!("CARGO_BIN_EXE_gpuiforge"))
        .args(["info", "android"])
        .current_dir(&app.0)
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains(&format!(
        "Cargo target directory: {}",
        app.0.join("target").display()
    )));
    assert!(!app.0.join("target").exists());
    assert_eq!(fs::read(config_path).unwrap(), before);
}

#[test]
fn clean_previews_and_removes_managed_caches_without_touching_sources() {
    let app = Fixture::new();
    fs::create_dir(app.0.join("template/engine")).unwrap();
    fs::write(
        app.0.join("template/engine/build.gradle.kts.tmpl"),
        "plugins {}\n",
    )
    .unwrap();
    app.ok(&["generate", "android"]);
    let generated = app.0.join("target/gpuiforge/android");
    let source = generated.join("settings.txt");
    fs::write(&source, "preserve native edits").unwrap();
    let caches = ["build", ".gradle", ".kotlin", "engine/build"];
    for cache in caches {
        let directory = generated.join(cache);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("output"), "cached").unwrap();
    }
    let cargo_target = app.0.join("rust-output");
    fs::create_dir(&cargo_target).unwrap();
    fs::write(cargo_target.join("compiled"), "retain").unwrap();
    let before = fs::read(generated.join(".gpuiforge-generated.json")).unwrap();
    let clean = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_gpuiforge"))
            .args(args)
            .current_dir(&app.0)
            .env("CARGO_TARGET_DIR", &cargo_target)
            .output()
            .unwrap()
    };
    let preview = clean(&["clean", "--dry-run"]);
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let listed = String::from_utf8(preview.stdout).unwrap();
    for cache in caches {
        assert!(listed.contains(&generated.join(cache).display().to_string()));
        assert!(generated.join(cache).join("output").exists());
    }
    let output = clean(&["clean", "android"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for cache in caches {
        assert!(!generated.join(cache).exists());
    }
    assert_eq!(fs::read(source).unwrap(), b"preserve native edits");
    assert_eq!(
        fs::read(generated.join(".gpuiforge-generated.json")).unwrap(),
        before
    );
    assert!(generated.join("engine/build.gradle.kts").exists());
    assert_eq!(fs::read(cargo_target.join("compiled")).unwrap(), b"retain");
    let again = clean(&["clean"]);
    assert!(again.status.success());
    assert!(String::from_utf8_lossy(&again.stdout).contains("Nothing to clean"));
}

#[test]
fn clean_staging_follows_root_configuration_and_preserves_other_libraries() {
    let mut app = Fixture::new();
    app.0 = dunce::canonicalize(&app.0).unwrap();
    fs::write(
        app.0.join("Cargo.toml"),
        "[package]\nname = 'cache-audit'\nversion = '0.1.0'\n[workspace]\n",
    )
    .unwrap();
    fs::create_dir(app.0.join("src")).unwrap();
    fs::write(app.0.join("src/main.rs"), "fn main() {}\n").unwrap();
    let config_path = app.0.join("gpuiforge.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    config["platforms"]["android"]["variables"]["native_library"] = "cache_audit".into();
    let target = app.0.join("rust-output");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("compiled"), "retain").unwrap();
    for (setting, root, environment) in [
        (None, target.join("android"), None),
        (
            None,
            app.0.join("environment-prepared"),
            Some("environment-prepared"),
        ),
        (
            Some(serde_json::json!("prepared-sources")),
            app.0.join("prepared-sources"),
            Some("unused-root"),
        ),
        (
            Some(serde_json::json!(app.0.join("absolute-prepared"))),
            app.0.join("absolute-prepared"),
            None,
        ),
    ] {
        if let Some(setting) = setting {
            config["platforms"]["android"]["staging-root"] = setting;
        }
        fs::write(&config_path, config.to_string()).unwrap();
        app.ok(&["generate", "android"]);
        let sibling = root.join("other_library/x86_64");
        fs::create_dir_all(&sibling).unwrap();
        fs::write(sibling.join("keep"), "unrelated snapshot").unwrap();
        for abi in ["arm64-v8a", "x86_64"] {
            app.ok(&[
                "prepare-android",
                "--manifest-path",
                app.0.join("Cargo.toml").to_str().unwrap(),
                "--workspace",
                app.0.to_str().unwrap(),
                "--output",
                root.join("cache_audit").join(abi).to_str().unwrap(),
            ]);
        }
        let clean = |preview: bool| {
            let mut command = Command::new(env!("CARGO_BIN_EXE_gpuiforge"));
            command
                .args(["clean", "--staging"])
                .current_dir(&app.0)
                .env("CARGO_TARGET_DIR", &target)
                .env_remove("GPUIFORGE_STAGING_ROOT");
            if let Some(environment) = environment {
                command.env("GPUIFORGE_STAGING_ROOT", environment);
            }
            if preview {
                command.arg("--dry-run");
            }
            command.output().unwrap()
        };
        let preview = clean(true);
        assert!(
            preview.status.success(),
            "{}",
            String::from_utf8_lossy(&preview.stderr)
        );
        let text = String::from_utf8(preview.stdout).unwrap();
        for abi in ["arm64-v8a", "x86_64"] {
            assert!(text.contains(&root.join("cache_audit").join(abi).display().to_string()));
            assert!(root.join("cache_audit").join(abi).exists());
        }
        let output = clean(false);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!root.join("cache_audit/arm64-v8a").exists());
        assert!(!root.join("cache_audit/x86_64").exists());
        assert!(sibling.join("keep").exists());
        assert_eq!(fs::read(target.join("compiled")).unwrap(), b"retain");
        assert_eq!(
            fs::read(app.0.join("src/main.rs")).unwrap(),
            b"fn main() {}\n"
        );
    }
    let prepared = app.0.join("absolute-prepared/cache_audit/x86_64");
    app.ok(&[
        "prepare-android",
        "--manifest-path",
        app.0.join("Cargo.toml").to_str().unwrap(),
        "--workspace",
        app.0.to_str().unwrap(),
        "--output",
        prepared.to_str().unwrap(),
    ]);
    let manifest_path = prepared.join("Cargo.toml");
    let mut manifest: toml::Value =
        toml::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
    let other_source = app.0.join("neighbor-source.rs");
    fs::write(&other_source, "fn main() {}\n").unwrap();
    manifest["lib"]["path"] = toml::Value::String(other_source.display().to_string());
    fs::write(&manifest_path, toml::to_string(&manifest).unwrap()).unwrap();
    let cache = app.0.join("target/gpuiforge/android/.gradle");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join("keep"), "cached").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_gpuiforge"))
        .args(["clean", "--staging"])
        .current_dir(&app.0)
        .env("CARGO_TARGET_DIR", &target)
        .env_remove("GPUIFORGE_STAGING_ROOT")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("does not belong to this application")
    );
    assert!(cache.join("keep").exists());
    assert!(manifest_path.exists());
    assert!(other_source.exists());
}

#[test]
fn clean_rejects_unowned_projects_and_generated_sources_before_deleting() {
    let app = Fixture::new();
    app.ok(&["generate", "android"]);
    let generated = app.0.join("target/gpuiforge/android");
    fs::create_dir(generated.join(".gradle")).unwrap();
    fs::write(generated.join(".gradle/keep"), "cached").unwrap();
    let state_path = generated.join(".gpuiforge-generated.json");
    let before = fs::read(&state_path).unwrap();
    let mut state: serde_json::Value = serde_json::from_slice(&before).unwrap();
    state["owner"] = serde_json::json!(app.0.join("other-project.json"));
    fs::write(&state_path, state.to_string()).unwrap();
    assert!(!app.run(&["clean"]).status.success());
    assert!(generated.join(".gradle/keep").exists());
    fs::remove_file(&state_path).unwrap();
    assert!(!app.run(&["clean"]).status.success());
    fs::write(&state_path, &before).unwrap();
    let target = generated.join(".gradle/rust-output");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("compiled"), "retain").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_gpuiforge"))
        .arg("clean")
        .current_dir(&app.0)
        .env("CARGO_TARGET_DIR", &target)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cargo target"));
    assert!(target.join("compiled").exists());
    assert!(generated.join(".gradle/keep").exists());
    fs::create_dir(app.0.join("template/build")).unwrap();
    fs::write(
        app.0.join("template/build/source.txt.tmpl"),
        "generated source",
    )
    .unwrap();
    app.ok(&["generate", "android"]);
    let output = app.run(&["clean"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("generated source files"));
    assert!(generated.join("build/source.txt").exists());
    assert!(generated.join(".gradle/keep").exists());
    let config_path = app.0.join("gpuiforge.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    config["platforms"]["android"]["management"] = "manual".into();
    fs::write(config_path, config.to_string()).unwrap();
    let output = app.run(&["clean", "--staging"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("manually managed"));
    assert!(generated.join("build/source.txt").exists());
    assert!(generated.join(".gradle/keep").exists());
}

#[cfg(unix)]
#[test]
fn clean_rejects_symlink_paths_and_never_follows_links_inside_caches() {
    use std::os::unix::fs::symlink;
    let app = Fixture::new();
    app.ok(&["generate", "android"]);
    let generated = app.0.join("target/gpuiforge/android");
    let outside = app.0.join("unrelated");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), "source").unwrap();
    fs::create_dir(generated.join(".gradle")).unwrap();
    fs::write(generated.join(".gradle/cache"), "cached").unwrap();
    symlink(&outside, generated.join("build")).unwrap();
    let output = app.run(&["clean"]);
    assert!(!output.status.success());
    assert!(generated.join(".gradle/cache").exists());
    assert!(outside.join("keep").exists());
    fs::remove_file(generated.join("build")).unwrap();
    fs::create_dir(generated.join("build")).unwrap();
    symlink(&outside, generated.join("build/linked-source")).unwrap();
    app.ok(&["clean"]);
    assert!(!generated.join("build").exists());
    assert_eq!(fs::read(outside.join("keep")).unwrap(), b"source");
}
