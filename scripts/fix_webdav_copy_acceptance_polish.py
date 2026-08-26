from pathlib import Path

p = Path('src/vfs/webdav_remote_copy_acceptance.rs')
text = p.read_text()

single = [
    ('use std::sync::atomic::{AtomicBool, Ordering};', 'use std::sync::atomic::AtomicBool;'),
    ('    pass: String,\n    provider: Arc<WebDavProvider>,', '    provider: Arc<WebDavProvider>,'),
    ('        user,\n        pass,\n        provider,', '        user,\n        provider,'),
    ('assert_eq!(manifest.total_bytes, Some(26));', 'assert_eq!(manifest.total_bytes, Some(24));'),
    ('async fn registry_with_urls(\n', 'fn registry_with_urls(\n'),
    ('let registry = registry_with_urls(f, proxy.listen_addr.clone(), f.b.url.clone()).await;', 'let registry = registry_with_urls(f, proxy.listen_addr.clone(), f.b.url.clone());'),
]
for old, new in single:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f'expected one anchor, found {count}: {old!r}')
    text = text.replace(old, new, 1)

old = 'let registry = registry_with_urls(f, f.a.url.clone(), proxy.listen_addr.clone()).await;'
if text.count(old) != 2:
    raise SystemExit(f'expected two proxy destination anchors, found {text.count(old)}')
text = text.replace(old, 'let registry = registry_with_urls(f, f.a.url.clone(), proxy.listen_addr.clone());')

p.write_text(text)
print('physical acceptance polish applied')
