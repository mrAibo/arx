from pathlib import Path


def replace_exact(path: str, old: str, new: str, expected: int = 1) -> None:
    p = Path(path)
    text = p.read_text()
    count = text.count(old)
    if count != expected:
        raise SystemExit(f"{path}: expected {expected} anchor(s), found {count}: {old[:120]!r}")
    p.write_text(text.replace(old, new))


# Restore the existing SFTP test annotation and keep exactly one attribute on
# the new WebDAV planner test.
replace_exact(
    "src/transfer/mod.rs",
    "    #[test]\n    #[test]\n    fn webdav_remote_copy_requires_frozen_copy_tree_and_rejects_move()",
    "    #[test]\n    fn webdav_remote_copy_requires_frozen_copy_tree_and_rejects_move()",
)
replace_exact(
    "src/transfer/mod.rs",
    "\n    fn remote_to_remote_uses_sftp_and_never_rsync()",
    "\n    #[test]\n    fn remote_to_remote_uses_sftp_and_never_rsync()",
)

# Load the shared physical fault proxy exactly once at the vfs parent level.
replace_exact(
    "src/vfs/mod.rs",
    '#[cfg(all(test, feature = "physical-webdav"))]\nmod webdav_acceptance;\n',
    '#[cfg(all(test, feature = "physical-webdav"))]\nmod webdav_acceptance_proxy;\n'
    '#[cfg(all(test, feature = "physical-webdav"))]\nmod webdav_acceptance;\n',
)
replace_exact(
    "src/vfs/webdav_acceptance.rs",
    '// Test-only TCP proxy for W15/W17/W18 (fault injection front of real Apache).\n#[path = "webdav_acceptance_proxy.rs"]\nmod webdav_acceptance_proxy;\n',
    '// Test-only TCP proxy for W15/W17/W18 is shared from the parent vfs test module.\nuse super::webdav_acceptance_proxy;\n',
)
replace_exact(
    "src/vfs/webdav_remote_copy_acceptance.rs",
    '#[path = "webdav_acceptance_proxy.rs"]\nmod fault_proxy;\n\n',
    '',
)
replace_exact(
    "src/vfs/webdav_remote_copy_acceptance.rs",
    'use fault_proxy::{ProxyMode, start_proxy};',
    'use super::webdav_acceptance_proxy::{ProxyMode, start_proxy};',
)

# Physical test polish: exact byte total, no unused secret copy/import, and a
# synchronous registry constructor because it performs no async work.
replace_exact(
    "src/vfs/webdav_remote_copy_acceptance.rs",
    'use std::sync::atomic::{AtomicBool, Ordering};',
    'use std::sync::atomic::AtomicBool;',
)
replace_exact(
    "src/vfs/webdav_remote_copy_acceptance.rs",
    '    pass: String,\n    provider: Arc<WebDavProvider>,',
    '    provider: Arc<WebDavProvider>,',
)
replace_exact(
    "src/vfs/webdav_remote_copy_acceptance.rs",
    '        user,\n        pass,\n        provider,',
    '        user,\n        provider,',
)
replace_exact(
    "src/vfs/webdav_remote_copy_acceptance.rs",
    'assert_eq!(manifest.total_bytes, Some(26));',
    'assert_eq!(manifest.total_bytes, Some(24));',
)
replace_exact(
    "src/vfs/webdav_remote_copy_acceptance.rs",
    'async fn registry_with_urls(\n',
    'fn registry_with_urls(\n',
)
replace_exact(
    "src/vfs/webdav_remote_copy_acceptance.rs",
    'let registry = registry_with_urls(f, proxy.listen_addr.clone(), f.b.url.clone()).await;',
    'let registry = registry_with_urls(f, proxy.listen_addr.clone(), f.b.url.clone());',
)
replace_exact(
    "src/vfs/webdav_remote_copy_acceptance.rs",
    'let registry = registry_with_urls(f, f.a.url.clone(), proxy.listen_addr.clone()).await;',
    'let registry = registry_with_urls(f, f.a.url.clone(), proxy.listen_addr.clone());',
    expected=2,
)

# Clippy: collapse factual size checks without changing semantics.
replace_exact(
    "src/transfer/webdav_transfer.rs",
    '''        if let Some(expected) = source_size {\n            if destination_files.get(&relative).copied().flatten() != Some(expected) {\n                return Err(io::Error::new(\n                    io::ErrorKind::InvalidData,\n                    format!(\n                        "WebDAV copy verification failed: size differs for {}",\n                        relative.display()\n                    ),\n                ));\n            }\n        }\n''',
    '''        if let Some(expected) = source_size\n            && destination_files.get(&relative).copied().flatten() != Some(expected)\n        {\n            return Err(io::Error::new(\n                io::ErrorKind::InvalidData,\n                format!(\n                    "WebDAV copy verification failed: size differs for {}",\n                    relative.display()\n                ),\n            ));\n        }\n''',
)
replace_exact(
    "src/transfer/webdav_transfer.rs",
    '''    if let Some(expected) = file.advertised_size {\n        if copied != expected {\n            return Err(io::Error::new(\n                io::ErrorKind::InvalidData,\n                format!(\n                    "WebDAV source size changed while streaming {}: expected {expected}, got {copied}",\n                    file.relative.display()\n                ),\n            ));\n        }\n    }\n''',
    '''    if let Some(expected) = file.advertised_size\n        && copied != expected\n    {\n        return Err(io::Error::new(\n            io::ErrorKind::InvalidData,\n            format!(\n                "WebDAV source size changed while streaming {}: expected {expected}, got {copied}",\n                file.relative.display()\n            ),\n        ));\n    }\n''',
)

# Keep the streaming helper small and coherent by carrying operation-scoped
# immutable values in one context instead of suppressing too_many_arguments.
replace_exact(
    "src/transfer/webdav_transfer.rs",
    '''const WEBDAV_REMOTE_COPY_PIPE_BYTES: usize = 64 * 1024;\nconst MAX_WEBDAV_REMOTE_COPY_FILE_BYTES: usize = 16 * 1024 * 1024 * 1024;\n\nasync fn copy_tree_file_streamed(\n    source_provider: &WebDavProvider,\n    destination_provider: &WebDavProvider,\n    file: &TreeFile,\n    destination: &WebDavWriteTarget,\n    cancel: Arc<AtomicBool>,\n    pause: crate::transfer_queue::PauseGate,\n    base: u64,\n    total: Option<u64>,\n    on_progress: &mut impl FnMut(TypedTransferProgress),\n) -> io::Result<u64> {\n''',
    '''const WEBDAV_REMOTE_COPY_PIPE_BYTES: usize = 64 * 1024;\nconst MAX_WEBDAV_REMOTE_COPY_FILE_BYTES: usize = 16 * 1024 * 1024 * 1024;\n\nstruct RemoteCopyFileContext<'a> {\n    source_provider: &'a WebDavProvider,\n    destination_provider: &'a WebDavProvider,\n    destination_root: &'a WebDavWriteTarget,\n    cancel: Arc<AtomicBool>,\n    pause: crate::transfer_queue::PauseGate,\n    total: Option<u64>,\n}\n\nasync fn copy_tree_file_streamed(\n    context: &RemoteCopyFileContext<'_>,\n    file: &TreeFile,\n    base: u64,\n    on_progress: &mut impl FnMut(TypedTransferProgress),\n) -> io::Result<u64> {\n    let destination = upload_target_for_relative(context.destination_root, &file.relative)?;\n''',
)
replace_exact(
    "src/transfer/webdav_transfer.rs",
    '''        let result = source_provider\n            .get_stream(\n                &file.source.href,\n                MAX_WEBDAV_REMOTE_COPY_FILE_BYTES,\n                &mut writer,\n                Some(&cancel),\n                Some(&pause),\n                |completed, _| {\n                    if let Some(completed) = base.checked_add(completed) {\n                        on_progress(TypedTransferProgress::Bytes { completed, total });\n                    }\n                },\n            )\n''',
    '''        let result = context\n            .source_provider\n            .get_stream(\n                &file.source.href,\n                MAX_WEBDAV_REMOTE_COPY_FILE_BYTES,\n                &mut writer,\n                Some(&context.cancel),\n                Some(&context.pause),\n                |completed, _| {\n                    if let Some(completed) = base.checked_add(completed) {\n                        on_progress(TypedTransferProgress::Bytes {\n                            completed,\n                            total: context.total,\n                        });\n                    }\n                },\n            )\n''',
)
replace_exact(
    "src/transfer/webdav_transfer.rs",
    '''    let put = destination_provider.put_logical_stream_with_policy(\n        &destination.logical_path,\n''',
    '''    let put = context.destination_provider.put_logical_stream_with_policy(\n        &destination.logical_path,\n''',
)
replace_exact(
    "src/transfer/webdav_transfer.rs",
    '''        let mut completed_before = 0u64;\n        for file in &manifest.files {\n''',
    '''        let file_context = RemoteCopyFileContext {\n            source_provider,\n            destination_provider,\n            destination_root,\n            cancel: cancel.clone(),\n            pause: pause.clone(),\n            total: manifest.total_bytes,\n        };\n        let mut completed_before = 0u64;\n        for file in &manifest.files {\n''',
)
replace_exact(
    "src/transfer/webdav_transfer.rs",
    '''            let target = upload_target_for_relative(destination_root, &file.relative)?;\n            let copied = copy_tree_file_streamed(\n                source_provider,\n                destination_provider,\n                file,\n                &target,\n                cancel.clone(),\n                pause.clone(),\n                completed_before,\n                manifest.total_bytes,\n                on_progress,\n            )\n''',
    '''            let copied = copy_tree_file_streamed(\n                &file_context,\n                file,\n                completed_before,\n                on_progress,\n            )\n''',
)

print("WebDAV copy clippy and acceptance fixes applied")
