//! Approved root metadata writes must not remount targets hidden by executor-side denials.

use super::codex_linux_sandbox_exe;
use super::should_skip_bwrap_tests;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::FileSystemSandboxPolicyContext;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use std::fs;
use std::process::Command;
use std::process::Output;

fn skip_on_outer_bwrap_namespace_failure(output: &Output) -> bool {
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.starts_with("bwrap: Creating new namespace failed")
        || stderr.contains("No permissions to create a new namespace")
        || stderr.contains("setting up uid map")
    {
        eprintln!(
            "skipping root metadata integration test: bubblewrap user namespaces unavailable: {stderr}"
        );
        return true;
    }
    false
}

#[tokio::test]
async fn root_metadata_symlinks_cannot_reopen_approved_denials() {
    if should_skip_bwrap_tests().await {
        eprintln!(
            "skipping root metadata integration test: bubblewrap/user-namespace prerequisite probe was unavailable or timed out"
        );
        return;
    }
    let Some(bwrap) = codex_sandboxing::find_system_bwrap_in_path() else {
        eprintln!("skipping root metadata integration test: system bubblewrap is unavailable");
        return;
    };

    // All root metadata aliases below live inside this disposable tmpfs root;
    // the test never creates or modifies metadata in the host filesystem root.
    let fixture = tempfile::tempdir_in("/tmp").expect("create temporary fixture");
    let work = fixture.path().join("work");
    let private = fixture.path().join("private");
    fs::create_dir_all(work.join("public")).expect("create public work directory");
    fs::create_dir_all(private.join("reopened")).expect("create private directory");
    fs::write(work.join("public/secret"), "outside").expect("write public secret");
    fs::write(private.join("reopened/secret"), "private").expect("write private secret");
    fs::write(private.join("sibling"), "sibling").expect("write private sibling");
    let sandbox_binary = work.join("sandbox");
    fs::copy(codex_linux_sandbox_exe(), &sandbox_binary).expect("copy sandbox binary");

    let path_entry = |path: &str, access| {
        FileSystemSandboxEntry::new(
            AbsolutePathBuf::from_absolute_path(path)
                .expect("absolute policy path")
                .into(),
            access,
        )
    };
    let cwd = AbsolutePathBuf::from_absolute_path("/tmp")
        .expect("absolute cwd")
        .into();
    let context = FileSystemSandboxPolicyContext {
        cwd: &cwd,
        workspace_roots: std::slice::from_ref(&cwd),
        user_home_dir: None,
        temporary_directories: Some(&[]),
    };
    let approved = |denied| {
        let mut policy = FileSystemSandboxPolicy::read_only();
        policy
            .entries
            .push(path_entry(denied, FileSystemAccessMode::Deny));
        policy.for_approved_command(&context)
    };
    let run = |policy: &FileSystemSandboxPolicy,
               private_mount: &str,
               codex_target: &str,
               script: &str| {
        let profile =
            PermissionProfile::from_runtime_permissions(policy, NetworkSandboxPolicy::Enabled);
        let mut command = Command::new(&bwrap);
        command
            .args("--unshare-user --uid 0 --gid 0 --die-with-parent --tmpfs /".split_whitespace());
        command.args(
            "--ro-bind /usr /usr --ro-bind /etc /etc --proc /proc --dev /dev".split_whitespace(),
        );
        command.args("--dir /run --dir /.agents --symlink /private-link /.git".split_whitespace());
        command.args(["--symlink", codex_target, "/.codex"]);
        command.args(["--symlink", "/private/reopened", "/private-link"]);
        for path in ["/bin", "/sbin", "/lib", "/lib64"] {
            match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    command
                        .arg("--symlink")
                        .arg(fs::read_link(path).expect("read system path symlink"))
                        .arg(path);
                }
                Ok(_) => {
                    command.args(["--ro-bind", path, path]);
                }
                Err(_) => {}
            }
        }
        command.arg("--bind").arg(&work).arg("/tmp");
        command.arg("--bind").arg(&private).arg(private_mount);
        if private_mount != "/private" {
            command.args(["--symlink", private_mount, "/private"]);
        }
        command.args(
            "--chdir /tmp --setenv TMPDIR /tmp --setenv PATH /usr/bin:/bin".split_whitespace(),
        );
        command.args(
            "-- /tmp/sandbox --sandbox-policy-cwd /tmp --permission-profile".split_whitespace(),
        );
        command.arg(serde_json::to_string(&profile).expect("serialize permission profile"));
        command.args(["--", "/bin/sh", "-c", script]);
        command.output().expect("run outer bubblewrap fixture")
    };

    let private_denied = approved("/private");
    let script = r#"
        set -eu
        if cat /private/reopened/secret >/dev/null 2>&1 ||
           cat /.codex/secret >/dev/null 2>&1 || cat /.git/secret >/dev/null 2>&1; then exit 21; fi
        if (printf bad > /.codex/changed) 2>/dev/null; then exit 22; fi
        printf good > /.agents/allowed
        printf good > /tmp/marker
        printf 'root-metadata-denials-held\n'
    "#;
    let output = run(&private_denied, "/private", "/private/reopened", script);
    if skip_on_outer_bwrap_namespace_failure(&output) {
        return;
    }
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, b"root-metadata-denials-held\n");
    assert_eq!(
        fs::read_to_string(work.join("marker")).expect("read marker written through /tmp"),
        "good"
    );
    assert!(!private.join("reopened/changed").exists());

    let script = r#"
        set -eu
        printf safe > /.codex/allowed
        if cat /.git/secret >/dev/null 2>&1; then exit 23; fi
    "#;
    let output = run(&private_denied, "/private", "/tmp/public", script);
    if skip_on_outer_bwrap_namespace_failure(&output) {
        return;
    }
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert_eq!(
        fs::read_to_string(work.join("public/allowed"))
            .expect("read public file written through /.codex"),
        "safe"
    );

    let script = r#"
        set -eu
        rm /private
        ln -s /tmp/public /private
        cat /.codex/secret
    "#;
    let output = run(&private_denied, "/real-private", "/private", script);
    if skip_on_outer_bwrap_namespace_failure(&output) {
        return;
    }
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(
            "cannot enforce sandbox deny-read path /private because it crosses writable symlink /private"
        ),
        "{output:?}"
    );
    assert!(output.stdout.is_empty(), "{output:?}");

    let mut reopened = private_denied;
    reopened
        .entries
        .push(path_entry("/private/reopened", FileSystemAccessMode::Write));
    let script = r#"
        set -eu
        test "$(cat /.codex/secret)" = private
        if cat /private/sibling >/dev/null 2>&1; then exit 24; fi
        printf nested > /private/reopened/changed
    "#;
    let output = run(&reopened, "/private", "/private/reopened", script);
    if skip_on_outer_bwrap_namespace_failure(&output) {
        return;
    }
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert_eq!(
        fs::read_to_string(private.join("reopened/changed"))
            .expect("read file written under the deeper writable grant"),
        "nested"
    );
    assert_eq!(
        fs::read_to_string(private.join("sibling")).expect("read unchanged sibling"),
        "sibling"
    );

    let output = run(
        &approved("/.codex/secret"),
        "/private",
        "/private/reopened",
        "true",
    );
    if skip_on_outer_bwrap_namespace_failure(&output) {
        return;
    }
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(
            "cannot enforce sandbox deny-read path /.codex/secret because it crosses writable symlink /.codex"
        ),
        "{output:?}"
    );
    assert!(output.stdout.is_empty(), "{output:?}");
}
