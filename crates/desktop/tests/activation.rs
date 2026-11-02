// SPDX-License-Identifier: GPL-3.0-or-later
//! Observe the real child command boundary in an isolated subprocess.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use ferroseek_desktop::{apps, launch};

#[test]
fn launch_context_is_fresh_for_apps_and_declared_actions() {
    let case = std::env::var("FERROSEEK_TEST_CONTEXT_CASE");
    let Ok(case) = case else {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let work = root.join("working directory");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(root.join("runtime")).unwrap();
        let capture = root.join("capture command");
        fs::write(&capture, "#!/bin/sh\nset -eu\nprintf '%s\\n' \"${XDG_ACTIVATION_TOKEN-<unset>}\" \"${DESKTOP_STARTUP_ID-<unset>}\" \"$PWD\" \"$@\" > \"$FERROSEEK_TEST_CONTEXT_OUTPUT.tmp\"\nmv \"$FERROSEEK_TEST_CONTEXT_OUTPUT.tmp\" \"$FERROSEEK_TEST_CONTEXT_OUTPUT\"\n").unwrap();
        fs::set_permissions(&capture, fs::Permissions::from_mode(0o700)).unwrap();
        let desktop = root.join("org.ferroseek.Context.desktop");
        fs::write(&desktop, format!(
            "[Desktop Entry]\nType=Application\nName=Context Probe\nExec=\"{}\" normal \"two words\" \"semi;literal\" %c\nPath={}\nActions=new;\n[Desktop Action new]\nName=New window\nExec=\"{}\" action \"two words\" %c\n",
            capture.display(), work.display(), capture.display()
        )).unwrap();
        for case in [
            "app-none",
            "app-fresh",
            "app-empty",
            "action-none",
            "action-fresh",
            "action-empty",
            "missing-app",
            "missing-action",
            "invalid-action",
        ] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "launch_context_is_fresh_for_apps_and_declared_actions", "--nocapture"])
                .env("FERROSEEK_TEST_CONTEXT_CASE", case)
                .env("FERROSEEK_TEST_CONTEXT_DESKTOP", &desktop)
                .env("FERROSEEK_TEST_CONTEXT_OUTPUT", root.join(case))
                .env("XDG_ACTIVATION_TOKEN", "inherited-token")
                .env("DESKTOP_STARTUP_ID", "inherited-startup")
                .env("HOME", root)
                .env("PATH", "/usr/bin:/bin")
                .env("XDG_CONFIG_HOME", root.join("config"))
                .env("XDG_DATA_HOME", root.join("data"))
                .env("XDG_STATE_HOME", root.join("state"))
                .env("XDG_CACHE_HOME", root.join("cache"))
                .env("XDG_RUNTIME_DIR", root.join("runtime"))
                .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent-ferroseek-test-bus")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "launch case {case}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    };

    let desktop = PathBuf::from(std::env::var_os("FERROSEEK_TEST_CONTEXT_DESKTOP").unwrap());
    let text = fs::read_to_string(&desktop).unwrap();
    let text = if case.starts_with("missing-") { text.replace("capture command", "unavailable command") } else { text };
    let app = apps::parse("org.ferroseek.Context.desktop", &desktop, &text, &[], &[]).unwrap();
    let output = PathBuf::from(std::env::var_os("FERROSEEK_TEST_CONTEXT_OUTPUT").unwrap());
    let result = match case.as_str() {
        "app-none" => launch::launch_with_token(&app, None),
        "app-fresh" => launch::launch_with_token(&app, Some("fresh-app")),
        "app-empty" => launch::launch_with_token(&app, Some("")),
        "action-none" => launch::launch_action_with_token(&app, 0, None),
        "action-fresh" => launch::launch_action_with_token(&app, 0, Some("fresh-action")),
        "action-empty" => launch::launch_action_with_token(&app, 0, Some("")),
        "missing-app" => launch::launch_with_token(&app, Some("fresh-app")),
        "missing-action" => launch::launch_action_with_token(&app, 0, Some("fresh-action")),
        "invalid-action" => launch::launch_action_with_token(&app, usize::MAX, Some("fresh-action")),
        _ => panic!("unknown fixture case"),
    };
    if case.starts_with("missing-") || case == "invalid-action" {
        let expected =
            if case == "invalid-action" { std::io::ErrorKind::InvalidInput } else { std::io::ErrorKind::NotFound };
        assert_eq!(result.unwrap_err().kind(), expected);
        assert!(!output.exists(), "a refused command must not run a child");
        return;
    }
    result.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !output.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let values = fs::read_to_string(output).unwrap();
    let expected_token = match case.as_str() {
        "app-fresh" => "fresh-app",
        "action-fresh" => "fresh-action",
        _ => "<unset>",
    };
    let work = desktop.parent().unwrap().join("working directory");
    let arguments = if case.starts_with("app-") {
        "normal\ntwo words\nsemi;literal\nContext Probe\n"
    } else {
        "action\ntwo words\nContext Probe\n"
    };
    assert_eq!(values, format!("{expected_token}\n<unset>\n{}\n{arguments}", work.display()));
}
