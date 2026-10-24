use super::*;
use pretty_assertions::assert_eq;

fn uri(value: &str) -> PathUri {
    PathUri::parse(value).expect("valid path URI")
}

fn path(value: &str) -> FileSystemPath {
    FileSystemPath::Path { path: uri(value) }
}

fn entry(path: FileSystemPath, access: FileSystemAccessMode) -> FileSystemSandboxEntry {
    FileSystemSandboxEntry::new(path, access)
}

fn workspace(subpath: Option<&str>) -> FileSystemPath {
    FileSystemPath::Special {
        value: FileSystemSpecialPath::ProjectRoots {
            subpath: subpath.map(str::to_owned),
        },
    }
}

fn glob(pattern: impl Into<String>) -> FileSystemPath {
    FileSystemPath::GlobPattern {
        pattern: pattern.into(),
    }
}

fn policy_context<'a>(
    cwd: &'a PathUri,
    workspace_roots: &'a [PathUri],
    user_home_dir: Option<&'a PathUri>,
    temporary_directories: Option<&'a [PathUri]>,
) -> FileSystemSandboxPolicyContext<'a> {
    FileSystemSandboxPolicyContext {
        cwd,
        workspace_roots,
        user_home_dir,
        temporary_directories,
    }
}

fn assert_unchanged(
    policy: &FileSystemSandboxPolicy,
    context: &FileSystemSandboxPolicyContext<'_>,
) {
    assert_eq!(policy.for_approved_command(context), policy.clone());
}

#[test]
fn approved_command_materializes_denials_without_host_path_projection() {
    for cwd_text in [
        "file:///work/repo",
        "file:///C:/work/repo",
        "file://server/share/work/repo",
    ] {
        let cwd = uri(cwd_text);
        let roots = [cwd.clone()];
        let temporary_directories = [];
        let context = policy_context(&cwd, &roots, None, Some(&temporary_directories));
        let root = file_system_root(&context).expect("filesystem root");
        let private = cwd.join_descendant("private").unwrap();
        let skip_missing = cwd.join_descendant("missing-secret").unwrap();
        let workspace_glob = cwd.join("**/*.env").unwrap().inferred_native_path_string();
        let absolute_glob = root
            .join("outside/[ab].env")
            .unwrap()
            .inferred_native_path_string();

        let mut policy = FileSystemSandboxPolicy::restricted(vec![
            entry(
                FileSystemPath::Special {
                    value: FileSystemSpecialPath::Root,
                },
                FileSystemAccessMode::Read,
            ),
            // Old grants are discarded before validating retained deny facts.
            entry(path("file:///D:/old-write"), FileSystemAccessMode::Write),
            entry(path("file:///D:/old-read"), FileSystemAccessMode::Read),
            entry(workspace(Some("private")), FileSystemAccessMode::Deny),
            entry(
                workspace(Some("private/public")),
                FileSystemAccessMode::Read,
            ),
            entry(
                workspace(Some("private/writable")),
                FileSystemAccessMode::Write,
            ),
            entry(
                glob(project_roots_glob_pattern(std::path::Path::new("**/*.env"))),
                FileSystemAccessMode::Deny,
            ),
            entry(glob(absolute_glob.clone()), FileSystemAccessMode::Deny),
            FileSystemSandboxEntry::skip_missing_path(
                FileSystemPath::Path {
                    path: skip_missing.clone(),
                },
                FileSystemAccessMode::Deny,
            ),
        ]);
        policy.glob_scan_max_depth = Some(7);

        let approved = policy.for_approved_command(&context);
        let expected = FileSystemSandboxPolicy {
            kind: FileSystemSandboxKind::Restricted,
            glob_scan_max_depth: Some(7),
            entries: vec![
                entry(
                    FileSystemPath::Special {
                        value: FileSystemSpecialPath::Root,
                    },
                    FileSystemAccessMode::Write,
                ),
                entry(private.clone().into(), FileSystemAccessMode::Deny),
                entry(glob(workspace_glob.clone()), FileSystemAccessMode::Deny),
                entry(glob(absolute_glob), FileSystemAccessMode::Deny),
                FileSystemSandboxEntry::skip_missing_path(
                    FileSystemPath::Path {
                        path: skip_missing.clone(),
                    },
                    FileSystemAccessMode::Deny,
                ),
                entry(
                    root.join_descendant(".git").unwrap().into(),
                    FileSystemAccessMode::Write,
                ),
                entry(
                    root.join_descendant(".agents").unwrap().into(),
                    FileSystemAccessMode::Write,
                ),
                entry(
                    root.join_descendant(".codex").unwrap().into(),
                    FileSystemAccessMode::Write,
                ),
            ],
        };
        assert_eq!(approved, expected, "{cwd_text}");
        assert_eq!(
            approved.resolve_access(&uri("file:///D:/old-write"), &context),
            FileSystemAccessMode::Deny,
            "old foreign write grant must not survive for {cwd_text}"
        );
        assert_eq!(
            approved.resolve_access(&cwd.join("private/public").unwrap(), &context),
            FileSystemAccessMode::Deny,
            "old deeper read grant must not override a retained denial for {cwd_text}"
        );

        let matcher = ReadDenyMatcher::from_context(&approved, &context)
            .expect("valid retained denials have a matcher");
        assert!(matcher.is_read_denied_uri(&cwd.join("private/writable/file").unwrap(), &context));
        assert!(matcher.is_read_denied_uri(&cwd.join("nested/secret.env").unwrap(), &context));
        assert!(matcher.is_read_denied_uri(&root.join("outside/a.env").unwrap(), &context));

        let raw = RawFileSystemSandboxPolicy::try_from(approved.clone()).unwrap();
        let wire: RawFileSystemSandboxPolicy =
            serde_json::from_value(serde_json::to_value(raw).unwrap()).unwrap();
        assert_eq!(FileSystemSandboxPolicy::try_from(wire).unwrap(), approved);
    }
}

#[test]
fn approved_command_only_reopens_metadata_not_denied_targets() {
    let cwd = uri("file:///work/repo");
    let roots = [cwd.clone()];
    let temporary_directories = [];
    let context = policy_context(&cwd, &roots, None, Some(&temporary_directories));
    let root = file_system_root(&context).unwrap();
    let git = root.join_descendant(".git").unwrap();
    let agents = root.join_descendant(".agents").unwrap();
    let codex = root.join_descendant(".codex").unwrap();
    let private = codex.join_descendant("private").unwrap();

    let policy = FileSystemSandboxPolicy::restricted(vec![
        entry(path(&git.to_string()), FileSystemAccessMode::Deny),
        entry(
            glob(agents.inferred_native_path_string()),
            FileSystemAccessMode::Deny,
        ),
        entry(path(&private.to_string()), FileSystemAccessMode::Deny),
    ]);
    let approved = policy.for_approved_command(&context);

    assert!(!approved.can_write_path(&git, &context));
    assert!(!approved.can_write_path(&agents, &context));
    assert!(approved.can_write_path(&codex, &context));
    assert!(approved.can_write_path(&codex.join("public").unwrap(), &context));
    assert!(!approved.can_write_path(&private.join("secret").unwrap(), &context));
}

#[test]
fn approved_command_falls_back_for_unsafe_or_unresolvable_denials() {
    let cwd = uri("file:///work/repo");
    let roots = [cwd.clone()];
    let temporary_directories = [];
    let context = policy_context(&cwd, &roots, None, Some(&temporary_directories));
    let root = file_system_root(&context).unwrap();
    let root_denials = [
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Root,
        },
        FileSystemPath::Path { path: root.clone() },
        glob("/"),
    ];
    for denial in root_denials {
        let policy =
            FileSystemSandboxPolicy::restricted(vec![entry(denial, FileSystemAccessMode::Deny)]);
        assert_unchanged(&policy, &context);
    }

    for denial in [
        glob("/work/repo/[z-a]"),
        workspace(Some("../outside")),
        workspace(Some("~/private")),
        glob(project_roots_glob_pattern(std::path::Path::new(
            "~/private/*.env",
        ))),
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Minimal,
        },
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Unknown {
                path: "future-denial".to_owned(),
                subpath: None,
            },
        },
    ] {
        let policy =
            FileSystemSandboxPolicy::restricted(vec![entry(denial, FileSystemAccessMode::Deny)]);
        assert_unchanged(&policy, &context);
    }

    let no_workspaces = policy_context(&cwd, &[], None, Some(&temporary_directories));
    for denial in [
        workspace(Some("private")),
        glob(project_roots_glob_pattern(std::path::Path::new("**/*.env"))),
    ] {
        let policy =
            FileSystemSandboxPolicy::restricted(vec![entry(denial, FileSystemAccessMode::Deny)]);
        assert_unchanged(&policy, &no_workspaces);
    }

    let no_home = FileSystemSandboxPolicy::restricted(vec![entry(
        glob("~/private/**"),
        FileSystemAccessMode::Deny,
    )]);
    assert_unchanged(&no_home, &context);

    let missing_tmp = policy_context(&cwd, &roots, None, None);
    let tmp_denial = FileSystemSandboxPolicy::restricted(vec![entry(
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Tmpdir,
        },
        FileSystemAccessMode::Deny,
    )]);
    assert_unchanged(&tmp_denial, &missing_tmp);

    let mixed_roots = [uri("file:///C:/workspace")];
    let mixed_context = policy_context(&cwd, &mixed_roots, None, Some(&temporary_directories));
    let policy = FileSystemSandboxPolicy::restricted(vec![entry(
        workspace(Some("private")),
        FileSystemAccessMode::Deny,
    )]);
    assert_unchanged(&policy, &mixed_context);

    let foreign_denials = [path("file:///C:/private"), path("file:///%00/bad/path/Lw")];
    for denial in foreign_denials {
        let policy =
            FileSystemSandboxPolicy::restricted(vec![entry(denial, FileSystemAccessMode::Deny)]);
        assert_unchanged(&policy, &context);
    }

    let windows_cwd = uri("file:///C:/work/repo");
    let windows_roots = [windows_cwd.clone()];
    let windows_tmp = [];
    let posix_home = uri("file:///Users/codex");
    let mixed_home_context = policy_context(
        &windows_cwd,
        &windows_roots,
        Some(&posix_home),
        Some(&windows_tmp),
    );
    let home_denial = FileSystemSandboxPolicy::restricted(vec![entry(
        glob("~/private/**"),
        FileSystemAccessMode::Deny,
    )]);
    assert_unchanged(&home_denial, &mixed_home_context);

    let posix_tmp = [uri("file:///tmp")];
    let mixed_tmp_context = policy_context(&windows_cwd, &windows_roots, None, Some(&posix_tmp));
    let tmp_denial = FileSystemSandboxPolicy::restricted(vec![entry(
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Tmpdir,
        },
        FileSystemAccessMode::Deny,
    )]);
    assert_unchanged(&tmp_denial, &mixed_tmp_context);

    for subpath in [r"~\private", r"C:\outside", r"\outside", "/outside"] {
        let policy = FileSystemSandboxPolicy::restricted(vec![entry(
            workspace(Some(subpath)),
            FileSystemAccessMode::Deny,
        )]);
        assert_unchanged(
            &policy,
            &policy_context(&windows_cwd, &windows_roots, None, Some(&windows_tmp)),
        );

        let scoped_glob = format!("codex-project-roots://{subpath}");
        let policy = FileSystemSandboxPolicy::restricted(vec![entry(
            glob(scoped_glob),
            FileSystemAccessMode::Deny,
        )]);
        assert_unchanged(
            &policy,
            &policy_context(&windows_cwd, &windows_roots, None, Some(&windows_tmp)),
        );
    }

    let opaque_cwd = uri("file:///%00/bad/path/Lw");
    let opaque_context = policy_context(&opaque_cwd, &[], None, Some(&temporary_directories));
    let policy = FileSystemSandboxPolicy::restricted(vec![]);
    assert_unchanged(&policy, &opaque_context);
}

#[test]
fn approved_command_rejects_unsafe_glob_bases_but_keeps_absolute_glob_syntax() {
    for cwd_text in ["file:///work/%5Brepo%5D", "file:///work/repo%5Cescape"] {
        let cwd = uri(cwd_text);
        let roots = [cwd.clone()];
        let temporary_directories = [];
        let context = policy_context(&cwd, &roots, None, Some(&temporary_directories));
        let policy = FileSystemSandboxPolicy::restricted(vec![entry(
            glob("secrets/**"),
            FileSystemAccessMode::Deny,
        )]);
        assert_unchanged(&policy, &context);
    }

    let cwd = uri("file:///work/repo");
    let roots = [cwd.clone()];
    let home = uri("file:///home/%5Buser%5D");
    let temporary_directories = [];
    let context = policy_context(&cwd, &roots, Some(&home), Some(&temporary_directories));
    let home_glob = FileSystemSandboxPolicy::restricted(vec![entry(
        glob("~/secrets/**"),
        FileSystemAccessMode::Deny,
    )]);
    assert_unchanged(&home_glob, &context);

    let lossy_directory = uri("file:///work/%FF");
    let lossy_roots = [lossy_directory.clone()];
    let temporary_directories = [];
    let lossy_cwd_context = policy_context(
        &lossy_directory,
        &lossy_roots,
        None,
        Some(&temporary_directories),
    );
    let relative_glob = FileSystemSandboxPolicy::restricted(vec![entry(
        glob("secrets/**"),
        FileSystemAccessMode::Deny,
    )]);
    assert_unchanged(&relative_glob, &lossy_cwd_context);

    let home = lossy_directory.clone();
    let lossy_home_context =
        policy_context(&cwd, &roots, Some(&home), Some(&temporary_directories));
    assert_unchanged(&home_glob, &lossy_home_context);

    let lossy_workspace_roots = [lossy_directory.clone()];
    let lossy_workspace_context = policy_context(
        &cwd,
        &lossy_workspace_roots,
        None,
        Some(&temporary_directories),
    );
    let scoped_glob = FileSystemSandboxPolicy::restricted(vec![entry(
        glob(project_roots_glob_pattern(std::path::Path::new("**/*.env"))),
        FileSystemAccessMode::Deny,
    )]);
    assert_unchanged(&scoped_glob, &lossy_workspace_context);

    let unsafe_workspace = uri("file:///work/%5Brepo%5D");
    let unsafe_roots = [unsafe_workspace.clone()];
    let unsafe_context = policy_context(
        &unsafe_workspace,
        &unsafe_roots,
        None,
        Some(&temporary_directories),
    );
    let workspace_glob = FileSystemSandboxPolicy::restricted(vec![entry(
        glob(project_roots_glob_pattern(std::path::Path::new("**/*.env"))),
        FileSystemAccessMode::Deny,
    )]);
    assert_unchanged(&workspace_glob, &unsafe_context);

    let absolute = cwd.join("outside/[ab].env").unwrap();
    let absolute_glob = FileSystemSandboxPolicy::restricted(vec![entry(
        glob(absolute.inferred_native_path_string()),
        FileSystemAccessMode::Deny,
    )]);
    let approved = absolute_glob.for_approved_command(&context);
    assert_ne!(approved, absolute_glob);
    let matcher = ReadDenyMatcher::from_context(&approved, &context).unwrap();
    assert!(matcher.is_read_denied_uri(&cwd.join("outside/a.env").unwrap(), &context));
    assert!(!matcher.is_read_denied_uri(&cwd.join("outside/c.env").unwrap(), &context));
}

#[test]
fn approved_command_handles_missing_and_inapplicable_tmp_facts_without_host_lookup() {
    let cwd = uri("file:///C:/work/repo");
    let roots = [cwd.clone()];
    let policy = FileSystemSandboxPolicy::restricted(vec![
        entry(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Tmpdir,
            },
            FileSystemAccessMode::Deny,
        ),
        entry(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::SlashTmp,
            },
            FileSystemAccessMode::Deny,
        ),
    ]);
    let no_tmp_facts = policy_context(&cwd, &roots, None, None);
    assert_unchanged(&policy, &no_tmp_facts);

    let no_temporary_directories = [];
    let context = policy_context(&cwd, &roots, None, Some(&no_temporary_directories));
    let approved = policy.for_approved_command(&context);
    assert_ne!(approved, policy);
    assert!(approved.entries.contains(&entry(
        FileSystemPath::Special {
            value: FileSystemSpecialPath::SlashTmp,
        },
        FileSystemAccessMode::Deny,
    )));
    assert!(approved.entries.contains(&entry(
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Tmpdir,
        },
        FileSystemAccessMode::Deny,
    )));
}

#[test]
fn approved_command_preserves_nonrestricted_policy_and_source_context_isolation() {
    let cwd = uri("file:///work/repo");
    let roots = [cwd.clone()];
    let temporary_directories = [];
    let context = policy_context(&cwd, &roots, None, Some(&temporary_directories));
    for policy in [
        FileSystemSandboxPolicy::unrestricted(),
        FileSystemSandboxPolicy::external_sandbox(),
    ] {
        assert_eq!(policy.for_approved_command(&context), policy);
    }

    let policy = FileSystemSandboxPolicy::restricted(vec![entry(
        path("file:///C:/old-foreign-grant"),
        FileSystemAccessMode::Write,
    )]);
    let approved = policy.for_approved_command(&context);
    assert_eq!(
        approved.resolve_access(&uri("file:///work/file"), &context),
        FileSystemAccessMode::Write
    );
    assert_eq!(
        approved.resolve_access(&uri("file:///C:/old-foreign-grant"), &context),
        FileSystemAccessMode::Deny
    );
}
