use std::path::Path;

use crate::process::ProcessService;

pub struct DiffService;

impl DiffService {
    pub async fn unified(left: &Path, right: &Path) -> Result<Vec<String>, String> {
        let left_exists = tokio::fs::try_exists(left)
            .await
            .map_err(|error| error.to_string())?;
        let right_exists = tokio::fs::try_exists(right)
            .await
            .map_err(|error| error.to_string())?;
        if !left_exists || !right_exists {
            return Err("both files must exist for content diff".into());
        }

        let args = vec![
            "--color=never".into(),
            "-u".into(),
            left.to_string_lossy().into_owned(),
            right.to_string_lossy().into_owned(),
        ];
        let output = ProcessService::output("diff", &args, None)
            .await
            .map_err(|error| error.to_string())?;

        // diff exits 1 when files differ; that is a successful comparison.
        if !matches!(output.status.code(), Some(0 | 1)) {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }

        let text = String::from_utf8_lossy(&output.stdout);
        if text.is_empty() {
            Ok(vec!["Files are identical".into()])
        } else {
            Ok(text.lines().map(str::to_string).collect())
        }
    }
}
