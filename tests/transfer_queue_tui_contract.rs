//! Source-level product wiring contracts for the Transfer Queue TUI path.
//!
//! These complement behavioural `transfer_queue_contracts` tests. The runtime
//! tests prove queue cancellation semantics; these checks make sure the actual
//! Jobs-panel Delete path continues to enter that runtime instead of silently
//! regressing to a direct `JobManager::cancel()` call.

const TUI_SOURCE: &str = include_str!("../src/tui.rs");

fn function_block(name: &str) -> &'static str {
    let needle = format!("fn {name}");
    let start = TUI_SOURCE
        .find(&needle)
        .unwrap_or_else(|| panic!("missing TUI helper {name}"));
    let tail = &TUI_SOURCE[start..];
    let next = tail[needle.len()..]
        .find("\nfn ")
        .map(|offset| start + needle.len() + offset)
        .unwrap_or(TUI_SOURCE.len());
    &TUI_SOURCE[start..next]
}

#[test]
fn jobs_panel_delete_enters_product_cancel_router() {
    let delete_arm = TUI_SOURCE
        .find("KeyCode::Delete if state.show_jobs")
        .expect("Jobs-panel Delete key arm must exist");
    let nearby_end = (delete_arm + 800).min(TUI_SOURCE.len());
    let nearby = &TUI_SOURCE[delete_arm..nearby_end];

    assert!(
        nearby.contains("cancel_job_product_route(&mut state, &sync_runtime, &id)"),
        "Jobs-panel Delete must route through the product cancel helper"
    );
}

#[test]
fn product_cancel_router_splits_transfer_and_legacy_jobs() {
    let helper = function_block("cancel_job_product_route");

    assert!(
        helper.contains(
            "Some(arx::jobs::JobKind::Transfer) => sync.transfers.cancel(job_id).is_ok()"
        ),
        "Transfer jobs must cancel through TransferQueueRuntime"
    );
    assert!(
        helper.contains("job_manager.cancel(&id)"),
        "non-transfer jobs must preserve the existing JobManager cancel path"
    );
}

#[test]
fn copy_and_move_product_paths_enqueue_into_persistent_runtime() {
    let enqueue_calls = TUI_SOURCE
        .matches("sync.transfers.enqueue(plan, names)")
        .count();
    assert_eq!(
        enqueue_calls, 2,
        "Copy and Move should each enqueue exactly once through the persistent runtime"
    );

    assert!(
        !TUI_SOURCE
            .contains("arx::transfer::executor::execute_transfer(\n                    &plan2"),
        "legacy direct Copy/Move executor spawn must not return to the TUI product path"
    );
}

#[test]
fn pause_resume_controls_remain_unexposed_until_runtime_pause_is_real() {
    assert!(
        !TUI_SOURCE.contains("sync.transfers.pause(")
            && !TUI_SOURCE.contains("sync.transfers.resume("),
        "Pause/Resume must not be exposed in the TUI before cooperative pause checkpoints, same-task resume, and worker joining are implemented and Q14-Q30 are green"
    );
}
