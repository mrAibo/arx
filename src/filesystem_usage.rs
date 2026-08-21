//! Linux-native filesystem/mount usage snapshot (`df++` core).
//!
//! Mount topology comes directly from `/proc/self/mountinfo`; capacity and
//! inode statistics come from `statvfs(3)`.  The module is deliberately
//! observation-only: it never mounts, unmounts, remounts, cleans, resizes, or
//! otherwise mutates a filesystem.

use std::ffi::{CString, OsString};
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

const MOUNTINFO_PATH: &str = "/proc/self/mountinfo";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MountCategory {
    Local,
    Network,
    Fuse,
    Special,
}

impl MountCategory {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Network => "network",
            Self::Fuse => "fuse",
            Self::Special => "special",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountInfo {
    pub mount_id: u32,
    pub parent_id: u32,
    pub major: u32,
    pub minor: u32,
    pub root: PathBuf,
    pub mount_point: PathBuf,
    pub mount_options: Vec<String>,
    pub optional_fields: Vec<String>,
    pub fs_type: String,
    pub mount_source: OsString,
    pub super_options: Vec<String>,
}

impl MountInfo {
    pub fn category(&self) -> MountCategory {
        classify_filesystem(&self.fs_type)
    }

    pub fn read_only(&self) -> bool {
        self.mount_options.iter().any(|option| option == "ro")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountStats {
    pub total_bytes: u128,
    pub used_bytes: Option<u128>,
    pub free_bytes: u128,
    pub available_bytes: u128,
    pub reserved_bytes: Option<u128>,
    /// Usage in tenths of one percent: 1000 == 100.0%.
    pub usage_tenths_percent: Option<u16>,
    pub total_inodes: u128,
    pub used_inodes: Option<u128>,
    pub free_inodes: u128,
    pub available_inodes: u128,
    /// Inode usage in tenths of one percent: 1000 == 100.0%.
    pub inode_usage_tenths_percent: Option<u16>,
}

impl MountStats {
    fn from_counts(counts: RawStatCounts) -> Self {
        let fragment_size = if counts.fragment_size == 0 {
            counts.block_size
        } else {
            counts.fragment_size
        } as u128;

        let total_bytes = u128::from(counts.blocks) * fragment_size;
        let free_bytes = u128::from(counts.blocks_free) * fragment_size;
        let available_bytes = u128::from(counts.blocks_available) * fragment_size;
        let used_bytes = total_bytes.checked_sub(free_bytes);
        let reserved_bytes = free_bytes.checked_sub(available_bytes);
        let usage_tenths_percent = percent_tenths(used_bytes, total_bytes);

        let total_inodes = u128::from(counts.files);
        let free_inodes = u128::from(counts.files_free);
        let available_inodes = u128::from(counts.files_available);
        let used_inodes = total_inodes.checked_sub(free_inodes);
        let inode_usage_tenths_percent = percent_tenths(used_inodes, total_inodes);

        Self {
            total_bytes,
            used_bytes,
            free_bytes,
            available_bytes,
            reserved_bytes,
            usage_tenths_percent,
            total_inodes,
            used_inodes,
            free_inodes,
            available_inodes,
            inode_usage_tenths_percent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountStatsState {
    Available(MountStats),
    /// Deliberately not probed: observing an autofs entry must not trigger it.
    SkippedAutoFs,
    /// Deliberately not probed: stale network mounts can block statvfs.
    SkippedNetwork,
    /// The mount remains visible, but live capacity truth could not be read.
    Unavailable(String),
}

impl MountStatsState {
    pub const fn available(&self) -> Option<&MountStats> {
        match self {
            Self::Available(stats) => Some(stats),
            Self::SkippedAutoFs | Self::SkippedNetwork | Self::Unavailable(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountRecord {
    pub info: MountInfo,
    pub category: MountCategory,
    pub read_only: bool,
    pub stats: MountStatsState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountInfoParseError {
    pub line_number: usize,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MountSnapshot {
    pub mounts: Vec<MountRecord>,
    pub parse_errors: Vec<MountInfoParseError>,
}

impl MountSnapshot {
    pub fn is_partial(&self) -> bool {
        !self.parse_errors.is_empty()
            || self
                .mounts
                .iter()
                .any(|mount| matches!(mount.stats, MountStatsState::Unavailable(_)))
    }

    pub fn unavailable_count(&self) -> usize {
        self.mounts
            .iter()
            .filter(|mount| matches!(mount.stats, MountStatsState::Unavailable(_)))
            .count()
    }

    pub fn intentionally_skipped_count(&self) -> usize {
        self.mounts
            .iter()
            .filter(|mount| {
                matches!(
                    mount.stats,
                    MountStatsState::SkippedAutoFs | MountStatsState::SkippedNetwork
                )
            })
            .count()
    }
}

#[derive(Debug, Clone, Copy)]
struct RawStatCounts {
    block_size: u64,
    fragment_size: u64,
    blocks: u64,
    blocks_free: u64,
    blocks_available: u64,
    files: u64,
    files_free: u64,
    files_available: u64,
}

pub fn collect_mount_snapshot() -> io::Result<MountSnapshot> {
    let bytes = std::fs::read(MOUNTINFO_PATH)?;
    Ok(collect_mount_snapshot_from_bytes_with_probe(
        &bytes,
        |mount| statvfs_mount_point(&mount.mount_point),
    ))
}

fn collect_mount_snapshot_from_bytes_with_probe<F>(input: &[u8], mut probe: F) -> MountSnapshot
where
    F: FnMut(&MountInfo) -> Result<MountStats, String>,
{
    let (mounts, parse_errors) = parse_mountinfo(input);
    let mounts = mounts
        .into_iter()
        .map(|info| {
            let category = info.category();
            let read_only = info.read_only();
            let stats = if info.fs_type.eq_ignore_ascii_case("autofs") {
                MountStatsState::SkippedAutoFs
            } else if category == MountCategory::Network {
                MountStatsState::SkippedNetwork
            } else {
                match probe(&info) {
                    Ok(stats) => MountStatsState::Available(stats),
                    Err(error) => MountStatsState::Unavailable(error),
                }
            };
            MountRecord {
                info,
                category,
                read_only,
                stats,
            }
        })
        .collect();

    MountSnapshot {
        mounts,
        parse_errors,
    }
}

pub fn parse_mountinfo(input: &[u8]) -> (Vec<MountInfo>, Vec<MountInfoParseError>) {
    let mut mounts = Vec::new();
    let mut errors = Vec::new();

    for (line_index, line) in input.split(|byte| *byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        match parse_mountinfo_line(line) {
            Ok(mount) => mounts.push(mount),
            Err(message) => errors.push(MountInfoParseError {
                line_number: line_index + 1,
                message,
            }),
        }
    }

    (mounts, errors)
}

fn parse_mountinfo_line(line: &[u8]) -> Result<MountInfo, String> {
    let fields: Vec<&[u8]> = line
        .split(|byte| *byte == b' ')
        .filter(|field| !field.is_empty())
        .collect();
    let separator = fields
        .iter()
        .position(|field| *field == b"-")
        .ok_or_else(|| "missing mountinfo separator '-'".to_string())?;

    if separator < 6 {
        return Err("mountinfo line has fewer than 6 mandatory fields".into());
    }
    if fields.len() < separator + 4 {
        return Err("mountinfo line is missing filesystem fields after '-'".into());
    }

    let mount_id = parse_u32(fields[0], "mount id")?;
    let parent_id = parse_u32(fields[1], "parent id")?;
    let (major, minor) = parse_device(fields[2])?;
    let root = PathBuf::from(OsString::from_vec(decode_mount_field(fields[3])));
    let mount_point = PathBuf::from(OsString::from_vec(decode_mount_field(fields[4])));
    let mount_options = split_csv_lossy(fields[5]);
    let optional_fields = fields[6..separator]
        .iter()
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect();
    let fs_type = String::from_utf8_lossy(fields[separator + 1]).into_owned();
    let mount_source = OsString::from_vec(decode_mount_field(fields[separator + 2]));
    let super_options = split_csv_lossy(fields[separator + 3]);

    Ok(MountInfo {
        mount_id,
        parent_id,
        major,
        minor,
        root,
        mount_point,
        mount_options,
        optional_fields,
        fs_type,
        mount_source,
        super_options,
    })
}

fn parse_u32(bytes: &[u8], field: &str) -> Result<u32, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| format!("{field} is not ASCII"))?;
    text.parse::<u32>()
        .map_err(|_| format!("invalid {field}: {text}"))
}

fn parse_device(bytes: &[u8]) -> Result<(u32, u32), String> {
    let mut parts = bytes.splitn(2, |byte| *byte == b':');
    let major = parts
        .next()
        .ok_or_else(|| "missing device major".to_string())?;
    let minor = parts
        .next()
        .ok_or_else(|| "missing device minor".to_string())?;
    Ok((
        parse_u32(major, "device major")?,
        parse_u32(minor, "device minor")?,
    ))
}

fn split_csv_lossy(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|byte| *byte == b',')
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect()
}

/// Decode the octal escapes used by procfs mount fields. Invalid/non-octal
/// escape sequences are preserved byte-for-byte rather than silently changed.
fn decode_mount_field(field: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::with_capacity(field.len());
    let mut index = 0;
    while index < field.len() {
        if field[index] == b'\\' && index + 3 < field.len() {
            let digits = &field[index + 1..index + 4];
            if digits.iter().all(|digit| (b'0'..=b'7').contains(digit)) {
                let value = (digits[0] - b'0') * 64
                    + (digits[1] - b'0') * 8
                    + (digits[2] - b'0');
                decoded.push(value);
                index += 4;
                continue;
            }
        }
        decoded.push(field[index]);
        index += 1;
    }
    decoded
}

pub fn classify_filesystem(fs_type: &str) -> MountCategory {
    let fs = fs_type.to_ascii_lowercase();

    const NETWORK: &[&str] = &[
        "nfs",
        "nfs4",
        "cifs",
        "smbfs",
        "smb3",
        "ceph",
        "afs",
        "coda",
        "9p",
        "lustre",
        "glusterfs",
    ];
    const REMOTE_FUSE_PREFIXES: &[&str] = &[
        "fuse.sshfs",
        "fuse.rclone",
        "fuse.s3fs",
        "fuse.gcsfuse",
        "fuse.goofys",
    ];
    const SPECIAL: &[&str] = &[
        "proc",
        "sysfs",
        "devtmpfs",
        "devpts",
        "tmpfs",
        "cgroup",
        "cgroup2",
        "pstore",
        "securityfs",
        "debugfs",
        "tracefs",
        "configfs",
        "hugetlbfs",
        "mqueue",
        "bpf",
        "binfmt_misc",
        "efivarfs",
        "ramfs",
        "autofs",
        "rpc_pipefs",
        "nsfs",
        "selinuxfs",
        "smackfs",
    ];

    if NETWORK.contains(&fs.as_str())
        || REMOTE_FUSE_PREFIXES
            .iter()
            .any(|prefix| fs.starts_with(prefix))
    {
        MountCategory::Network
    } else if fs.starts_with("fuse") {
        MountCategory::Fuse
    } else if SPECIAL.contains(&fs.as_str()) {
        MountCategory::Special
    } else {
        MountCategory::Local
    }
}

fn statvfs_mount_point(path: &Path) -> Result<MountStats, String> {
    let path_bytes = path.as_os_str().as_bytes();
    let c_path = CString::new(path_bytes)
        .map_err(|_| format!("mount point contains NUL: {}", path.display()))?;
    let mut raw = MaybeUninit::<libc::statvfs>::uninit();

    // SAFETY: c_path is NUL-terminated and points to live storage for this call;
    // raw is initialized by libc only when statvfs returns success.
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), raw.as_mut_ptr()) };
    if rc != 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    // SAFETY: successful statvfs initialized the complete output structure.
    let raw = unsafe { raw.assume_init() };

    Ok(MountStats::from_counts(RawStatCounts {
        block_size: raw.f_bsize as u64,
        fragment_size: raw.f_frsize as u64,
        blocks: raw.f_blocks as u64,
        blocks_free: raw.f_bfree as u64,
        blocks_available: raw.f_bavail as u64,
        files: raw.f_files as u64,
        files_free: raw.f_ffree as u64,
        files_available: raw.f_favail as u64,
    }))
}

fn percent_tenths(used: Option<u128>, total: u128) -> Option<u16> {
    let used = used?;
    if total == 0 {
        return None;
    }
    let tenths = used.checked_mul(1000)?.checked_div(total)?;
    u16::try_from(tenths).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const SIMPLE: &[u8] = b"36 25 8:1 / / rw,relatime shared:1 master:2 - ext4 /dev/sda1 rw,errors=remount-ro\n";

    fn stats() -> MountStats {
        MountStats::from_counts(RawStatCounts {
            block_size: 4096,
            fragment_size: 4096,
            blocks: 100,
            blocks_free: 40,
            blocks_available: 30,
            files: 50,
            files_free: 20,
            files_available: 20,
        })
    }

    #[test]
    fn parser_handles_optional_fields() {
        let (mounts, errors) = parse_mountinfo(SIMPLE);
        assert!(errors.is_empty());
        assert_eq!(mounts.len(), 1);
        let mount = &mounts[0];
        assert_eq!(mount.mount_id, 36);
        assert_eq!(mount.parent_id, 25);
        assert_eq!((mount.major, mount.minor), (8, 1));
        assert_eq!(mount.optional_fields, ["shared:1", "master:2"]);
        assert_eq!(mount.fs_type, "ext4");
        assert_eq!(mount.mount_source, OsString::from("/dev/sda1"));
    }

    #[test]
    fn parser_decodes_procfs_octal_escapes() {
        let input = b"1 0 0:1 /root\\040dir /mnt\\011tab\\012line\\134slash rw - ext4 /dev/a\\040b rw\n";
        let (mounts, errors) = parse_mountinfo(input);
        assert!(errors.is_empty());
        assert_eq!(mounts.len(), 1);
        assert_eq!(
            mounts[0].root.as_os_str().as_bytes(),
            b"/root dir".as_slice()
        );
        assert_eq!(
            mounts[0].mount_point.as_os_str().as_bytes(),
            b"/mnt\ttab\nline\\slash".as_slice()
        );
        assert_eq!(
            mounts[0].mount_source.as_os_str().as_bytes(),
            b"/dev/a b".as_slice()
        );
    }

    #[test]
    fn malformed_line_is_partial_and_does_not_hide_following_mount() {
        let input = b"malformed line\n36 25 8:1 / / rw - ext4 /dev/sda1 rw\n";
        let (mounts, errors) = parse_mountinfo(input);
        assert_eq!(mounts.len(), 1);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line_number, 1);
    }

    #[test]
    fn duplicate_mount_rows_are_preserved() {
        let input = b"1 0 8:1 / /a rw - ext4 /dev/sda1 rw\n2 0 8:1 / /b rw - ext4 /dev/sda1 rw\n";
        let snapshot = collect_mount_snapshot_from_bytes_with_probe(input, |_| Ok(stats()));
        assert_eq!(snapshot.mounts.len(), 2);
        assert_eq!(snapshot.mounts[0].info.mount_source, snapshot.mounts[1].info.mount_source);
        assert_ne!(snapshot.mounts[0].info.mount_point, snapshot.mounts[1].info.mount_point);
    }

    #[test]
    fn category_classification_is_deterministic() {
        assert_eq!(classify_filesystem("ext4"), MountCategory::Local);
        assert_eq!(classify_filesystem("overlay"), MountCategory::Local);
        assert_eq!(classify_filesystem("NFS4"), MountCategory::Network);
        assert_eq!(classify_filesystem("fuse.sshfs"), MountCategory::Network);
        assert_eq!(classify_filesystem("fuse.rclone"), MountCategory::Network);
        assert_eq!(classify_filesystem("fuse.portal"), MountCategory::Fuse);
        assert_eq!(classify_filesystem("proc"), MountCategory::Special);
    }

    #[test]
    fn autofs_is_never_probed() {
        let calls = Cell::new(0usize);
        let input = b"1 0 0:1 / /auto rw - autofs systemd-1 rw\n";
        let snapshot = collect_mount_snapshot_from_bytes_with_probe(input, |_| {
            calls.set(calls.get() + 1);
            Ok(stats())
        });
        assert_eq!(calls.get(), 0);
        assert!(matches!(snapshot.mounts[0].stats, MountStatsState::SkippedAutoFs));
    }

    #[test]
    fn known_network_mount_is_visible_but_never_probed() {
        let calls = Cell::new(0usize);
        let input = b"1 0 0:1 / /net rw - nfs4 server:/export rw\n";
        let snapshot = collect_mount_snapshot_from_bytes_with_probe(input, |_| {
            calls.set(calls.get() + 1);
            Ok(stats())
        });
        assert_eq!(calls.get(), 0);
        assert_eq!(snapshot.mounts.len(), 1);
        assert!(matches!(snapshot.mounts[0].stats, MountStatsState::SkippedNetwork));
    }

    #[test]
    fn stat_math_uses_fragment_size_and_preserves_reserved_truth() {
        let stats = MountStats::from_counts(RawStatCounts {
            block_size: 8192,
            fragment_size: 4096,
            blocks: 100,
            blocks_free: 40,
            blocks_available: 30,
            files: 50,
            files_free: 20,
            files_available: 18,
        });
        assert_eq!(stats.total_bytes, 409_600);
        assert_eq!(stats.free_bytes, 163_840);
        assert_eq!(stats.available_bytes, 122_880);
        assert_eq!(stats.used_bytes, Some(245_760));
        assert_eq!(stats.reserved_bytes, Some(40_960));
        assert_eq!(stats.usage_tenths_percent, Some(600));
        assert_eq!(stats.total_inodes, 50);
        assert_eq!(stats.used_inodes, Some(30));
        assert_eq!(stats.available_inodes, 18);
        assert_eq!(stats.inode_usage_tenths_percent, Some(600));
    }

    #[test]
    fn zero_fragment_size_falls_back_to_block_size() {
        let stats = MountStats::from_counts(RawStatCounts {
            block_size: 8192,
            fragment_size: 0,
            blocks: 2,
            blocks_free: 1,
            blocks_available: 1,
            files: 0,
            files_free: 0,
            files_available: 0,
        });
        assert_eq!(stats.total_bytes, 16_384);
        assert_eq!(stats.used_bytes, Some(8192));
    }

    #[test]
    fn inconsistent_block_counters_never_fabricate_used_or_percent() {
        let stats = MountStats::from_counts(RawStatCounts {
            block_size: 4096,
            fragment_size: 4096,
            blocks: 10,
            blocks_free: 11,
            blocks_available: 12,
            files: 10,
            files_free: 11,
            files_available: 12,
        });
        assert_eq!(stats.used_bytes, None);
        assert_eq!(stats.reserved_bytes, None);
        assert_eq!(stats.usage_tenths_percent, None);
        assert_eq!(stats.used_inodes, None);
        assert_eq!(stats.inode_usage_tenths_percent, None);
    }

    #[test]
    fn zero_inode_total_has_no_fake_percentage() {
        let stats = MountStats::from_counts(RawStatCounts {
            block_size: 4096,
            fragment_size: 4096,
            blocks: 1,
            blocks_free: 1,
            blocks_available: 1,
            files: 0,
            files_free: 0,
            files_available: 0,
        });
        assert_eq!(stats.inode_usage_tenths_percent, None);
    }

    #[test]
    fn unavailable_mount_does_not_fail_snapshot() {
        let snapshot = collect_mount_snapshot_from_bytes_with_probe(SIMPLE, |_| {
            Err("permission denied".into())
        });
        assert_eq!(snapshot.mounts.len(), 1);
        assert!(snapshot.is_partial());
        assert_eq!(snapshot.unavailable_count(), 1);
        assert!(matches!(
            &snapshot.mounts[0].stats,
            MountStatsState::Unavailable(error) if error == "permission denied"
        ));
    }

    #[test]
    fn live_mountinfo_smoke_is_nonempty() {
        let snapshot = collect_mount_snapshot().expect("read /proc/self/mountinfo");
        assert!(!snapshot.mounts.is_empty());
    }
}
