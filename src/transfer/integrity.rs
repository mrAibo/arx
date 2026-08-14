//! Post-transfer integrity evidence for S3 objects.
//!
//! ARX does NOT compute a universal content hash (no SHA-256/MD5 over the
//! whole object) — that would require re-reading the entire payload and add a
//! dependency for checksum certainty that S3 does not guarantee. Instead the
//! integrity model is the **size** plus the S3 **ETag** returned by the service.
//! The ETag is an opaque server-computed value (MD5 for single-PUT, multipart
//! composite for multipart); we only compare it for *equality* — we never treat
//! it as a cryptographic hash or reconstruct one from it.
//!
//! Verification therefore answers a factual question ("did the remote object
//! report the same size and ETag we expected?") without rewriting the physical
//! local outcome.

/// Integrity evidence collected for an S3 object. NOT a content hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectIntegrity {
    /// Object size in bytes, as reported by the service (HeadObject /
    /// CompleteMultipartUpload).
    pub size: u64,
    /// ETag as returned by the service, if present.
    pub etag: Option<String>,
}

impl ObjectIntegrity {
    pub fn new(size: u64, etag: Option<String>) -> Self {
        Self { size, etag }
    }

    /// Compare two ETags for equality, tolerating the surrounding double-quotes
    /// that S3 sometimes includes in the `ETag` header / SDK value.
    /// Returns `None` (unknown) when either side has no ETag — callers must
    /// decide whether absence is acceptable rather than asserting mismatch.
    pub fn etag_matches(a: &Option<String>, b: &Option<String>) -> Option<bool> {
        match (a, b) {
            (None, _) | (_, None) => None,
            (Some(x), Some(y)) => {
                let nx = x.trim_matches('"');
                let ny = y.trim_matches('"');
                Some(nx == ny)
            }
        }
    }
}
