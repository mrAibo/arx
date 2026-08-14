//! Pure S3 multipart upload planning.
//!
//! No AWS SDK, no network, no async. Given an object size it decides between a
//! single `PUT` and a multipart upload and computes the exact part layout
//! (offsets, lengths, numbers) with all arithmetic checked so it cannot
//! overflow. The policy constants below are ARX policy, not S3 service limits.

// ARX S3 multipart upload policy (NOT AWS service limits).
pub const SINGLE_PUT_THRESHOLD: u64 = 64 * 1024 * 1024; // 64 MiB
pub const MIN_PART_SIZE: u64 = 5 * 1024 * 1024; // 5 MiB
pub const MAX_PART_SIZE: u64 = 5 * 1024 * 1024 * 1024; // 5 GiB
pub const MAX_PARTS: u32 = 10_000;
pub const MAX_OBJECT_BYTES: u64 = 50_000_000_000_000; // 50 TB
pub const PREFERRED_PART_SIZE: u64 = 8 * 1024 * 1024; // 8 MiB
pub const PART_ALIGNMENT: u64 = 1024 * 1024; // 1 MiB

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadStrategy {
    SinglePut,
    Multipart(MultipartPlan),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultipartPlan {
    pub object_size: u64,
    pub part_size: u64,
    pub part_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultipartPart {
    pub number: i32,
    pub offset: u64,
    pub len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum S3MultipartError {
    #[error("object size {size} exceeds maximum allowed {max} bytes")]
    ObjectTooLarge { size: u64, max: u64 },
    #[error(
        "no part size <= {max_part_size} bytes satisfies the part limit for object of {size} bytes"
    )]
    PartLimitUnsatisfiable { size: u64, max_part_size: u64 },
    #[error("computed part count {count} is out of range 1..={max}")]
    PartCountOutOfRange { count: u32, max: u32 },
}

/// `ceil(a / b)`; `b` is always a nonzero constant.
fn ceil_div(a: u64, b: u64) -> u64 {
    a.div_ceil(b)
}

/// Round `x` up to the next multiple of `align`.
fn round_up(x: u64, align: u64) -> u64 {
    x.div_ceil(align) * align
}

/// Decide the upload strategy for `object_size`.
///
/// Returns [`UploadStrategy::SinglePut`] for objects at or below
/// [`SINGLE_PUT_THRESHOLD`]; otherwise a [`UploadStrategy::Multipart`] plan
/// whose part size is `max(PREFERRED_PART_SIZE, MIN_PART_SIZE,
/// ceil(object_size / MAX_PARTS))` rounded up to [`PART_ALIGNMENT`]. Errors
/// truthfully when the object exceeds [`MAX_OBJECT_BYTES`] or the part-size /
/// part-count limits cannot be satisfied.
pub fn plan_upload(object_size: u64) -> Result<UploadStrategy, S3MultipartError> {
    if object_size <= SINGLE_PUT_THRESHOLD {
        return Ok(UploadStrategy::SinglePut);
    }
    if object_size > MAX_OBJECT_BYTES {
        return Err(S3MultipartError::ObjectTooLarge {
            size: object_size,
            max: MAX_OBJECT_BYTES,
        });
    }

    let required_for_part_limit = ceil_div(object_size, MAX_PARTS as u64);
    let candidate = PREFERRED_PART_SIZE
        .max(MIN_PART_SIZE)
        .max(required_for_part_limit);
    let part_size = round_up(candidate, PART_ALIGNMENT);

    if part_size > MAX_PART_SIZE {
        return Err(S3MultipartError::PartLimitUnsatisfiable {
            size: object_size,
            max_part_size: MAX_PART_SIZE,
        });
    }

    let part_count_u64 = ceil_div(object_size, part_size);
    if !(1..=MAX_PARTS as u64).contains(&part_count_u64) {
        return Err(S3MultipartError::PartCountOutOfRange {
            count: part_count_u64 as u32,
            max: MAX_PARTS,
        });
    }

    Ok(UploadStrategy::Multipart(MultipartPlan {
        object_size,
        part_size,
        part_count: part_count_u64 as u32,
    }))
}

/// Materialize the exact part list for a [`MultipartPlan`].
///
/// Every part segment is contiguous (part `i+1` starts where part `i` ended),
/// the part numbers run `1..=part_count` ascending, and the sum of lengths
/// equals the object size. Non-last parts are exactly `part_size`; the last
/// part carries the remainder.
pub fn multipart_parts(plan: &MultipartPlan) -> Vec<MultipartPart> {
    let mut parts = Vec::with_capacity(plan.part_count as usize);
    for i in 0..plan.part_count {
        let offset = i as u64 * plan.part_size;
        let remaining = plan.object_size - offset;
        let len = remaining.min(plan.part_size);
        parts.push(MultipartPart {
            number: (i + 1) as i32,
            offset,
            len,
        });
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assert every invariant a valid multipart plan must satisfy.
    fn check_plan(plan: &MultipartPlan) {
        assert!(plan.part_count >= 1);
        assert!(plan.part_count <= MAX_PARTS);
        let parts = multipart_parts(plan);
        assert_eq!(parts.len(), plan.part_count as usize);
        let mut cursor = 0u64;
        let mut total = 0u64;
        for (i, p) in parts.iter().enumerate() {
            assert_eq!(p.number, (i + 1) as i32, "part number sequence");
            assert_eq!(p.offset, cursor, "part offset contiguous");
            assert!(p.len > 0, "part len must be > 0");
            assert!(p.len <= plan.part_size, "part len <= part_size");
            let is_last = i + 1 == parts.len();
            if is_last {
                assert!(p.len <= MAX_PART_SIZE, "last part <= MAX_PART_SIZE");
            } else {
                assert_eq!(p.len, plan.part_size, "non-last part is full part_size");
                assert!(p.len >= MIN_PART_SIZE, "non-last part >= MIN_PART_SIZE");
                assert!(p.len <= MAX_PART_SIZE, "non-last part <= MAX_PART_SIZE");
            }
            cursor += p.len;
            total += p.len;
        }
        assert_eq!(cursor, plan.object_size, "no gap / overlap");
        assert_eq!(total, plan.object_size, "sum of lengths == object_size");
    }

    #[test]
    fn single_put_boundary() {
        for &sz in &[0u64, 1, SINGLE_PUT_THRESHOLD - 1, SINGLE_PUT_THRESHOLD] {
            let s = plan_upload(sz).expect("small object plans");
            assert_eq!(s, UploadStrategy::SinglePut, "size {sz}");
        }
    }

    #[test]
    fn just_over_threshold_is_multipart() {
        let sz = SINGLE_PUT_THRESHOLD + 1;
        match plan_upload(sz).expect("plans") {
            UploadStrategy::Multipart(plan) => check_plan(&plan),
            UploadStrategy::SinglePut => panic!("expected multipart just over threshold"),
        }
    }

    #[test]
    fn five_gib_object_is_multipart() {
        let sz = 5 * 1024 * 1024 * 1024; // 5 GiB
        let plan = match plan_upload(sz).expect("plans") {
            UploadStrategy::Multipart(p) => p,
            UploadStrategy::SinglePut => panic!("5 GiB must be multipart"),
        };
        check_plan(&plan);
        assert!(plan.part_size >= MIN_PART_SIZE);
    }

    #[test]
    fn one_tib_aligned_and_bounded() {
        let sz = 1024u64 * 1024 * 1024 * 1024; // 1 TiB
        let plan = match plan_upload(sz).expect("plans") {
            UploadStrategy::Multipart(p) => p,
            UploadStrategy::SinglePut => panic!("1 TiB must be multipart"),
        };
        check_plan(&plan);
        assert!(plan.part_count <= MAX_PARTS);
        assert_eq!(
            plan.part_size % PART_ALIGNMENT,
            0,
            "part_size aligned to 1 MiB"
        );
    }

    #[test]
    fn near_fifty_tb_bounded() {
        let sz = 49_999_999_999_999u64;
        let plan = match plan_upload(sz).expect("plans") {
            UploadStrategy::Multipart(p) => p,
            UploadStrategy::SinglePut => panic!("near-50TB must be multipart"),
        };
        check_plan(&plan);
        assert!(plan.part_count <= MAX_PARTS);
        assert_eq!(plan.part_size % PART_ALIGNMENT, 0);
    }

    #[test]
    fn exact_max_object_is_multipart() {
        let sz = MAX_OBJECT_BYTES;
        let plan = match plan_upload(sz).expect("exact max plans") {
            UploadStrategy::Multipart(p) => p,
            UploadStrategy::SinglePut => panic!("exact max must be multipart"),
        };
        check_plan(&plan);
        assert!(plan.part_count <= MAX_PARTS);
    }

    #[test]
    fn over_max_object_rejected() {
        let res = plan_upload(MAX_OBJECT_BYTES + 1);
        assert!(matches!(res, Err(S3MultipartError::ObjectTooLarge { .. })));
    }

    #[test]
    fn rounding_boundary_is_aligned() {
        // 100 GB: required_for_part_limit (1e7) wins over PREFERRED and is not
        // 1 MiB aligned, so the rounding must align it up.
        let sz = 100_000_000_000u64;
        let plan = match plan_upload(sz).expect("plans") {
            UploadStrategy::Multipart(p) => p,
            UploadStrategy::SinglePut => panic!("100 GB must be multipart"),
        };
        check_plan(&plan);
        assert_eq!(plan.part_size % PART_ALIGNMENT, 0, "aligned to 1 MiB");
        assert!(plan.part_size >= 10 * 1024 * 1024);
    }
}
