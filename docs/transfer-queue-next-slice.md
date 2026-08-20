# PACK G — Progress and retry truth checkpoint

Reviewed start SHA: `9dd7863aad4daa1c2a9d703d5a4f8139f9eb83f6`

This note is an implementation contract for the next PACK G slice. It is not a
product claim and must be removed or folded into permanent documentation before
final release if it becomes stale.

## Single progress model

Use the existing `transfer_queue::TypedTransferProgress` end-to-end. Do not add
a second progress-unit abstraction beside it.

- Native/rsync may remain `Items` where bytes are not truthfully observed.
- SFTP streaming should publish cumulative job-level `Bytes` after successful
  chunks. Multi-file progress must never reset to zero between files.
- S3 download should publish cumulative body bytes. Missing Content-Length is
  `total: None`, never zero.
- WebDAV download should expose bytes from the existing streaming seam and keep
  the exact streamed terminal count.
- WebDAV upload must not invent incremental bytes without an observable request
  body transmission seam.
- S3 upload remains `Items`/parts unless exact transmitted bytes are measured.

Speed/ETA are byte-only. Unknown total has no ETA.

## Retry is phase truth, not intent truth

`SafeToRetry` is allowed only when the final destination is provably unchanged,
cleanup is confirmed, and replay is idempotent. Unclassified paths stay
`NeverRetry`.

Required provider distinctions:

- SFTP pre-commit staging/read/write failures may be safe only when cleanup is
  confirmed. Once target/backup/rename commit state is uncertain, use
  `AmbiguousMutation` or `RecoveryRequired`. Failed rollback is
  `RecoveryRequired`.
- S3 download transient GetObject/read/stage failures may be safe before final
  rename with confirmed stage cleanup. `AlreadyExists`, policy and validation
  failures are `NeverRetry`. Post-finalization verification failures are not a
  clean replay.
- WebDAV download transient GET/stage failures may be safe before persist with
  confirmed cleanup. Persist/policy/post-commit uncertainty is not a clean
  replay.
- S3/WebDAV upload mutation uncertainty stays one-shot.

Typed `RecoveryRequired` or `AmbiguousMutation` must take precedence over a
concurrent cancel flag. Do not overwrite a proven uncertain/recovery outcome as
`Cancelled` merely because an underlying I/O error is `Interrupted`. Clean
cancellation should be represented explicitly.

## S3 progress ordering

Do not expose terminal-looking part progress before required completion and
verification:

- Single PUT: final `1/1` only after HeadObject verification succeeds.
- Multipart: the last UploadPart must not publish terminal `N/N` before
  CompleteMultipartUpload. Final `N/N` belongs after Complete and required
  HeadObject verification.
- Abort/cleanup failure must preserve `RecoveryRequired`; do not flatten it into
  generic upload ambiguity.

## Retry attempt progress

At a safe retry boundary, reset attempt-local progress/rate truth explicitly so
stale previous-attempt values are not shown as current. Within one attempt,
unit and cumulative completed values are monotonic.

## Remaining sequence

After this slice is exact-SHA CI green:

pause token / safe checkpoints -> status bar -> Transfer Center -> shutdown/join
-> full deterministic matrix -> physical Local/OpenSSH SFTP/Apache WebDAV/MinIO
matrix -> full gates.

No merge, version bump, tag, release, or next feature while PACK G remains open.
