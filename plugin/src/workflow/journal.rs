//! Bounded, single-owner durable workflow history. This is a record of facts,
//! not a transaction log: a dispatch intent never proves whether an effect ran.
//!
//! The host chooses the directory. Callers supply redacted report snapshots;
//! specs, input values, tool arguments and private fingerprint keys are never
//! included in the record format. No record is evicted to admit new work.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use tokio::sync::{mpsc, oneshot};

const JOURNAL_FILE: &str = "events.jsonl";
const KEY_FILE: &str = "fingerprint.key";
const LOCK_FILE: &str = "owner.lock";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JournalRecord {
    pub run_id: String,
    pub revision: u64,
    pub event_seq: u64,
    pub event: String,
    pub app_instance_id: String,
    pub spec_hash: String,
    pub run_key_hash: String,
    pub principal_hash: String,
    /// Already-redacted public report. Never pass a workflow spec here.
    pub report: Value,
}

#[derive(Debug, Clone, Copy)]
pub struct JournalOptions {
    pub max_bytes: u64,
    pub max_records: usize,
    pub max_record_bytes: usize,
    pub queue_capacity: usize,
}

impl Default for JournalOptions {
    fn default() -> Self {
        Self {
            max_bytes: 64 * 1024 * 1024,
            max_records: 100_000,
            max_record_bytes: 64 * 1024,
            queue_capacity: 64,
        }
    }
}

#[derive(Debug, Clone)]
pub struct JournalError {
    code: &'static str,
    message: String,
}

impl JournalError {
    pub fn code(&self) -> &'static str {
        self.code
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            code: "persistence_unavailable",
            message: message.into(),
        }
    }

    fn corrupt(message: impl Into<String>) -> Self {
        Self {
            code: "journal_corrupt",
            message: message.into(),
        }
    }
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code(), self.message)
    }
}

impl std::error::Error for JournalError {}

struct Append {
    record: JournalRecord,
    bytes: Vec<u8>,
    ack: oneshot::Sender<Result<(), JournalError>>,
}

struct Inner {
    sender: Option<mpsc::Sender<Append>>,
    writer: Option<JoinHandle<()>>,
    key: [u8; 32],
    max_record_bytes: usize,
    poisoned: Arc<AtomicBool>,
    #[cfg(all(test, unix))]
    fault: Arc<std::sync::atomic::AtomicU8>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Closing the last sender drains outstanding acknowledgements, then
        // joining releases the OS lock before another owner can open history.
        self.sender.take();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

#[derive(Clone)]
pub struct Journal(Arc<Inner>);

impl Journal {
    pub fn open(directory: &Path) -> Result<(Self, Vec<JournalRecord>), JournalError> {
        Self::open_with_options(directory, JournalOptions::default())
    }

    /// Initialization performs bounded blocking I/O; invoke during host setup
    /// or through spawn_blocking, never on the UI event loop during execution.
    pub fn open_with_options(
        directory: &Path,
        options: JournalOptions,
    ) -> Result<(Self, Vec<JournalRecord>), JournalError> {
        if options.max_bytes == 0
            || options.max_records == 0
            || options.max_record_bytes == 0
            || options.queue_capacity == 0
        {
            return Err(JournalError::unavailable("invalid journal limits"));
        }
        private_directory(directory)?;
        let owner = private_file(&directory.join(LOCK_FILE))?;
        owner
            .try_lock()
            .map_err(|_| JournalError::unavailable("journal namespace already has an owner"))?;
        let mut file = private_file(&directory.join(JOURNAL_FILE))?;
        let key = private_key(directory, file.metadata().map_err(io_error)?.len() != 0)?;
        // Persist directory entries as well as key contents before acknowledging
        // any event, so recovery cannot silently generate a different identity.
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(io_error)?;
        let (history, mut index, mut byte_count) = read_history(&mut file, options)?;
        let mut record_count = history.len();
        let (sender, mut receiver) = mpsc::channel::<Append>(options.queue_capacity);
        let poisoned = Arc::new(AtomicBool::new(false));
        let writer_poisoned = poisoned.clone();
        #[cfg(test)]
        let fault = Arc::new(std::sync::atomic::AtomicU8::new(0));
        #[cfg(test)]
        let writer_fault = fault.clone();
        let writer = std::thread::Builder::new()
            .name("connector-workflow-journal".into())
            .spawn(move || {
                let _owner = owner;
                while let Some(append) = receiver.blocking_recv() {
                    let result = if writer_poisoned.load(Ordering::Acquire) {
                        Err(JournalError::unavailable(
                            "journal writer failed; restart requires inspection",
                        ))
                    } else if record_count >= options.max_records
                        || append.bytes.len() as u64 > options.max_bytes.saturating_sub(byte_count)
                    {
                        Err(JournalError::unavailable("journal retention limit reached"))
                    } else if let Err(error) = validate_sequence(&append.record, &index) {
                        Err(error)
                    } else {
                        #[cfg(test)]
                        let result = write_with_fault(
                            &mut file,
                            &append.bytes,
                            writer_fault.swap(0, Ordering::AcqRel),
                        );
                        #[cfg(not(test))]
                        let result = write_durable(&mut file, &append.bytes);
                        match result {
                            Ok(()) => {
                                byte_count += append.bytes.len() as u64;
                                record_count += 1;
                                remember(&append.record, &mut index);
                                Ok(())
                            }
                            Err(error) => {
                                writer_poisoned.store(true, Ordering::Release);
                                Err(error)
                            }
                        }
                    };
                    // An abandoned caller does not stop a write or allow replay.
                    let _ = append.ack.send(result);
                }
            })
            .map_err(io_error)?;
        Ok((
            Self(Arc::new(Inner {
                sender: Some(sender),
                writer: Some(writer),
                key,
                max_record_bytes: options.max_record_bytes,
                poisoned,
                #[cfg(all(test, unix))]
                fault,
            })),
            history,
        ))
    }

    /// Fingerprint the effective, default-expanded spec supplied by validation.
    /// Object order and top-level runKey do not affect identity. Transport
    /// waitMs belongs outside the spec and must never be supplied here.
    pub fn fingerprint(&self, effective_spec: &Value) -> Result<String, JournalError> {
        let mut value = effective_spec.clone();
        if let Some(object) = value.as_object_mut() {
            object.remove("runKey");
        }
        let bytes = serde_json::to_vec(&canonical(value))
            .map_err(|_| JournalError::unavailable("cannot fingerprint effective spec"))?;
        Ok(hmac_sha256(&self.0.key, &bytes))
    }

    /// A bounded queue and explicit durable ack gate every new side effect.
    /// Errors must stop dispatch; errors after dispatch do not authorize retry.
    pub async fn append(&self, record: JournalRecord) -> Result<(), JournalError> {
        if self.0.poisoned.load(Ordering::Acquire) {
            return Err(JournalError::unavailable("journal writer has failed"));
        }
        validate_record(&record)?;
        let mut buffer = BoundedBuffer {
            bytes: Vec::new(),
            remaining: self.0.max_record_bytes.saturating_sub(1),
        };
        serde_json::to_writer(&mut buffer, &record).map_err(|_| {
            JournalError::unavailable("journal record exceeds byte limit or cannot be encoded")
        })?;
        let mut bytes = buffer.bytes;
        bytes.push(b'\n');
        if bytes.len() > self.0.max_record_bytes {
            return Err(JournalError::unavailable(
                "journal record exceeds byte limit",
            ));
        }
        let (ack, result) = oneshot::channel();
        self.0
            .sender
            .as_ref()
            .ok_or_else(|| JournalError::unavailable("journal writer is closed"))?
            .try_send(Append { record, bytes, ack })
            .map_err(|_| JournalError::unavailable("journal queue is full or closed"))?;
        result
            .await
            .map_err(|_| JournalError::unavailable("journal acknowledgement unavailable"))?
    }
}

struct BoundedBuffer {
    bytes: Vec<u8>,
    remaining: usize,
}

impl Write for BoundedBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(std::io::Error::other("journal byte limit"));
        }
        self.bytes.extend_from_slice(bytes);
        self.remaining -= bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct LastRecord {
    revision: u64,
    event_seq: u64,
    spec_hash: String,
    run_key_hash: String,
    principal_hash: String,
}

type Index = HashMap<String, LastRecord>;
type History = (Vec<JournalRecord>, Index, u64);

fn remember(record: &JournalRecord, index: &mut Index) {
    index.insert(
        record.run_id.clone(),
        LastRecord {
            revision: record.revision,
            event_seq: record.event_seq,
            spec_hash: record.spec_hash.clone(),
            run_key_hash: record.run_key_hash.clone(),
            principal_hash: record.principal_hash.clone(),
        },
    );
}

fn validate_sequence(record: &JournalRecord, index: &Index) -> Result<(), JournalError> {
    if let Some(previous) = index.get(&record.run_id)
        && (record.revision <= previous.revision
            || record.event_seq <= previous.event_seq
            || record.spec_hash != previous.spec_hash
            || record.run_key_hash != previous.run_key_hash
            || record.principal_hash != previous.principal_hash)
    {
        return Err(JournalError::corrupt(
            "non-monotonic event or changed run identity",
        ));
    }
    Ok(())
}

fn validate_record(record: &JournalRecord) -> Result<(), JournalError> {
    if record.run_id.is_empty()
        || record.run_id.len() > 256
        || record.app_instance_id.is_empty()
        || record.app_instance_id.len() > 256
        || record.event.is_empty()
        || record.event.len() > 128
        || !record.report.is_object()
        || [
            &record.spec_hash,
            &record.run_key_hash,
            &record.principal_hash,
        ]
        .iter()
        .any(|hash| hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(JournalError::corrupt("invalid journal record metadata"));
    }
    Ok(())
}

fn read_history(file: &mut File, options: JournalOptions) -> Result<History, JournalError> {
    let byte_count = file.metadata().map_err(io_error)?.len();
    if byte_count > options.max_bytes {
        return Err(JournalError::unavailable("journal exceeds retention limit"));
    }
    let mut history = Vec::new();
    let mut index = Index::new();
    let mut reader = BufReader::new(file);
    loop {
        let mut line = Vec::new();
        Read::take(&mut reader, options.max_record_bytes as u64 + 1)
            .read_until(b'\n', &mut line)
            .map_err(io_error)?;
        if line.is_empty() {
            break;
        }
        if history.len() >= options.max_records || line.len() > options.max_record_bytes {
            return Err(JournalError::unavailable("journal exceeds record limit"));
        }
        if line.last() != Some(&b'\n') {
            return Err(JournalError::corrupt(
                "partial final record; no automatic repair or replay",
            ));
        }
        let record: JournalRecord = serde_json::from_slice(&line)
            .map_err(|_| JournalError::corrupt("invalid record; no automatic repair or replay"))?;
        validate_record(&record)?;
        validate_sequence(&record, &index)?;
        remember(&record, &mut index);
        history.push(record);
    }
    Ok((history, index, byte_count))
}

fn io_error(error: std::io::Error) -> JournalError {
    JournalError::unavailable(format!("journal I/O failed: {error}"))
}

#[cfg(unix)]
fn private_directory(path: &Path) -> Result<(), JournalError> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0 => Ok(()),
        Ok(_) => Err(JournalError::unavailable(
            "journal directory must be private and not a symlink",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)
                .map_err(io_error)?;
            private_directory(path)
        }
        Err(error) => Err(io_error(error)),
    }
}

#[cfg(not(unix))]
fn private_directory(_path: &Path) -> Result<(), JournalError> {
    Err(JournalError::unavailable(
        "private journal ACL enforcement is not available on this platform",
    ))
}

fn private_file(path: &Path) -> Result<File, JournalError> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        validate_private_file(&metadata)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(io_error)?;
    validate_private_file(&file.metadata().map_err(io_error)?)?;
    Ok(file)
}

fn validate_private_file(metadata: &fs::Metadata) -> Result<(), JournalError> {
    if !metadata.is_file() {
        return Err(JournalError::unavailable(
            "journal files must be regular files, not symlinks",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.permissions().mode() & 0o077 != 0 || metadata.nlink() != 1 {
            return Err(JournalError::unavailable(
                "journal files must be private and not hard-linked",
            ));
        }
    }
    Ok(())
}

fn private_key(directory: &Path, has_history: bool) -> Result<[u8; 32], JournalError> {
    let path = directory.join(KEY_FILE);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            validate_private_file(&metadata)?;
            if metadata.len() != 32 {
                return Err(JournalError::corrupt("invalid private fingerprint key"));
            }
            let mut key = [0; 32];
            private_file(&path)?
                .read_exact(&mut key)
                .map_err(io_error)?;
            Ok(key)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !has_history => {
            let mut key = [0; 32];
            key[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
            key[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(path).map_err(io_error)?;
            file.write_all(&key).map_err(io_error)?;
            file.sync_data().map_err(io_error)?;
            Ok(key)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(JournalError::corrupt(
            "history exists but private fingerprint key is missing",
        )),
        Err(error) => Err(io_error(error)),
    }
}

fn canonical(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, canonical(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(array) => Value::Array(array.into_iter().map(canonical).collect()),
        value => value,
    }
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    let hex = crate::handlers::sha256_hex(bytes);
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let digit = |byte: u8| {
                if byte <= b'9' {
                    byte - b'0'
                } else {
                    byte - b'a' + 10
                }
            };
            digit(pair[0]) * 16 + digit(pair[1])
        })
        .collect()
}

fn hmac_sha256(key: &[u8], bytes: &[u8]) -> String {
    let shortened;
    let key = if key.len() > 64 {
        shortened = sha256(key);
        shortened.as_slice()
    } else {
        key
    };
    let mut inner = vec![0x36; 64];
    let mut outer = vec![0x5c; 64];
    for (i, byte) in key.iter().enumerate() {
        inner[i] ^= byte;
        outer[i] ^= byte;
    }
    inner.extend_from_slice(bytes);
    outer.extend_from_slice(&sha256(&inner));
    crate::handlers::sha256_hex(&outer)
}

fn write_durable(file: &mut File, bytes: &[u8]) -> Result<(), JournalError> {
    file.write_all(bytes).map_err(io_error)?;
    file.sync_data().map_err(io_error)
}

#[cfg(test)]
fn write_with_fault(file: &mut File, bytes: &[u8], fault: u8) -> Result<(), JournalError> {
    match fault {
        1 => Err(JournalError::unavailable("injected disk-full error")),
        2 => {
            file.write_all(&bytes[..bytes.len() / 2])
                .map_err(io_error)?;
            Err(JournalError::unavailable("injected partial write"))
        }
        3 => {
            file.write_all(bytes).map_err(io_error)?;
            Err(JournalError::unavailable("injected sync failure"))
        }
        _ => write_durable(file, bytes),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("connector-journal-{}", uuid::Uuid::new_v4())))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn record(revision: u64) -> JournalRecord {
        JournalRecord {
            run_id: "run-1".into(),
            revision,
            event_seq: revision,
            event: "step_dispatch_intent".into(),
            app_instance_id: "instance-1".into(),
            spec_hash: "a".repeat(64),
            run_key_hash: "b".repeat(64),
            principal_hash: "c".repeat(64),
            report: json!({"status":"running","completedSteps":0}),
        }
    }

    #[test]
    fn hmac_matches_rfc_4231_vectors() {
        assert_eq!(
            hmac_sha256(&[0x0b; 20], b"Hi There"),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            ),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn fingerprint_is_canonical_private_and_stable_across_reopen() {
        let directory = TestDirectory::new();
        let (journal, _) = Journal::open(&directory.0).unwrap();
        let spec = json!({"runKey":"key-1", "inputs":{"password":"low-entropy-secret", "a":1},"mode":"strict"});
        let fingerprint = journal.fingerprint(&spec).unwrap();
        assert_eq!(fingerprint, journal.fingerprint(&json!({"mode":"strict","inputs":{"a":1,"password":"low-entropy-secret"},"runKey":"key-2"})).unwrap());
        assert_ne!(
            fingerprint,
            journal
                .fingerprint(&json!({"inputs":{"password":"changed","a":1},"mode":"strict"}))
                .unwrap()
        );
        assert_ne!(
            fingerprint,
            journal
                .fingerprint(
                    &json!({"inputs":{"password":"low-entropy-secret","a":1},"mode":"task"})
                )
                .unwrap()
        );
        drop(journal);
        let (journal, history) = Journal::open(&directory.0).unwrap();
        assert!(history.is_empty());
        assert_eq!(fingerprint, journal.fingerprint(&spec).unwrap());
        let other_directory = TestDirectory::new();
        let (other, _) = Journal::open(&other_directory.0).unwrap();
        assert_ne!(fingerprint, other.fingerprint(&spec).unwrap());
        assert!(fs::read(directory.0.join(JOURNAL_FILE)).unwrap().is_empty());
    }

    #[tokio::test]
    async fn acknowledged_intents_survive_reopen_without_becoming_success() {
        let directory = TestDirectory::new();
        let (journal, _) = Journal::open(&directory.0).unwrap();
        journal.append(record(1)).await.unwrap();
        drop(journal);
        let (_, history) = Journal::open(&directory.0).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].event, "step_dispatch_intent");
        assert_eq!(history[0].report["status"], "running");
        assert_eq!(
            fs::metadata(&directory.0).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for name in [KEY_FILE, JOURNAL_FILE, LOCK_FILE] {
            assert_eq!(
                fs::metadata(directory.0.join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn abandoning_an_acknowledgement_does_not_cancel_the_durable_record() {
        let directory = TestDirectory::new();
        let (journal, _) = Journal::open(&directory.0).unwrap();
        let record = record(1);
        let mut bytes = serde_json::to_vec(&record).unwrap();
        bytes.push(b'\n');
        let (ack, result) = oneshot::channel();
        assert!(
            journal
                .0
                .sender
                .as_ref()
                .unwrap()
                .try_send(Append { record, bytes, ack })
                .is_ok()
        );
        drop(result);
        drop(journal);
        let (_, history) = Journal::open(&directory.0).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].event, "step_dispatch_intent");
    }

    #[test]
    fn a_namespace_has_one_owner_until_all_handles_close() {
        let directory = TestDirectory::new();
        let (journal, _) = Journal::open(&directory.0).unwrap();
        let clone = journal.clone();
        assert!(Journal::open(&directory.0).is_err());
        drop(journal);
        assert!(Journal::open(&directory.0).is_err());
        drop(clone);
        assert!(Journal::open(&directory.0).is_ok());
    }

    #[tokio::test]
    async fn corrupt_and_partial_tails_are_never_repaired_silently() {
        for tail in [b"{\"runId\":".as_slice(), b"broken\n", b"{}\n"] {
            let directory = TestDirectory::new();
            let (journal, _) = Journal::open(&directory.0).unwrap();
            journal.append(record(1)).await.unwrap();
            drop(journal);
            let mut file = private_file(&directory.0.join(JOURNAL_FILE)).unwrap();
            file.write_all(tail).unwrap();
            let size = file.metadata().unwrap().len();
            assert!(
                matches!(Journal::open(&directory.0), Err(error) if error.code() == "journal_corrupt")
            );
            assert_eq!(file.metadata().unwrap().len(), size);
        }
    }

    #[tokio::test]
    async fn missing_key_never_resets_existing_deduplication_identity() {
        let directory = TestDirectory::new();
        let (journal, _) = Journal::open(&directory.0).unwrap();
        journal.append(record(1)).await.unwrap();
        drop(journal);
        fs::remove_file(directory.0.join(KEY_FILE)).unwrap();
        assert!(
            matches!(Journal::open(&directory.0), Err(error) if error.code() == "journal_corrupt")
        );
        assert!(!directory.0.join(KEY_FILE).exists());
    }

    #[tokio::test]
    async fn faulted_writes_fail_closed_for_the_lifetime_of_the_writer() {
        for fault in [1, 2, 3] {
            let directory = TestDirectory::new();
            let (journal, _) = Journal::open(&directory.0).unwrap();
            journal.append(record(1)).await.unwrap();
            journal.0.fault.store(fault, Ordering::Release);
            assert!(journal.append(record(2)).await.is_err());
            assert!(journal.append(record(3)).await.is_err());
            drop(journal);
            if fault == 2 {
                assert!(
                    matches!(Journal::open(&directory.0), Err(error) if error.code() == "journal_corrupt")
                );
            } else {
                let (_, history) = Journal::open(&directory.0).unwrap();
                assert_eq!(history.len(), if fault == 3 { 2 } else { 1 });
            }
        }
    }

    #[tokio::test]
    async fn retention_limits_refuse_work_without_evicting_history() {
        let directory = TestDirectory::new();
        let (journal, _) = Journal::open_with_options(
            &directory.0,
            JournalOptions {
                max_records: 1,
                ..Default::default()
            },
        )
        .unwrap();
        journal.append(record(1)).await.unwrap();
        assert!(journal.append(record(2)).await.is_err());
        drop(journal);
        let (_, history) = Journal::open(&directory.0).unwrap();
        assert_eq!(history.len(), 1);
        let tiny_directory = TestDirectory::new();
        let (journal, _) = Journal::open_with_options(
            &tiny_directory.0,
            JournalOptions {
                max_bytes: 10,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(journal.append(record(1)).await.is_err());
        assert!(
            fs::read(tiny_directory.0.join(JOURNAL_FILE))
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn oversized_records_are_rejected_before_entering_the_writer() {
        let directory = TestDirectory::new();
        let (journal, _) = Journal::open_with_options(
            &directory.0,
            JournalOptions {
                max_record_bytes: 1024,
                ..Default::default()
            },
        )
        .unwrap();
        let mut oversized = record(1);
        oversized.report["diagnostic"] = json!("x".repeat(4096));
        assert!(journal.append(oversized).await.is_err());
        assert!(fs::read(directory.0.join(JOURNAL_FILE)).unwrap().is_empty());
        journal.append(record(1)).await.unwrap();
        drop(journal);
        let (_, history) = Journal::open(&directory.0).unwrap();
        assert_eq!(history.len(), 1);
    }

    #[tokio::test]
    async fn valid_json_without_a_final_newline_is_an_incomplete_commit() {
        let directory = TestDirectory::new();
        let (journal, _) = Journal::open(&directory.0).unwrap();
        journal.append(record(1)).await.unwrap();
        drop(journal);
        let path = directory.0.join(JOURNAL_FILE);
        let mut bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.pop(), Some(b'\n'));
        fs::write(path, bytes).unwrap();
        assert!(
            matches!(Journal::open(&directory.0), Err(error) if error.code() == "journal_corrupt")
        );
    }

    #[tokio::test]
    async fn stale_revisions_and_changed_identity_are_rejected() {
        let directory = TestDirectory::new();
        let (journal, _) = Journal::open(&directory.0).unwrap();
        journal.append(record(1)).await.unwrap();
        assert!(journal.append(record(1)).await.is_err());
        let mut changed = record(2);
        changed.spec_hash = "d".repeat(64);
        assert!(journal.append(changed).await.is_err());
        journal.append(record(2)).await.unwrap();
        drop(journal);
        let (_, history) = Journal::open(&directory.0).unwrap();
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn public_directories_and_symlink_files_are_rejected() {
        let directory = TestDirectory::new();
        fs::create_dir(&directory.0).unwrap();
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Journal::open(&directory.0).is_err());
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o700)).unwrap();
        let external = TestDirectory::new();
        fs::create_dir(&external.0).unwrap();
        let file = external.0.join("external");
        fs::write(&file, b"unchanged").unwrap();
        symlink(&file, directory.0.join(JOURNAL_FILE)).unwrap();
        assert!(Journal::open(&directory.0).is_err());
        assert_eq!(fs::read(file).unwrap(), b"unchanged");
    }
}
