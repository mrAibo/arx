from pathlib import Path


def rep(path: str, old: str, new: str, count: int = 1) -> None:
    p = Path(path)
    text = p.read_text()
    if text.count(old) < count:
        raise SystemExit(f"anchor not found in {path}: {old[:100]!r}")
    p.write_text(text.replace(old, new, count))


rep(
    "src/s3_inspector.rs",
    '''    let metadata = head
        .metadata()
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
''',
    '''    let metadata = head
        .metadata()
        .map(|metadata| {
            metadata
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();
''',
)

rep(
    "src/app/registration.rs",
    '''    /// Local-pane-only with a truthful disabled reason.
    LocalOnly(&'static str),
''',
    '''    /// Read-only storage intelligence available on Local and S3 panes.
    LocalOrS3(&'static str),
''',
)
rep(
    "src/app/registration.rs",
    '''            description: "Read-only local disk usage analysis by JobManager scan",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::LocalOnly(
            "Storage Inspector is available for local paths only",
        ),
''',
    '''            description: "Read-only storage intelligence for Local and S3 locations",
            category: ActionCategory::Panels,
            destructive: false,
        },
        policy: AvailabilityPolicy::LocalOrS3(
            "Storage Inspector is available for Local and S3 paths",
        ),
''',
)
rep(
    "src/app/registration.rs",
    '''            AvailabilityPolicy::LocalOnly("Storage Inspector is available for local paths only")
''',
    '''            AvailabilityPolicy::LocalOrS3("Storage Inspector is available for Local and S3 paths")
''',
)

rep(
    "src/app/availability.rs",
    '''        super::registration::AvailabilityPolicy::LocalOnly(reason) => {
            if ctx.active_provider == ProviderId::Local {
                ActionAvailability::Available
            } else {
                ActionAvailability::Disabled {
                    reason: reason.to_string(),
                }
            }
        }
''',
    '''        super::registration::AvailabilityPolicy::LocalOrS3(reason) => {
            if matches!(ctx.active_provider, ProviderId::Local | ProviderId::S3) {
                ActionAvailability::Available
            } else {
                ActionAvailability::Disabled {
                    reason: reason.to_string(),
                }
            }
        }
''',
)

p = Path("src/storage_inspector_ui.rs")
text = p.read_text()
needle = "launch_storage_inspector(&mut state)"
if text.count(needle) != 3:
    raise SystemExit(
        f"expected exactly 3 legacy local test calls in storage_inspector_ui.rs, found {text.count(needle)}"
    )
text = text.replace(needle, "launch_storage_inspector(&mut state, None)")
old_reason = '"Storage Inspector is available for local paths only"'
if text.count(old_reason) != 1:
    raise SystemExit(
        f"expected exactly 1 legacy local-only assertion in storage_inspector_ui.rs, found {text.count(old_reason)}"
    )
text = text.replace(
    old_reason,
    '"Storage Inspector is available for Local and S3 paths"',
    1,
)
p.write_text(text)
