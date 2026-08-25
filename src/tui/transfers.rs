use super::*;

// Build a frozen Local<->S3 single-object copy spec from the S3 pane's focused
// identity and the Local pane's focused entry. The S3 `S3ObjectRef` is the sole
// authority for bucket/key — `entry.name` is never used to derive S3 identity.
// ponytail: one-file-per-op basic transfer; multi-object needs a later card
fn build_s3_copy_spec(
    src_provider: ProviderId,
    s3_listed: &ListedEntry,
    local_listed: Option<&ListedEntry>,
    s3_prefix: &str,
    local_path: &std::path::Path,
) -> Result<arx::transfer::S3TransferSpec, String> {
    let EntryIdentity::S3Object(s3_ref) = &s3_listed.identity else {
        return Err("Copy supports a single S3 object only".into());
    };
    if src_provider == ProviderId::S3 {
        // Download: S3 -> Local. Destination key is the S3 key verbatim.
        let basename = s3_ref.key.rsplit('/').next().unwrap_or(s3_ref.key.as_str());
        let local_name =
            arx::transfer::s3_download_local_name(s3_ref, basename).map_err(|e| e.to_string())?;
        Ok(arx::transfer::S3TransferSpec::DownloadOne {
            source: s3_ref.clone(),
            local_destination: local_path.join(local_name),
        })
    } else {
        // Upload: Local -> S3. Key is nav_prefix + local filename; target/bucket
        // come from the frozen S3 identity.
        let Some(local_listed) = local_listed else {
            return Err("Select a local file to upload".into());
        };
        let filename = local_listed.entry.name.as_str();
        let destination = arx::transfer::s3_upload_destination_ref(
            &s3_ref.target,
            &s3_ref.bucket,
            s3_prefix,
            filename,
        )
        .map_err(|e| e.to_string())?;
        Ok(arx::transfer::S3TransferSpec::UploadOne {
            local_source: local_path.join(filename),
            destination,
        })
    }
}

pub(super) fn handle_action(
    state: &mut AppState,
    action: &Action,
    focused: Option<&Entry>,
    focused_listed: Option<&ListedEntry>,
    other_listed: Option<&ListedEntry>,
    active_listed: &[&ListedEntry],
    sync: &SyncUiRuntime,
) -> bool {
    match action {
        Action::Copy => {
            let selected_names: Vec<String> = state
                .selection_names(state.active, &state.active_pane().location)
                .map(|names| names.iter().cloned().collect())
                .unwrap_or_default();
            let mut names: Vec<String> = if selected_names.is_empty() {
                focused
                    .map(|entry| vec![entry.name.clone()])
                    .unwrap_or_default()
            } else {
                selected_names.clone()
            };
            if names.is_empty() {
                state.message = Some("Select a file or directory to copy".into());
                return true;
            }
            let src_loc = state.active_pane().location.clone();
            let dst_loc = state.other_pane().location.clone();
            let src_provider = src_loc.provider_id();
            let dst_provider = dst_loc.provider_id();
            // PACK Q1: capability truth from the exact Locations.
            let src_caps = state
                .registry
                .capabilities_for_location(&src_loc)
                .unwrap_or_default();
            let dst_caps = state
                .registry
                .capabilities_for_location(&dst_loc)
                .unwrap_or_default();
            // S3 basic transfer: a single S3 object paired with a Local pane.
            let s3_spec: Option<arx::transfer::S3TransferSpec> =
                if src_provider == ProviderId::S3 || dst_provider == ProviderId::S3 {
                    // The S3 object identity comes from the S3 pane's focused entry
                    // (active or passive) — never reconstructed from entry.name.
                    let s3_is_source = src_provider == ProviderId::S3;
                    let (s3_listed, local_listed, local_path, s3_prefix) = if s3_is_source {
                        let Location::Local(p) = &dst_loc else {
                            state.message = Some("S3 download requires a Local destination".into());
                            return true;
                        };
                        (focused_listed, other_listed, p.clone(), String::new())
                    } else {
                        let Location::S3 { prefix, .. } = &dst_loc else {
                            state.message = Some("S3 upload requires an S3 destination".into());
                            return true;
                        };
                        let Location::Local(p) = &src_loc else {
                            state.message = Some("S3 upload requires a Local source".into());
                            return true;
                        };
                        (other_listed, focused_listed, p.clone(), prefix.clone())
                    };
                    let Some(s3_listed) = s3_listed else {
                        state.message = Some("Focus an S3 object to copy".into());
                        return true;
                    };
                    match build_s3_copy_spec(
                        src_provider,
                        s3_listed,
                        local_listed,
                        &s3_prefix,
                        &local_path,
                    ) {
                        Ok(spec) => Some(spec),
                        Err(msg) => {
                            state.message = Some(msg);
                            return true;
                        }
                    }
                } else {
                    None
                };

            // WebDAV basic transfer: resolve exactly ONE source from the ACTIVE
            // pane. Selection wins over cursor and is matched against current
            // real ListedEntry rows; passive pane contributes Location only.
            let webdav_spec: Option<arx::transfer::WebDavTransferSpec> =
                if src_provider == ProviderId::WebDAV || dst_provider == ProviderId::WebDAV {
                    match arx::transfer::prepare_webdav_copy(
                        &src_loc,
                        &dst_loc,
                        &selected_names,
                        focused_listed,
                        active_listed,
                    ) {
                        Ok((spec, queue_name)) => {
                            names = vec![queue_name];
                            Some(spec)
                        }
                        Err(msg) => {
                            state.message = Some(msg);
                            return true;
                        }
                    }
                } else {
                    None
                };

            let mut executors =
                arx::transfer::probe::local_executors(arx::transfer::probe::detect_local_tools());
            if s3_spec.is_some() {
                executors.s3 = true;
            }
            if webdav_spec.is_some() {
                executors.webdav = true;
            }
            let request = arx::transfer::TransferRequest {
                source: src_loc.clone(),
                destination: dst_loc.clone(),
                source_provider: src_provider,
                destination_provider: dst_provider,
                source_capabilities: src_caps,
                destination_capabilities: dst_caps,
                intent: arx::transfer::TransferIntent::Copy,
                executors,
                delete_extraneous: false,
                s3_spec,
                webdav_spec,
            };
            let plan = match arx::transfer::TransferPlanner::plan(request) {
                Ok(p) => p,
                Err(e) => {
                    state.message = Some(e.to_string());
                    return true;
                }
            };
            let id = match sync.transfers.enqueue(plan, names) {
                Ok(id) => id,
                Err(e) => {
                    state.message = Some(e.to_string());
                    return true;
                }
            };
            state.jobs = sync.jobs.snapshot();
            state.message = Some(format!("Copy queued ({id})"));
            state.clear_selection();
            true
        }
        Action::Move => {
            let names: Vec<String> = state
                .selection_names(state.active, &state.active_pane().location)
                .map(|names| names.iter().cloned().collect())
                .or_else(|| focused.map(|entry| vec![entry.name.clone()]))
                .unwrap_or_default();
            if names.is_empty() {
                state.message = Some("Select a file or directory to move".into());
                return true;
            }
            // S3 move not supported — no destructive S3 move.
            if state.active_pane().location.provider_id() == ProviderId::S3
                || state.other_pane().location.provider_id() == ProviderId::S3
            {
                state.message = Some("S3 move not supported (use copy)".into());
                return true;
            }
            let src_loc = state.active_pane().location.clone();
            let dst_loc = state.other_pane().location.clone();
            let src_provider = src_loc.provider_id();
            let dst_provider = dst_loc.provider_id();
            // PACK Q1: capability truth from the exact Locations.
            let src_caps = state
                .registry
                .capabilities_for_location(&src_loc)
                .unwrap_or_default();
            let dst_caps = state
                .registry
                .capabilities_for_location(&dst_loc)
                .unwrap_or_default();
            let executors =
                arx::transfer::probe::local_executors(arx::transfer::probe::detect_local_tools());
            let request = arx::transfer::TransferRequest {
                source: src_loc.clone(),
                destination: dst_loc.clone(),
                source_provider: src_provider,
                destination_provider: dst_provider,
                source_capabilities: src_caps,
                destination_capabilities: dst_caps,
                intent: arx::transfer::TransferIntent::Move,
                executors,
                delete_extraneous: false,
                s3_spec: None,
                webdav_spec: None,
            };
            let plan = match arx::transfer::TransferPlanner::plan(request) {
                Ok(p) => p,
                Err(e) => {
                    state.message = Some(e.to_string());
                    return true;
                }
            };
            let id = match sync.transfers.enqueue(plan, names) {
                Ok(id) => id,
                Err(e) => {
                    state.message = Some(e.to_string());
                    return true;
                }
            };
            state.jobs = sync.jobs.snapshot();
            state.message = Some(format!("Move queued ({id})"));
            state.clear_selection();
            true
        }
        _ => false,
    }
}

// S3-40/41 F5 wiring: upload spec uses frozen S3ObjectRef, never entry.name.
#[cfg(test)]
mod tests {
    use super::*;
    use arx::transfer::{
        ExecutorAvailability, S3TransferSpec, TransferIntent, TransferMethod, TransferPlanner,
        TransferRequest,
    };
    use arx::vfs::CapabilitySet;

    // Upload: destination key = nav_prefix + local filename, target/bucket
    // from the frozen S3ObjectRef. entry.name is never consulted for S3 identity.
    #[test]
    fn upload_spec_preserves_frozen_s3_ref_key() {
        let s3_ref = arx::vfs::s3::S3ObjectRef {
            target: "tgt".into(),
            bucket: "bk".into(),
            key: "deep/existing/key".into(),
        };
        let s3_listed = ListedEntry {
            entry: Entry {
                name: "display-name".into(), // must NOT become the S3 key
                kind: EntryKind::File,
                size: Some(42),
                modified_unix_ms: Some(1),
            },
            identity: EntryIdentity::S3Object(s3_ref.clone()),
        };
        let local_listed = ListedEntry {
            entry: Entry {
                name: "local.txt".into(),
                kind: EntryKind::File,
                size: Some(10),
                modified_unix_ms: Some(2),
            },
            identity: EntryIdentity::Other,
        };
        let spec = build_s3_copy_spec(
            ProviderId::Local,
            &s3_listed,
            Some(&local_listed),
            "prefix",
            &PathBuf::from("/local/src"),
        )
        .unwrap();
        let S3TransferSpec::UploadOne {
            local_source,
            destination,
        } = spec
        else {
            panic!("expected UploadOne");
        };
        assert_eq!(local_source, PathBuf::from("/local/src/local.txt"));
        assert_eq!(destination.key, "prefix/local.txt");
        assert_eq!(destination.target, "tgt");
        assert_eq!(destination.bucket, "bk");
        assert_eq!(
            destination,
            arx::transfer::s3_upload_destination_ref("tgt", "bk", "prefix", "local.txt").unwrap()
        );
    }

    // Download: source is the frozen ref verbatim — key never reconstructed.
    #[test]
    fn download_spec_preserves_frozen_s3_ref_verbatim() {
        let s3_ref = arx::vfs::s3::S3ObjectRef {
            target: "tgt".into(),
            bucket: "bk".into(),
            key: "deep/existing/key".into(),
        };
        let s3_listed = ListedEntry {
            entry: Entry {
                name: "display-name".into(), // must NOT override the key
                kind: EntryKind::File,
                size: Some(42),
                modified_unix_ms: Some(1),
            },
            identity: EntryIdentity::S3Object(s3_ref.clone()),
        };
        let spec = build_s3_copy_spec(
            ProviderId::S3,
            &s3_listed,
            None,
            "",
            &PathBuf::from("/local/dst"),
        )
        .unwrap();
        let S3TransferSpec::DownloadOne {
            source,
            local_destination,
        } = spec
        else {
            panic!("expected DownloadOne");
        };
        assert_eq!(source.key, "deep/existing/key");
        assert_eq!(source, s3_ref);
        assert_eq!(local_destination, PathBuf::from("/local/dst/key"));
    }

    // Planner selects TransferMethod::S3 when the request carries a frozen spec
    // and executors.s3 == true.
    #[test]
    fn planner_selects_s3_method_for_frozen_copy_request() {
        let s3_ref = arx::vfs::s3::S3ObjectRef {
            target: "tgt".into(),
            bucket: "bk".into(),
            key: "path/file".into(),
        };
        let spec = S3TransferSpec::DownloadOne {
            source: s3_ref.clone(),
            local_destination: PathBuf::from("/dst/file"),
        };
        let request = TransferRequest {
            source: Location::S3 {
                target: "tgt".into(),
                bucket: Some("bk".into()),
                prefix: String::new(),
            },
            destination: Location::Local(PathBuf::from("/dst")),
            source_provider: ProviderId::S3,
            destination_provider: ProviderId::Local,
            source_capabilities: CapabilitySet::NONE,
            destination_capabilities: CapabilitySet::NONE,
            intent: TransferIntent::Copy,
            executors: ExecutorAvailability {
                native: true,
                rsync: false,
                sftp: false,
                s3: true,
                webdav: false,
            },
            delete_extraneous: false,
            s3_spec: Some(spec),
            webdav_spec: None,
        };
        let plan = TransferPlanner::plan(request).unwrap();
        assert_eq!(plan.method, TransferMethod::S3);
        assert!(plan.s3_spec.is_some());
        assert_eq!(
            plan.s3_spec.unwrap(),
            S3TransferSpec::DownloadOne {
                source: s3_ref,
                local_destination: PathBuf::from("/dst/file")
            }
        );
    }

    #[test]
    fn wrong_s3_identity_is_rejected() {
        let listed = ListedEntry {
            entry: Entry {
                name: "display-name".into(),
                kind: EntryKind::File,
                size: None,
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        };

        assert_eq!(
            build_s3_copy_spec(
                ProviderId::S3,
                &listed,
                None,
                "",
                &PathBuf::from("/local/dst"),
            ),
            Err("Copy supports a single S3 object only".into())
        );
    }

    #[test]
    fn webdav_multi_selection_dispatch_fails_before_enqueue() {
        let registry = ProviderRegistry::new();
        let sync = sync_runtime(registry.clone());
        let mut state = AppState {
            registry,
            ..AppState::default()
        };
        state.left.location = Location::Local(PathBuf::from("/active"));
        state.right.location = Location::WebDav {
            target: "dav".into(),
            path: "/dst/".into(),
        };
        state.active = Pane::Left;
        let scope = state.left.location.clone();
        state.toggle_selection(Pane::Left, &scope, "A.txt");
        state.toggle_selection(Pane::Left, &scope, "B.txt");
        let a = ListedEntry {
            entry: Entry {
                name: "A.txt".into(),
                kind: EntryKind::File,
                size: Some(1),
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        };
        let b = ListedEntry {
            entry: Entry {
                name: "B.txt".into(),
                kind: EntryKind::File,
                size: Some(1),
                modified_unix_ms: None,
            },
            identity: EntryIdentity::Other,
        };

        assert!(handle_action(
            &mut state,
            &Action::Copy,
            Some(&a.entry),
            Some(&a),
            None,
            &[&a, &b],
            &sync,
        ));
        assert_eq!(
            state.message.as_deref(),
            Some("WebDAV copy currently supports one selected item")
        );
        assert!(sync.jobs.snapshot().is_empty(), "nothing enqueued");
    }

    fn sync_runtime(registry: ProviderRegistry) -> SyncUiRuntime {
        let jobs = arx::jobs::JobManager::new();
        let (job_events, _job_rx) = mpsc::unbounded_channel();
        let (verification_events, _verification_rx) = mpsc::unbounded_channel();
        let (launch_events, _launch_rx) = mpsc::unbounded_channel();
        SyncUiRuntime {
            controller: WorkspaceSyncController::new(registry.clone()),
            jobs: jobs.clone(),
            job_events: job_events.clone(),
            verification_events,
            launch_events,
            transfers: arx::transfer_queue_runtime::TransferQueueRuntime::new(
                jobs,
                job_events,
                registry,
                arx::transfer_queue::TransferQueueConfig::default(),
            ),
        }
    }

    #[test]
    fn non_transfer_action_returns_false() {
        let registry = ProviderRegistry::new();
        let sync = sync_runtime(registry.clone());
        let mut state = AppState {
            registry,
            ..AppState::default()
        };

        assert!(!handle_action(
            &mut state,
            &Action::Quit,
            None,
            None,
            None,
            &[],
            &sync,
        ));
    }
}
