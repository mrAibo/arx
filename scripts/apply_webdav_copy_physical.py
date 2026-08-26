from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one anchor, found {count}: {old[:100]!r}")
    p.write_text(text.replace(old, new, 1))


replace_once(
    "src/vfs/mod.rs",
    '#[cfg(all(test, feature = "physical-webdav"))]\nmod webdav_interop_acceptance;\n',
    '#[cfg(all(test, feature = "physical-webdav"))]\nmod webdav_interop_acceptance;\n'
    '#[cfg(all(test, feature = "physical-webdav"))]\nmod webdav_remote_copy_acceptance;\n',
)

replace_once(
    "src/transfer/webdav_transfer.rs",
    'async fn revalidate_tree_manifest(\n',
    'pub(crate) async fn revalidate_tree_manifest(\n',
)

replace_once(
    "src/transfer/executor.rs",
    '''        Some(CopyTreeFailure::AmbiguousMutation { .. }) => TransferExecutionError::Io {\n            source: error,\n            disposition: crate::transfer_queue::RetryDisposition::AmbiguousMutation,\n        },\n''',
    '''        Some(CopyTreeFailure::AmbiguousMutation { .. }) => TransferExecutionError::Io {\n            source: error,\n            // #275: destination mutation certainty is lost even when best-effort\n            // owned-root cleanup succeeds. Require operator recovery evidence;\n            // never let the queue auto-replay this remote mutation.\n            disposition: crate::transfer_queue::RetryDisposition::RecoveryRequired,\n        },\n''',
)

replace_once(
    "src/vfs/webdav_acceptance_proxy.rs",
    '''//!   AmbiguousPut      — forward the complete PUT by Content-Length, observe\n//!                       Apache's response head (confirm server completed), then\n//!                       discard it and close the ARX side.\n''',
    '''//!   AmbiguousPut      — forward the complete PUT by Content-Length, observe\n//!                       Apache's response head (confirm server completed), then\n//!                       discard it and close the ARX side.\n//!   AmbiguousPutDropDelete — same ambiguous PUT, then drop the cleanup DELETE\n//!                       before it reaches Apache to prove RecoveryRequired.\n''',
)
replace_once(
    "src/vfs/webdav_acceptance_proxy.rs",
    '''pub enum ProxyMode {\n    PassThroughRecord,\n    DropGetBody,\n    AmbiguousPut,\n}\n''',
    '''pub enum ProxyMode {\n    PassThroughRecord,\n    DropGetBody,\n    AmbiguousPut,\n    AmbiguousPutDropDelete,\n}\n''',
)
replace_once(
    "src/vfs/webdav_acceptance_proxy.rs",
    '''    pub propfind_count: usize,\n    pub seen_if_none_match: bool,\n''',
    '''    pub propfind_count: usize,\n    pub delete_count: usize,\n    pub seen_if_none_match: bool,\n''',
)
replace_once(
    "src/vfs/webdav_acceptance_proxy.rs",
    '''            propfind_count: 0,\n            seen_if_none_match: false,\n''',
    '''            propfind_count: 0,\n            delete_count: 0,\n            seen_if_none_match: false,\n''',
)
replace_once(
    "src/vfs/webdav_acceptance_proxy.rs",
    '''            "PROPFIND" => r.propfind_count += 1,\n            _ => {}\n''',
    '''            "PROPFIND" => r.propfind_count += 1,\n            "DELETE" => r.delete_count += 1,\n            _ => {}\n''',
)
replace_once(
    "src/vfs/webdav_acceptance_proxy.rs",
    '''        ProxyMode::AmbiguousPut => {\n            if method == "PUT" {\n''',
    '''        ProxyMode::AmbiguousPut | ProxyMode::AmbiguousPutDropDelete => {\n            if mode == ProxyMode::AmbiguousPutDropDelete && method == "DELETE" {\n                // Cleanup transport ambiguity: do not forward the DELETE.\n                let _ = client.shutdown().await;\n                return;\n            }\n            if method == "PUT" {\n''',
)

print("#275 physical wiring patch applied")
