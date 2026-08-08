from pathlib import Path

path = Path('.github/ux_late_verification_migrate.py')
text = path.read_text()

old = '''        let new_diff = state.diff.clone().unwrap();
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
'''
new = '''        let new_diff = state.diff.clone().unwrap();
        let current_plan = state.plan.clone().unwrap();
        let plan_id = crate::workspace_sync_execution::SyncPlanValidator::freeze(
            &current_plan,
            &new_diff,
            &crate::vfs::default_registry(),
        )
        .unwrap()
        .id();
        let old_roots = (
            Location::Local(PathBuf::from("/old-left")),
            Location::Local(PathBuf::from("/old-right")),
        );
        let verification = SyncVerificationSnapshot {
            id: crate::workspace_sync_verification::SyncVerificationId(99),
            plan_id,
            left_root: old_roots.0,
            right_root: old_roots.1,
            status: SyncVerificationStatus::Superseded,
        };
'''
if text.count(old) != 1:
    raise SystemExit(f'late verification fixture mismatch: {text.count(old)}')
path.write_text(text.replace(old, new, 1))
