use super::FileSystemAccessMode;
use super::FileSystemPath;
use super::FileSystemSandboxEntry;
use super::FileSystemSandboxPolicy;
use super::FileSystemSpecialPath;
use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use std::path::Path;
use tempfile::TempDir;

fn absolute_path(path: impl AsRef<Path>) -> AbsolutePathBuf {
    AbsolutePathBuf::from_absolute_path(path).expect("absolute path")
}

#[cfg(unix)]
#[test]
fn inheriting_root_metadata_suppresses_only_separate_filesystem_metadata_roots() {
    let cwd = TempDir::new().expect("tempdir");
    let file_system_root = AbsolutePathBuf::from_absolute_path("/").expect("filesystem root");
    let agents_root = file_system_root.join(".agents");
    let denied_path = cwd.path().join("denied");
    let effective_denied_root = absolute_path(
        cwd.path()
            .canonicalize()
            .expect("canonicalize cwd")
            .join("denied"),
    );
    let policy = FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            FileSystemAccessMode::Write,
        ),
        FileSystemSandboxEntry::new(agents_root.clone().into(), FileSystemAccessMode::Write),
        FileSystemSandboxEntry::new(
            absolute_path(&denied_path).into(),
            FileSystemAccessMode::Deny,
        ),
    ]);

    let separate_roots = policy.get_writable_roots_with_cwd(cwd.path());
    let preserving_mutable_paths_roots =
        policy.get_writable_roots_with_cwd_preserving_mutable_paths(cwd.path());
    assert_eq!(
        separate_roots
            .iter()
            .map(|root| root.root.clone())
            .collect::<Vec<_>>(),
        vec![file_system_root.clone(), agents_root.clone()]
    );
    assert_eq!(preserving_mutable_paths_roots, separate_roots);

    let inherited_roots = policy.get_writable_roots_with_cwd_inheriting_root_metadata(cwd.path());

    assert_eq!(inherited_roots.len(), 1);
    assert_eq!(inherited_roots[0].root, file_system_root);
    assert_eq!(
        inherited_roots[0].protected_metadata_names,
        vec![".git".to_string(), ".codex".to_string()]
    );
    assert!(
        inherited_roots[0]
            .read_only_subpaths
            .contains(&effective_denied_root)
    );
    assert!(
        policy.can_write_path_with_cwd(agents_root.join("settings.toml").as_path(), cwd.path())
    );
    assert!(inherited_roots[0].is_path_writable(agents_root.join("settings.toml").as_path()));
}

#[cfg(unix)]
#[test]
fn standalone_filesystem_metadata_write_is_not_suppressed() {
    let cwd = TempDir::new().expect("tempdir");
    let file_system_root = AbsolutePathBuf::from_absolute_path("/").expect("filesystem root");
    let codex_root = file_system_root.join(".codex");
    let denied_root = AbsolutePathBuf::from_absolute_path(cwd.path())
        .expect("absolute cwd")
        .join("denied");
    let policy = FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry::new(codex_root.clone().into(), FileSystemAccessMode::Write),
        FileSystemSandboxEntry::new(denied_root.into(), FileSystemAccessMode::Deny),
    ]);

    let separate_roots = policy.get_writable_roots_with_cwd(cwd.path());
    let inherited_roots = policy.get_writable_roots_with_cwd_inheriting_root_metadata(cwd.path());

    assert_eq!(inherited_roots, separate_roots);
    assert!(inherited_roots.iter().any(|root| root.root == codex_root));
}

#[cfg(unix)]
#[test]
fn unreadable_roots_preserve_denied_symlink_paths_and_same_target_overrides() {
    use std::fs;
    use std::os::unix::fs::symlink;

    let cwd = TempDir::new().expect("tempdir");
    let target = cwd.path().join("target");
    let alias = cwd.path().join("alias");
    let same_target = cwd.path().join("same-target");
    let denied_parent = cwd.path().join("denied-parent");
    let allowed_child = denied_parent.join("allowed-child");
    fs::create_dir(&target).expect("create target");
    symlink(&target, &alias).expect("create alias");

    let policy = FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            FileSystemAccessMode::Write,
        ),
        FileSystemSandboxEntry::new(absolute_path(&alias).into(), FileSystemAccessMode::Deny),
        FileSystemSandboxEntry::new(absolute_path(&target).into(), FileSystemAccessMode::Deny),
        FileSystemSandboxEntry::new(absolute_path(&alias).into(), FileSystemAccessMode::Deny),
        FileSystemSandboxEntry::new(
            absolute_path(&same_target).into(),
            FileSystemAccessMode::Deny,
        ),
        FileSystemSandboxEntry::new(
            absolute_path(&same_target).into(),
            FileSystemAccessMode::Write,
        ),
        FileSystemSandboxEntry::new(
            absolute_path(&denied_parent).into(),
            FileSystemAccessMode::Deny,
        ),
        FileSystemSandboxEntry::new(
            absolute_path(&allowed_child).into(),
            FileSystemAccessMode::Write,
        ),
    ]);

    let resolved_roots = policy.get_unreadable_roots_with_cwd(cwd.path());
    let preserved_roots = policy.get_unreadable_roots_with_cwd_preserving_symlinks(cwd.path());
    let alias_root = AbsolutePathBuf::from_absolute_path(&alias).expect("absolute alias");
    let target_root = AbsolutePathBuf::from_absolute_path(&target).expect("absolute target");
    let same_target_root = absolute_path(&same_target);
    let denied_parent_root = absolute_path(&denied_parent);
    let allowed_child_root = absolute_path(&allowed_child);

    let mut expected_roots = resolved_roots.clone();
    for literal_root in [
        alias_root.clone(),
        target_root,
        alias_root.clone(),
        same_target_root.clone(),
        denied_parent_root,
    ] {
        if !expected_roots.contains(&literal_root) {
            expected_roots.push(literal_root);
        }
    }
    assert_eq!(preserved_roots, expected_roots);
    assert_eq!(
        preserved_roots
            .iter()
            .filter(|root| root.as_path() == alias_root.as_path())
            .count(),
        1
    );
    assert!(policy.can_read_path_with_cwd(&allowed_child, cwd.path()));
    assert!(!policy.can_read_path_with_cwd(&same_target, cwd.path()));
    assert!(!preserved_roots.contains(&allowed_child_root));

    let root_denied_policy =
        FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            FileSystemAccessMode::Deny,
        )]);
    assert!(
        root_denied_policy
            .get_unreadable_roots_with_cwd_preserving_symlinks(cwd.path())
            .is_empty()
    );

    for policy in [
        FileSystemSandboxPolicy::unrestricted(),
        FileSystemSandboxPolicy::external_sandbox(),
    ] {
        assert_eq!(
            policy.get_unreadable_roots_with_cwd_preserving_symlinks(cwd.path()),
            policy.get_unreadable_roots_with_cwd(cwd.path())
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn unreadable_roots_include_literal_paths_under_top_level_system_aliases() {
    use std::fs;

    if !fs::symlink_metadata("/tmp")
        .expect("inspect /tmp")
        .file_type()
        .is_symlink()
    {
        return;
    }

    let cwd = TempDir::new_in("/tmp").expect("tempdir under /tmp");
    let denied_path = cwd.path().join("denied");
    let policy = FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            FileSystemAccessMode::Write,
        ),
        FileSystemSandboxEntry::new(
            absolute_path(&denied_path).into(),
            FileSystemAccessMode::Deny,
        ),
    ]);

    let resolved_roots = policy.get_unreadable_roots_with_cwd(cwd.path());
    let preserved_roots = policy.get_unreadable_roots_with_cwd_preserving_symlinks(cwd.path());
    let effective_denied_path = AbsolutePathBuf::from_absolute_path(
        cwd.path()
            .canonicalize()
            .expect("canonicalize tempdir")
            .join("denied"),
    )
    .expect("absolute effective denied path");
    let literal_denied_path =
        AbsolutePathBuf::from_absolute_path(&denied_path).expect("absolute literal denied path");

    assert_eq!(resolved_roots, vec![effective_denied_path.clone()]);
    assert_ne!(effective_denied_path, literal_denied_path);
    assert_eq!(
        preserved_roots,
        vec![effective_denied_path, literal_denied_path]
    );
}
