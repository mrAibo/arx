use arx::app::AppState;

/// Save workspace to ~/.local/share/arx/workspaces/ as JSON.
pub fn save_workspace(state: &AppState) -> std::io::Result<()> {
    let dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("arx")
        .join("workspaces");
    std::fs::create_dir_all(&dir)?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let path = dir.join(format!("workspace-{ts}.json"));
    let json = serde_json::json!({
        "version": "0.13",
        "left": { "location": state.left.location.to_string(), "cursor": state.left.cursor },
        "right": { "location": state.right.location.to_string(), "cursor": state.right.cursor },
        "panel_ratio": state.panel_ratio,
    });
    std::fs::write(&path, serde_json::to_string_pretty(&json)?)?;
    Ok(())
}
