from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f'{label}: expected 1 match, found {count}')
    return text.replace(old, new, 1)

path = Path('src/app/remote_workspace.rs')
text = path.read_text()
text = replace_once(
    text,
    '''    pub fn sync_verification_stage(&mut self, job_id: &str) {
        let Some(verification) = &self.verification else {
            return;
        };
        self.ux = if verification.status.is_terminal() {
            WorkspaceSyncUxState::Finished {
                job_id: job_id.to_string(),
            }
        } else {
            WorkspaceSyncUxState::Verifying {
                job_id: job_id.to_string(),
            }
        };
    }
''',
    '''    pub fn sync_verification_stage(&mut self, job_id: &str) {
        let Some(verification) = &self.verification else {
            return;
        };
        self.ux = if verification.status.is_terminal() {
            WorkspaceSyncUxState::Finished {
                job_id: job_id.to_string(),
            }
        } else {
            WorkspaceSyncUxState::Verifying {
                job_id: job_id.to_string(),
            }
        };
    }

    /// A verification result can belong to a Job that is still shown while the
    /// panes already point at a newer workspace. In that case the old result
    /// must not replace the new diff, but a terminal result must still settle
    /// the old Job presentation instead of leaving it stuck in Verifying.
    pub fn settle_rejected_verification(
        &mut self,
        job_id: &str,
        verification: &SyncVerificationSnapshot,
    ) {
        if verification.status.is_terminal()
            && self.ux.job_id().is_some_and(|current| current == job_id)
        {
            self.ux = WorkspaceSyncUxState::Finished {
                job_id: job_id.to_string(),
            };
        }
    }
''',
    'late verification settle method',
)
marker = '''    #[test]
    fn policy_change_invalidates_frozen_execution_context() {'''
insert = '''    #[test]
    fn rejected_terminal_verification_settles_old_job_without_replacing_new_workspace() {
        let mut state = RemoteWorkspaceState {
            enabled: true,
            ux: WorkspaceSyncUxState::Verifying {
                job_id: "sync-old".into(),
            },
            ..RemoteWorkspaceState::default()
        };
        state.refresh_visible(
            Location::Local(PathBuf::from("/new-left")),
            Location::Local(PathBuf::from("/new-right")),
            &[file("new.txt", 1)],
            &[],
        );
        state.ux = WorkspaceSyncUxState::Verifying {
            job_id: "sync-old".into(),
        };
        let new_diff = state.diff.clone().unwrap();
        let old_roots = (
            Location::Local(PathBuf::from("/old-left")),
            Location::Local(PathBuf::from("/old-right")),
        );
        let verification = SyncVerificationSnapshot {
            id: crate::workspace_sync_verification::SyncVerificationId(99),
            job_id: "sync-old".into(),
            plan_id: crate::workspace_sync_execution::SyncPlanId(99),
            left_root: old_roots.0,
            right_root: old_roots.1,
            status: SyncVerificationStatus::Superseded,
        };

        assert!(!state.apply_verification(
            &verification,
            &Location::Local(PathBuf::from("/new-left")),
            &Location::Local(PathBuf::from("/new-right")),
        ));
        state.settle_rejected_verification("sync-old", &verification);

        assert_eq!(state.diff.as_ref(), Some(&new_diff));
        assert!(matches!(
            state.ux,
            WorkspaceSyncUxState::Finished { ref job_id } if job_id == "sync-old"
        ));
    }

'''
if text.count(marker) != 1:
    raise SystemExit('late verification test anchor mismatch')
text = text.replace(marker, insert + marker, 1)
path.write_text(text)

path = Path('src/tui.rs')
text = path.read_text()
text = replace_once(
    text,
    '''            Some(event) = verification_rx.recv() => {
                let left_root = state.left.location.clone();
                let right_root = state.right.location.clone();
                if state.remote_workspace.apply_verification(
                    &event.verification,
                    &left_root,
                    &right_root,
                ) {
                    state.remote_workspace.sync_verification_stage(&event.job_id);
                    state.jobs = job_manager.snapshot();
                }
                continue;
            }
''',
    '''            Some(event) = verification_rx.recv() => {
                let left_root = state.left.location.clone();
                let right_root = state.right.location.clone();
                let accepted = state.remote_workspace.apply_verification(
                    &event.verification,
                    &left_root,
                    &right_root,
                );
                // JobManager accepted the verification before publishing this
                // event, so its render snapshot is useful even when pane roots
                // have moved and RemoteWorkspaceState rejects the old diff.
                state.jobs = job_manager.snapshot();
                if accepted {
                    state.remote_workspace.sync_verification_stage(&event.job_id);
                } else {
                    state
                        .remote_workspace
                        .settle_rejected_verification(&event.job_id, &event.verification);
                }
                continue;
            }
''',
    'verification event late-root handling',
)
path.write_text(text)
