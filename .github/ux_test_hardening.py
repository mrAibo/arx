from pathlib import Path

path = Path('tests/workspace_sync_verification.rs')
text = path.read_text()

old = '''#[tokio::test(flavor = "current_thread")]
async fn cancel_during_first_step_starts_verification_with_zero_completed_steps() {
'''
new = '''#[tokio::test(flavor = "current_thread")]
async fn first_step_cancel_request_never_suppresses_required_verification() {
'''
if text.count(old) != 1:
    raise SystemExit('first-step cancellation test name anchor mismatch')
text = text.replace(old, new, 1)

old = '''            assert!(manager.cancel(&id));
            break;
'''
# There is only one remaining current-step cancellation assertion in this file
# after the deterministic partial-failure test introduced in #33.
if text.count(old) != 1:
    raise SystemExit(f'cancel assertion anchor mismatch: {text.count(old)}')
text = text.replace(
    old,
    '''            // The tiny native copy may legitimately win the race before the\n            // presentation observes StepStarted. Losing that race is not a\n            // cancellation failure: Completed also requires verification.\n            let _ = manager.cancel(&id);\n            break;\n''',
    1,
)

old = '''    assert!(matches!(
        terminal_job_event(&mut job_rx).await,
        JobEvent::Cancelled { .. }
    ));
    let _ = terminal_verification_event(&mut verification_rx).await;

    let job = manager.get(&id).unwrap();
    assert_eq!(job.status, JobStatus::Cancelled);
    let Some(JobResult::WorkspaceSync(outcome)) = job.result else {
        panic!("cancelled first-step outcome was lost");
    };
    assert!(outcome.completed.is_empty());
    assert!(outcome.workspace_may_have_changed);
    assert!(job.verification.is_some());
}

#[tokio::test]
async fn changed_workspace_roots_reject_late_verification_result() {
'''
new = '''    let terminal = terminal_job_event(&mut job_rx).await;
    assert!(matches!(
        terminal,
        JobEvent::Cancelled { .. } | JobEvent::Completed { .. }
    ));
    let _ = terminal_verification_event(&mut verification_rx).await;

    let job = manager.get(&id).unwrap();
    assert!(matches!(job.status, JobStatus::Cancelled | JobStatus::Completed));
    let Some(JobResult::WorkspaceSync(outcome)) = job.result else {
        panic!("first-step execution outcome was lost");
    };
    assert!(outcome.workspace_may_have_changed);
    assert!(job.verification.is_some());
}

#[tokio::test]
async fn changed_workspace_roots_reject_late_verification_result() {
'''
if text.count(old) != 1:
    raise SystemExit('first-step cancellation terminal block mismatch')
text = text.replace(old, new, 1)
path.write_text(text)
