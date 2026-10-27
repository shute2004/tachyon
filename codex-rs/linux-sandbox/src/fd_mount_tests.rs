use super::verify_fd_mounts;
use pretty_assertions::assert_eq;
use std::fs::File;
use std::os::fd::IntoRawFd;
use std::process::Command;
use std::process::Stdio;
use std::thread;
use std::time::Duration;
use std::time::Instant;

const ISOLATED_CASE_ENV: &str = "TACHYON_FD_MOUNT_ISOLATED_CASE";
const CASE_COMPLETION_SENTINEL: &str = "TACHYON_FD_MOUNT_CASE_COMPLETED::";

// Parallel libtest peers can reuse an FD number after it is closed. Check descriptor lifetimes
// in a separate process so another test cannot make a correctly closed descriptor look open.
fn run_isolated_case(exact_test_name: &str, test_body: impl FnOnce()) {
    if let Some(requested_test_name) = std::env::var_os(ISOLATED_CASE_ENV) {
        assert_eq!(requested_test_name.to_str(), Some(exact_test_name));
        test_body();
        println!("{CASE_COMPLETION_SENTINEL}{exact_test_name}");
        return;
    }

    let output_dir = tempfile::tempdir().expect("child output directory should be created");
    let stdout_path = output_dir.path().join("stdout");
    let stderr_path = output_dir.path().join("stderr");
    let stdout = File::create(&stdout_path).expect("child stdout capture should be created");
    let stderr = File::create(&stderr_path).expect("child stderr capture should be created");
    let mut child =
        Command::new(std::env::current_exe().expect("current test executable should be available"))
            .args([
                "--exact",
                exact_test_name,
                "--test-threads=1",
                "--nocapture",
            ])
            .env(ISOLATED_CASE_ENV, exact_test_name)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("isolated test child should start");

    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let kill_result = child.kill();
                let wait_result = child.wait();
                let stdout = std::fs::read_to_string(&stdout_path)
                    .unwrap_or_else(|error| format!("<could not read stdout: {error}>"));
                let stderr = std::fs::read_to_string(&stderr_path)
                    .unwrap_or_else(|error| format!("<could not read stderr: {error}>"));
                panic!(
                    "isolated test {exact_test_name} exceeded 20 seconds; kill={kill_result:?}; \
                     wait={wait_result:?}; stdout:\n{stdout}\nstderr:\n{stderr}"
                );
            }
            Err(error) => {
                let kill_result = child.kill();
                let wait_result = child.wait();
                let stdout = std::fs::read_to_string(&stdout_path)
                    .unwrap_or_else(|error| format!("<could not read stdout: {error}>"));
                let stderr = std::fs::read_to_string(&stderr_path)
                    .unwrap_or_else(|error| format!("<could not read stderr: {error}>"));
                panic!(
                    "could not wait for isolated test {exact_test_name}: {error}; \
                     kill={kill_result:?}; wait={wait_result:?}; \
                     stdout:\n{stdout}\nstderr:\n{stderr}"
                );
            }
        }
    };

    let stdout = std::fs::read_to_string(&stdout_path).expect("child stdout should be readable");
    let stderr = std::fs::read_to_string(&stderr_path).expect("child stderr should be readable");
    assert!(
        status.success(),
        "isolated test {exact_test_name} failed with {status}; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let expected_sentinel = format!("{CASE_COMPLETION_SENTINEL}{exact_test_name}");
    assert!(
        stdout.contains(&expected_sentinel),
        "isolated test child did not report completion sentinel {expected_sentinel:?}; \
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// A matching mount authenticates its original inode and closes the inherited descriptor.
#[test]
fn matching_mount_closes_inherited_descriptor() {
    run_isolated_case(
        "fd_mount::tests::matching_mount_closes_inherited_descriptor",
        || {
            let root = tempfile::tempdir().expect("temporary directory should be created");
            let descriptor = File::open(root.path())
                .expect("directory descriptor should open")
                .into_raw_fd();
            let marker = format!("{descriptor}:{}", root.path().display());

            verify_fd_mounts(&[marker]).expect("matching mount should verify");

            assert_eq!(unsafe { libc::fcntl(descriptor, libc::F_GETFD) }, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EBADF)
            );
        },
    );
}

/// A swapped destination fails closed without leaking the original descriptor.
#[test]
fn mismatched_mount_closes_inherited_descriptor() {
    run_isolated_case(
        "fd_mount::tests::mismatched_mount_closes_inherited_descriptor",
        || {
            let source = tempfile::tempdir().expect("source directory should be created");
            let destination = tempfile::tempdir().expect("destination directory should be created");
            let descriptor = File::open(source.path())
                .expect("source directory descriptor should open")
                .into_raw_fd();
            let marker = format!("{descriptor}:{}", destination.path().display());

            let error = verify_fd_mounts(&[marker]).expect_err("different inodes must be rejected");

            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            assert_eq!(unsafe { libc::fcntl(descriptor, libc::F_GETFD) }, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EBADF)
            );
        },
    );
}

/// A symlink to the original inode is not itself the authenticated mount.
#[test]
fn symlinked_mount_destination_closes_inherited_descriptor() {
    run_isolated_case(
        "fd_mount::tests::symlinked_mount_destination_closes_inherited_descriptor",
        || {
            let root = tempfile::tempdir().expect("temporary directory should be created");
            let source = root.path().join("source");
            let destination = root.path().join("destination");
            std::fs::create_dir(&source).expect("source directory should be created");
            std::os::unix::fs::symlink(&source, &destination)
                .expect("mount destination symlink should be created");
            let descriptor = File::open(&source)
                .expect("source directory descriptor should open")
                .into_raw_fd();
            let marker = format!("{descriptor}:{}", destination.display());

            let error = verify_fd_mounts(&[marker]).expect_err("symlinked mounts must be rejected");

            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            assert_eq!(unsafe { libc::fcntl(descriptor, libc::F_GETFD) }, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EBADF)
            );
        },
    );
}

/// Malformed mount markers cannot claim standard streams or relative destinations.
#[test]
fn malformed_mount_markers_are_rejected() {
    run_isolated_case(
        "fd_mount::tests::malformed_mount_markers_are_rejected",
        || {
            for marker in [
                "missing-separator",
                "invalid:/tmp",
                "0:/tmp",
                "1:/tmp",
                "2:/tmp",
            ] {
                let error = verify_fd_mounts(&[marker.to_string()])
                    .expect_err("malformed mount marker must be rejected");
                assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            }

            let root = tempfile::tempdir().expect("temporary directory should be created");
            let descriptor = File::open(root.path())
                .expect("directory descriptor should open")
                .into_raw_fd();
            let error = verify_fd_mounts(&[format!("{descriptor}:relative")])
                .expect_err("relative mount destinations must be rejected");

            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            assert_eq!(unsafe { libc::fcntl(descriptor, libc::F_GETFD) }, -1);
        },
    );
}

/// A transferred descriptor can be consumed only once even if a marker is repeated.
#[test]
fn duplicate_mount_descriptors_are_rejected() {
    run_isolated_case(
        "fd_mount::tests::duplicate_mount_descriptors_are_rejected",
        || {
            let root = tempfile::tempdir().expect("temporary directory should be created");
            let descriptor = File::open(root.path())
                .expect("directory descriptor should open")
                .into_raw_fd();
            let marker = format!("{descriptor}:{}", root.path().display());

            let error = verify_fd_mounts(&[marker.clone(), marker])
                .expect_err("a mount descriptor must not be consumed twice");

            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            assert_eq!(unsafe { libc::fcntl(descriptor, libc::F_GETFD) }, -1);
        },
    );
}
