use crate::process::ProcessService;
use crate::vfs::Location;

pub struct TreeService;

impl TreeService {
    pub async fn snapshot(location: &Location, filter: &str) -> Vec<String> {
        let Location::Local(path) = location else {
            return vec!["Tree preview for remote/archive locations is not available yet".into()];
        };

        let args = vec![
            "-L".into(),
            "2".into(),
            "--noreport".into(),
            path.to_string_lossy().into_owned(),
        ];
        let output = match ProcessService::output("tree", &args, None).await {
            Ok(output) if output.status.success() => output,
            Ok(output) => {
                return vec![String::from_utf8_lossy(&output.stderr).trim().to_string()];
            }
            Err(error) => return vec![format!("tree unavailable: {error}")],
        };

        let query = filter.to_lowercase();
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| query.is_empty() || line.to_lowercase().contains(&query))
            .map(str::to_string)
            .collect()
    }
}
