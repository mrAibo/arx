//! Read-only S3 object and bucket/prefix inspection.
//!
//! The inspector deliberately reuses the concrete `S3Provider` and its lazy
//! client. It never creates a second client cache, registry, scheduler, or job
//! runtime. Aggregate scans are page-at-a-time and bounded in memory.

use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::vfs::Location;
use crate::vfs::s3::{S3BucketRef, S3ObjectRef, S3PrefixRef, S3Provider};

const INSPECT_PAGE_SIZE: i32 = 1000;
const TOP_OBJECTS_LIMIT: usize = 20;
const TOP_PREFIXES_LIMIT: usize = 20;
const PREFIX_CARDINALITY_LIMIT: usize = 2048;
const STORAGE_CLASS_CARDINALITY_LIMIT: usize = 64;
const DAY_MS: u64 = 86_400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S3EvidenceSource {
    LiveScan,
    StorageLens,
    Inventory,
    OtherProvider,
    Unavailable,
}

impl S3EvidenceSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::LiveScan => "LiveScan",
            Self::StorageLens => "StorageLens",
            Self::Inventory => "Inventory",
            Self::OtherProvider => "OtherProvider",
            Self::Unavailable => "Unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Evidence<T> {
    pub source: S3EvidenceSource,
    pub value: Option<T>,
    pub note: Option<String>,
}

impl<T> S3Evidence<T> {
    fn live(value: T) -> Self {
        Self {
            source: S3EvidenceSource::LiveScan,
            value: Some(value),
            note: None,
        }
    }

    fn unavailable(note: impl Into<String>) -> Self {
        Self {
            source: S3EvidenceSource::Unavailable,
            value: None,
            note: Some(note.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S3InspectionScope {
    Bucket(S3BucketRef),
    Prefix(S3PrefixRef),
}

impl S3InspectionScope {
    pub fn target_id(&self) -> &str {
        match self {
            Self::Bucket(reference) => &reference.target,
            Self::Prefix(reference) => &reference.target,
        }
    }

    pub fn bucket(&self) -> &str {
        match self {
            Self::Bucket(reference) => &reference.bucket,
            Self::Prefix(reference) => &reference.bucket,
        }
    }

    pub fn wire_prefix(&self) -> &str {
        match self {
            Self::Bucket(_) => "",
            Self::Prefix(reference) => &reference.prefix,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S3InspectionTarget {
    Object(S3ObjectRef),
    Scope(S3InspectionScope),
}

impl S3InspectionTarget {
    pub fn target_id(&self) -> &str {
        match self {
            Self::Object(reference) => &reference.target,
            Self::Scope(scope) => scope.target_id(),
        }
    }
}

/// Convert the current typed S3 navigation location into an exact inspection
/// scope. A non-empty navigation prefix receives exactly one protocol `/`,
/// matching the existing ListObjectsV2 navigation contract. No trimming or
/// filesystem normalization is performed.
pub fn scope_from_location(location: &Location) -> Option<S3InspectionScope> {
    let Location::S3 {
        target,
        bucket: Some(bucket),
        prefix,
    } = location
    else {
        return None;
    };
    if prefix.is_empty() {
        Some(S3InspectionScope::Bucket(S3BucketRef {
            target: target.clone(),
            bucket: bucket.clone(),
        }))
    } else {
        Some(S3InspectionScope::Prefix(S3PrefixRef {
            target: target.clone(),
            bucket: bucket.clone(),
            prefix: format!("{prefix}/"),
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3ObjectSnapshot {
    pub evidence: S3EvidenceSource,
    pub observed_at_unix_ms: u64,
    pub target: String,
    pub endpoint_override: Option<String>,
    pub bucket: String,
    pub key: String,
    pub size: Option<u64>,
    pub last_modified_unix_ms: Option<u64>,
    pub etag: Option<String>,
    pub content_type: Option<String>,
    pub storage_class: Option<String>,
    pub metadata: BTreeMap<String, String>,
    pub version_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3LargestObject {
    pub key: String,
    pub size: u64,
    pub last_modified_unix_ms: Option<u64>,
    pub etag: Option<String>,
    pub storage_class: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3PrefixAggregate {
    pub prefix: String,
    pub object_count: u64,
    pub logical_bytes: u128,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct S3AgeDistribution {
    pub under_30_days: u64,
    pub days_30_to_89: u64,
    pub days_90_to_364: u64,
    pub days_365_plus: u64,
    pub unavailable: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3ScanSnapshot {
    pub evidence: S3EvidenceSource,
    pub observed_at_unix_ms: u64,
    pub scope: S3InspectionScope,
    pub complete: bool,
    pub cancelled: bool,
    pub terminal_note: Option<String>,
    pub pages_seen: u64,
    pub object_count: u64,
    pub total_logical_bytes: u128,
    pub objects_without_size: u64,
    pub largest_objects: Vec<S3LargestObject>,
    pub largest_prefixes: S3Evidence<Vec<S3PrefixAggregate>>,
    pub age_distribution: S3AgeDistribution,
    pub storage_classes: BTreeMap<String, u64>,
    pub objects_without_storage_class: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S3InspectorSnapshot {
    Object(S3ObjectSnapshot),
    Scan(S3ScanSnapshot),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct S3ScanProgress {
    pub pages_seen: u64,
    pub objects_seen: u64,
    pub logical_bytes_seen: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S3ScanOutcome {
    Complete(S3ScanSnapshot),
    Cancelled(S3ScanSnapshot),
    Partial {
        snapshot: S3ScanSnapshot,
        error: String,
    },
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn validate_provider_target(provider: &S3Provider, target: &str, bucket: &str) -> io::Result<()> {
    if target != provider.target.id {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "S3 inspector target mismatch",
        ));
    }
    if bucket.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "S3 inspector requires a non-empty bucket",
        ));
    }
    if provider
        .target
        .bucket
        .as_deref()
        .is_some_and(|bound| bound != bucket)
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "S3 inspector bucket escape rejected",
        ));
    }
    Ok(())
}

pub async fn inspect_object(
    provider: Arc<S3Provider>,
    object: S3ObjectRef,
    cancellation: Arc<AtomicBool>,
) -> io::Result<S3ObjectSnapshot> {
    validate_provider_target(&provider, &object.target, &object.bucket)?;
    if object.key.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "S3 object inspection requires a non-empty key",
        ));
    }
    if cancellation.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "S3 object inspection cancelled before request",
        ));
    }

    let observed_at_unix_ms = now_unix_ms();
    let client = provider.client().await?;
    let head = client
        .head_object()
        .bucket(&object.bucket)
        .key(&object.key)
        .send()
        .await
        .map_err(|_| io::Error::other("S3 HeadObject inspection request failed"))?;

    let size = head
        .content_length()
        .and_then(|value| (value >= 0).then_some(value as u64));
    let last_modified_unix_ms = head
        .last_modified()
        .and_then(|value| value.to_millis().ok())
        .and_then(|value| (value >= 0).then_some(value as u64));
    let metadata = head
        .metadata()
        .map(|metadata| {
            metadata
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();

    Ok(S3ObjectSnapshot {
        evidence: S3EvidenceSource::LiveScan,
        observed_at_unix_ms,
        target: object.target,
        endpoint_override: provider.target.endpoint_url.clone(),
        bucket: object.bucket,
        key: object.key,
        size,
        last_modified_unix_ms,
        etag: head.e_tag().map(str::to_owned),
        content_type: head.content_type().map(str::to_owned),
        storage_class: head.storage_class().map(|value| value.as_str().to_string()),
        metadata,
        version_id: head.version_id().map(str::to_owned),
    })
}

#[derive(Debug, Clone, Default)]
struct PrefixTally {
    object_count: u64,
    logical_bytes: u128,
}

struct ScanAccumulator {
    scope: S3InspectionScope,
    observed_at_unix_ms: u64,
    pages_seen: u64,
    object_count: u64,
    total_logical_bytes: u128,
    objects_without_size: u64,
    largest_objects: Vec<S3LargestObject>,
    prefix_tallies: BTreeMap<String, PrefixTally>,
    prefix_cardinality_exceeded: bool,
    age_distribution: S3AgeDistribution,
    storage_classes: BTreeMap<String, u64>,
    objects_without_storage_class: u64,
}

impl ScanAccumulator {
    fn new(scope: S3InspectionScope, observed_at_unix_ms: u64) -> Self {
        Self {
            scope,
            observed_at_unix_ms,
            pages_seen: 0,
            object_count: 0,
            total_logical_bytes: 0,
            objects_without_size: 0,
            largest_objects: Vec::new(),
            prefix_tallies: BTreeMap::new(),
            prefix_cardinality_exceeded: false,
            age_distribution: S3AgeDistribution::default(),
            storage_classes: BTreeMap::new(),
            objects_without_storage_class: 0,
        }
    }

    fn progress(&self) -> S3ScanProgress {
        S3ScanProgress {
            pages_seen: self.pages_seen,
            objects_seen: self.object_count,
            logical_bytes_seen: self.total_logical_bytes,
        }
    }

    fn ingest(
        &mut self,
        key: &str,
        size: Option<u64>,
        last_modified_unix_ms: Option<u64>,
        etag: Option<&str>,
        storage_class: Option<&str>,
    ) -> io::Result<()> {
        if !key.starts_with(self.scope.wire_prefix()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "S3 inspection response contained a key outside the requested prefix",
            ));
        }

        self.object_count = self.object_count.saturating_add(1);
        if let Some(size) = size {
            self.total_logical_bytes = self
                .total_logical_bytes
                .checked_add(u128::from(size))
                .ok_or_else(|| io::Error::other("S3 logical-byte total overflow"))?;
            self.largest_objects.push(S3LargestObject {
                key: key.to_string(),
                size,
                last_modified_unix_ms,
                etag: etag.map(str::to_owned),
                storage_class: storage_class.map(str::to_owned),
            });
            self.largest_objects.sort_by(|left, right| {
                right
                    .size
                    .cmp(&left.size)
                    .then_with(|| left.key.cmp(&right.key))
            });
            self.largest_objects.truncate(TOP_OBJECTS_LIMIT);
        } else {
            self.objects_without_size = self.objects_without_size.saturating_add(1);
        }

        if !self.prefix_cardinality_exceeded {
            let relative = key
                .strip_prefix(self.scope.wire_prefix())
                .expect("prefix checked above");
            if let Some(index) = relative.find('/') {
                let child_prefix = format!("{}{}", self.scope.wire_prefix(), &relative[..=index]);
                if !self.prefix_tallies.contains_key(&child_prefix)
                    && self.prefix_tallies.len() >= PREFIX_CARDINALITY_LIMIT
                {
                    self.prefix_cardinality_exceeded = true;
                    self.prefix_tallies.clear();
                } else {
                    let tally = self.prefix_tallies.entry(child_prefix).or_default();
                    tally.object_count = tally.object_count.saturating_add(1);
                    if let Some(size) = size {
                        tally.logical_bytes = tally
                            .logical_bytes
                            .checked_add(u128::from(size))
                            .ok_or_else(|| io::Error::other("S3 prefix-byte total overflow"))?;
                    }
                }
            }
        }

        match last_modified_unix_ms {
            Some(modified) => {
                let age = self.observed_at_unix_ms.saturating_sub(modified);
                if age < 30 * DAY_MS {
                    self.age_distribution.under_30_days += 1;
                } else if age < 90 * DAY_MS {
                    self.age_distribution.days_30_to_89 += 1;
                } else if age < 365 * DAY_MS {
                    self.age_distribution.days_90_to_364 += 1;
                } else {
                    self.age_distribution.days_365_plus += 1;
                }
            }
            None => self.age_distribution.unavailable += 1,
        }

        match storage_class {
            Some(class) => {
                if !self.storage_classes.contains_key(class)
                    && self.storage_classes.len() >= STORAGE_CLASS_CARDINALITY_LIMIT
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "S3 storage-class cardinality exceeds bounded inspector limit",
                    ));
                }
                *self.storage_classes.entry(class.to_string()).or_insert(0) += 1;
            }
            None => self.objects_without_storage_class += 1,
        }
        Ok(())
    }

    fn finish(
        self,
        complete: bool,
        cancelled: bool,
        terminal_note: Option<String>,
    ) -> S3ScanSnapshot {
        let largest_prefixes = if self.prefix_cardinality_exceeded {
            S3Evidence::unavailable(format!(
                "more than {PREFIX_CARDINALITY_LIMIT} immediate prefixes; exact ranking not retained"
            ))
        } else {
            let mut prefixes = self
                .prefix_tallies
                .into_iter()
                .map(|(prefix, tally)| S3PrefixAggregate {
                    prefix,
                    object_count: tally.object_count,
                    logical_bytes: tally.logical_bytes,
                })
                .collect::<Vec<_>>();
            prefixes.sort_by(|left, right| {
                right
                    .logical_bytes
                    .cmp(&left.logical_bytes)
                    .then_with(|| right.object_count.cmp(&left.object_count))
                    .then_with(|| left.prefix.cmp(&right.prefix))
            });
            prefixes.truncate(TOP_PREFIXES_LIMIT);
            S3Evidence::live(prefixes)
        };

        S3ScanSnapshot {
            evidence: S3EvidenceSource::LiveScan,
            observed_at_unix_ms: self.observed_at_unix_ms,
            scope: self.scope,
            complete,
            cancelled,
            terminal_note,
            pages_seen: self.pages_seen,
            object_count: self.object_count,
            total_logical_bytes: self.total_logical_bytes,
            objects_without_size: self.objects_without_size,
            largest_objects: self.largest_objects,
            largest_prefixes,
            age_distribution: self.age_distribution,
            storage_classes: self.storage_classes,
            objects_without_storage_class: self.objects_without_storage_class,
        }
    }
}

fn next_token(
    consumed: Option<&str>,
    is_truncated: Option<bool>,
    returned: Option<&str>,
) -> io::Result<Option<String>> {
    match is_truncated {
        Some(false) if returned.is_none() => Ok(None),
        Some(false) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "S3 inspection pagination contradiction",
        )),
        Some(true) => match returned {
            Some(token) if !token.is_empty() && Some(token) != consumed => {
                Ok(Some(token.to_string()))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "S3 inspection pagination token did not advance",
            )),
        },
        None => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "S3 inspection pagination missing IsTruncated",
        )),
    }
}

pub async fn scan_scope<F>(
    provider: Arc<S3Provider>,
    scope: S3InspectionScope,
    cancellation: Arc<AtomicBool>,
    mut on_progress: F,
) -> io::Result<S3ScanOutcome>
where
    F: FnMut(S3ScanProgress),
{
    validate_provider_target(&provider, scope.target_id(), scope.bucket())?;
    if matches!(&scope, S3InspectionScope::Prefix(reference) if !reference.prefix.ends_with('/')) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "S3 prefix inspection requires an exact delimiter-terminated prefix",
        ));
    }

    let observed_at_unix_ms = now_unix_ms();
    let mut accumulator = ScanAccumulator::new(scope.clone(), observed_at_unix_ms);
    let client = provider.client().await?;
    let bucket = scope.bucket().to_string();
    let wire_prefix = scope.wire_prefix().to_string();
    let mut continuation: Option<String> = None;

    loop {
        if cancellation.load(Ordering::Relaxed) {
            return Ok(S3ScanOutcome::Cancelled(accumulator.finish(
                false,
                true,
                Some("cancelled between ListObjectsV2 pages".into()),
            )));
        }

        let consumed = continuation.clone();
        let mut request = client
            .list_objects_v2()
            .bucket(&bucket)
            .prefix(&wire_prefix)
            .max_keys(INSPECT_PAGE_SIZE);
        if let Some(token) = consumed.as_deref() {
            request = request.continuation_token(token);
        }

        let output = match request.send().await {
            Ok(output) => output,
            Err(_) if accumulator.pages_seen > 0 => {
                let error = "S3 ListObjectsV2 inspection request failed".to_string();
                let snapshot = accumulator.finish(false, false, Some(error.clone()));
                return Ok(S3ScanOutcome::Partial { snapshot, error });
            }
            Err(_) => {
                return Err(io::Error::other(
                    "S3 ListObjectsV2 inspection request failed",
                ));
            }
        };

        accumulator.pages_seen = accumulator.pages_seen.saturating_add(1);
        for object in output.contents() {
            let Some(key) = object.key() else {
                continue;
            };
            let size = object
                .size()
                .and_then(|value| (value >= 0).then_some(value as u64));
            let last_modified_unix_ms = object
                .last_modified()
                .and_then(|value| value.to_millis().ok())
                .and_then(|value| (value >= 0).then_some(value as u64));
            let storage_class = object.storage_class().map(|value| value.as_str());
            if let Err(error) = accumulator.ingest(
                key,
                size,
                last_modified_unix_ms,
                object.e_tag(),
                storage_class,
            ) {
                if accumulator.pages_seen > 1 || accumulator.object_count > 0 {
                    let message = error.to_string();
                    let snapshot = accumulator.finish(false, false, Some(message.clone()));
                    return Ok(S3ScanOutcome::Partial {
                        snapshot,
                        error: message,
                    });
                }
                return Err(error);
            }
        }
        on_progress(accumulator.progress());

        if cancellation.load(Ordering::Relaxed) {
            return Ok(S3ScanOutcome::Cancelled(accumulator.finish(
                false,
                true,
                Some("cancelled after a completed ListObjectsV2 page".into()),
            )));
        }

        continuation = match next_token(
            consumed.as_deref(),
            output.is_truncated(),
            output.next_continuation_token(),
        ) {
            Ok(next) => next,
            Err(error) => {
                let message = error.to_string();
                let snapshot = accumulator.finish(false, false, Some(message.clone()));
                return Ok(S3ScanOutcome::Partial {
                    snapshot,
                    error: message,
                });
            }
        };
        if continuation.is_none() {
            return Ok(S3ScanOutcome::Complete(
                accumulator.finish(true, false, None),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_model_keeps_all_frozen_classes_distinct() {
        let all = [
            S3EvidenceSource::LiveScan,
            S3EvidenceSource::StorageLens,
            S3EvidenceSource::Inventory,
            S3EvidenceSource::OtherProvider,
            S3EvidenceSource::Unavailable,
        ];
        assert_eq!(all.len(), 5);
        assert_ne!(all[0], all[4]);
    }

    #[test]
    fn location_scope_preserves_navigation_delimiter_semantics() {
        let location = Location::S3 {
            target: "t".into(),
            bucket: Some("b".into()),
            prefix: "foo//".into(),
        };
        assert_eq!(
            scope_from_location(&location),
            Some(S3InspectionScope::Prefix(S3PrefixRef {
                target: "t".into(),
                bucket: "b".into(),
                prefix: "foo///".into(),
            }))
        );
    }

    #[test]
    fn accumulator_is_bounded_and_truthful_when_prefix_cardinality_exceeds_cap() {
        let scope = S3InspectionScope::Bucket(S3BucketRef {
            target: "t".into(),
            bucket: "b".into(),
        });
        let mut accumulator = ScanAccumulator::new(scope, 400 * DAY_MS);
        for index in 0..=PREFIX_CARDINALITY_LIMIT {
            accumulator
                .ingest(
                    &format!("p{index}/file"),
                    Some(index as u64 + 1),
                    Some(10 * DAY_MS),
                    None,
                    Some("STANDARD"),
                )
                .unwrap();
        }
        let snapshot = accumulator.finish(true, false, None);
        assert_eq!(snapshot.object_count, PREFIX_CARDINALITY_LIMIT as u64 + 1);
        assert_eq!(snapshot.largest_objects.len(), TOP_OBJECTS_LIMIT);
        assert_eq!(
            snapshot.largest_prefixes.source,
            S3EvidenceSource::Unavailable
        );
        assert!(snapshot.largest_prefixes.value.is_none());
    }

    #[test]
    fn accumulator_fails_closed_when_storage_class_cardinality_exceeds_cap() {
        let scope = S3InspectionScope::Bucket(S3BucketRef {
            target: "t".into(),
            bucket: "b".into(),
        });
        let mut accumulator = ScanAccumulator::new(scope, 400 * DAY_MS);
        for index in 0..STORAGE_CLASS_CARDINALITY_LIMIT {
            let class = format!("CLASS-{index}");
            accumulator
                .ingest(
                    &format!("file-{index}"),
                    Some(1),
                    None,
                    None,
                    Some(&class),
                )
                .unwrap();
        }
        let error = accumulator
            .ingest("overflow", Some(1), None, None, Some("CLASS-overflow"))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("storage-class cardinality"));
        assert_eq!(
            accumulator.storage_classes.len(),
            STORAGE_CLASS_CARDINALITY_LIMIT
        );
    }

    #[test]
    fn accumulator_reports_age_storage_class_and_top_objects_from_observed_facts() {
        let scope = S3InspectionScope::Prefix(S3PrefixRef {
            target: "t".into(),
            bucket: "b".into(),
            prefix: "root/".into(),
        });
        let now = 400 * DAY_MS;
        let mut accumulator = ScanAccumulator::new(scope, now);
        accumulator
            .ingest(
                "root/a.bin",
                Some(5),
                Some(now - 10 * DAY_MS),
                Some("e1"),
                Some("STANDARD"),
            )
            .unwrap();
        accumulator
            .ingest(
                "root/child/b.bin",
                Some(9),
                Some(now - 100 * DAY_MS),
                None,
                Some("GLACIER"),
            )
            .unwrap();
        accumulator
            .ingest("root/unknown", None, None, None, None)
            .unwrap();
        let snapshot = accumulator.finish(true, false, None);
        assert_eq!(snapshot.object_count, 3);
        assert_eq!(snapshot.total_logical_bytes, 14);
        assert_eq!(snapshot.objects_without_size, 1);
        assert_eq!(snapshot.largest_objects[0].key, "root/child/b.bin");
        assert_eq!(snapshot.age_distribution.under_30_days, 1);
        assert_eq!(snapshot.age_distribution.days_90_to_364, 1);
        assert_eq!(snapshot.age_distribution.unavailable, 1);
        assert_eq!(snapshot.storage_classes.get("STANDARD"), Some(&1));
        assert_eq!(snapshot.objects_without_storage_class, 1);
        let prefixes = snapshot.largest_prefixes.value.unwrap();
        assert_eq!(prefixes[0].prefix, "root/child/");
        assert_eq!(prefixes[0].logical_bytes, 9);
    }

    #[test]
    fn pagination_rejects_missing_empty_and_nonadvancing_tokens() {
        assert_eq!(next_token(None, Some(false), None).unwrap(), None);
        assert_eq!(
            next_token(None, Some(true), Some("next")).unwrap(),
            Some("next".into())
        );
        assert!(next_token(Some("same"), Some(true), Some("same")).is_err());
        assert!(next_token(None, Some(true), None).is_err());
        assert!(next_token(None, None, None).is_err());
    }
}
