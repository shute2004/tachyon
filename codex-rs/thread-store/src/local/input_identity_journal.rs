use std::fs;
use std::fs::File;
use std::fs::Metadata;
use std::fs::OpenOptions;
use std::io;
use std::io::Read;
use std::io::Write;
use std::num::NonZeroU64;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use codex_protocol::ThreadId;
use serde::Deserialize;
use serde::Serialize;

use crate::InputIdentityContinuity;
use crate::InputIdentityReservation;
use crate::InputStreamIncarnation;
use crate::ReservedInputIdentity;

const JOURNAL_DIRECTORY: &str = "thread-input-identities";
const JOURNAL_VERSION: u8 = 1;
const MAX_JOURNAL_BYTES: u64 = 4096;
const READ_LIMIT: u64 = MAX_JOURNAL_BYTES + 1;

#[derive(Deserialize, Serialize)]
struct JournalRecord {
    version: u8,
    thread_id: ThreadId,
    incarnation: InputStreamIncarnation,
    sequence: NonZeroU64,
}

/// Commits one bounded journal record while the caller holds the live writer guard.
///
/// The Codex-home-owned directory tree is trusted: observed symlinks and non-regular paths are
/// rejected, but checks are not race-proof against another process with the same UID. Successful
/// sync calls are the filesystem's durability acknowledgment, not a hardware power-loss promise.
pub(super) fn reserve(
    codex_home: &Path,
    thread_id: ThreadId,
) -> io::Result<InputIdentityReservation> {
    let home_metadata = fs::metadata(codex_home)?;
    if !home_metadata.is_dir() {
        return Err(invalid_data(format!(
            "Codex home is not a directory: {}",
            codex_home.display()
        )));
    }

    let journal_directory = codex_home.join(JOURNAL_DIRECTORY);
    ensure_journal_directory(&journal_directory)?;

    let canonical_path = journal_directory.join(format!("{thread_id}.json"));
    let pending_path = journal_directory.join(format!("{thread_id}.pending"));

    // Validate the canonical record before examining or recovering the staging file. A damaged
    // canonical record is never reconstructed from rollout history or its timestamp.
    let (record, continuity) = match read_canonical(&canonical_path, thread_id)? {
        Some(previous) => {
            let next = previous.sequence.get().checked_add(1).ok_or_else(|| {
                invalid_data(format!(
                    "input identity sequence exhausted for thread {thread_id}"
                ))
            })?;
            let sequence = NonZeroU64::new(next).ok_or_else(|| {
                invalid_data(format!(
                    "input identity sequence exhausted for thread {thread_id}"
                ))
            })?;
            (
                JournalRecord {
                    version: JOURNAL_VERSION,
                    thread_id,
                    incarnation: previous.incarnation,
                    sequence,
                },
                InputIdentityContinuity::Continuing,
            )
        }
        None => (
            JournalRecord {
                version: JOURNAL_VERSION,
                thread_id,
                incarnation: InputStreamIncarnation::new(),
                sequence: NonZeroU64::new(1).expect("one is non-zero"),
            },
            InputIdentityContinuity::NewIncarnation,
        ),
    };

    recover_pending(&pending_path)?;

    let serialized = serde_json::to_vec(&record).map_err(|err| {
        invalid_data(format!("failed to serialize input identity journal: {err}"))
    })?;
    if serialized.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(invalid_data(
            "serialized input identity record exceeds 4096 bytes",
        ));
    }

    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut pending_file = options.open(&pending_path)?;
    pending_file.set_permissions(fs::Permissions::from_mode(0o600))?;
    pending_file.write_all(&serialized)?;
    pending_file.sync_all()?;
    drop(pending_file);

    fs::rename(&pending_path, &canonical_path)?;
    File::open(&journal_directory)?.sync_all()?;
    File::open(codex_home)?.sync_all()?;

    Ok(InputIdentityReservation {
        identity: ReservedInputIdentity {
            thread_id,
            incarnation: record.incarnation,
            sequence: record.sequence,
        },
        continuity,
    })
}

fn ensure_journal_directory(journal_directory: &Path) -> io::Result<()> {
    match fs::symlink_metadata(journal_directory) {
        Ok(metadata) => validate_directory_metadata(journal_directory, &metadata),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(journal_directory) {
                Ok(()) => {
                    fs::set_permissions(journal_directory, fs::Permissions::from_mode(0o700))?;
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err),
            }
            let metadata = fs::symlink_metadata(journal_directory)?;
            validate_directory_metadata(journal_directory, &metadata)?;
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn validate_directory_metadata(path: &Path, metadata: &Metadata) -> io::Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid_data(format!(
            "input identity journal path is not a real directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn read_canonical(path: &Path, thread_id: ThreadId) -> io::Result<Option<JournalRecord>> {
    let Some(metadata) = regular_file_metadata(path)? else {
        return Ok(None);
    };
    let bytes = read_bounded(path, &metadata)?;
    let record: JournalRecord = serde_json::from_slice(&bytes).map_err(|err| {
        invalid_data(format!(
            "invalid input identity journal {}: {err}",
            path.display()
        ))
    })?;
    if record.version != JOURNAL_VERSION {
        return Err(invalid_data(format!(
            "unsupported input identity journal version {} at {}",
            record.version,
            path.display()
        )));
    }
    if record.thread_id != thread_id {
        return Err(invalid_data(format!(
            "input identity journal thread id does not match {}",
            path.display()
        )));
    }
    Ok(Some(record))
}

fn recover_pending(path: &Path) -> io::Result<()> {
    let Some(metadata) = regular_file_metadata(path)? else {
        return Ok(());
    };
    // Pending content is scratch, not a second source of truth. Its bytes are only checked for
    // boundedness before deterministic removal; even a complete-looking record is not replayed.
    let _ = read_bounded(path, &metadata)?;
    fs::remove_file(path)
}

fn regular_file_metadata(path: &Path) -> io::Result<Option<Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(invalid_data(format!(
                "input identity journal path is not a regular file: {}",
                path.display()
            )))
        }
        Ok(metadata) => Ok(Some(metadata)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn read_bounded(path: &Path, metadata: &Metadata) -> io::Result<Vec<u8>> {
    if metadata.len() > MAX_JOURNAL_BYTES {
        return Err(invalid_data(format!(
            "input identity journal exceeds 4096 bytes: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(READ_LIMIT as usize);
    File::open(path)?.take(READ_LIMIT).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(invalid_data(format!(
            "input identity journal exceeds 4096 bytes: {}",
            path.display()
        )));
    }
    Ok(bytes)
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
