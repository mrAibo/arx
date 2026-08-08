use crate::process::ProcessService;
use crate::vfs::Location;

pub struct GitService;

impl GitService {
    /// Return the compact status-bar suffix for a local Git worktree.
    ///
    /// This must never run from `render()`. Callers cache the result until the
    /// active location changes.
    pub async fn status_suffix(location: &Location) -> String {
        let Location::Local(dir) = location else {
            return String::new();
        };

        let has_git_dir = tokio::fs::try_exists(dir.join(".git"))
            .await
            .unwrap_or(false);
        let has_head = tokio::fs::try_exists(dir.join("HEAD"))
            .await
            .unwrap_or(false);
        if !has_git_dir && !has_head {
            return String::new();
        }

        let branch_args = vec!["rev-parse".into(), "--abbrev-ref".into(), "HEAD".into()];
        let branch = match ProcessService::output("git", &branch_args, Some(dir)).await {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            }
            _ => return String::new(),
        };
        if branch.is_empty() {
            return String::new();
        }

        let status_args = vec!["status".into(), "--porcelain".into()];
        let dirty = ProcessService::output("git", &status_args, Some(dir))
            .await
            .ok()
            .filter(|output| output.status.success())
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .filter(|line| !line.is_empty())
                    .count()
            })
            .unwrap_or(0);

        if dirty == 0 {
            format!(" | git:{branch}")
        } else {
            format!(" | git:{branch}+{dirty}")
        }
    }
}
