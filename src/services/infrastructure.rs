use tokio::task::JoinSet;

use crate::process::ProcessService;

pub struct InfrastructureService;

impl InfrastructureService {
    pub async fn snapshot() -> Vec<String> {
        let hosts = crate::remote::ssh_config::parse_ssh_config().unwrap_or_default();
        let mut tasks = JoinSet::new();

        for (alias, entry) in hosts.into_iter().take(30) {
            tasks.spawn(async move {
                let user = entry.user.as_deref().unwrap_or("root");
                let hostname = entry.hostname.as_deref().unwrap_or(&alias);
                let destination = format!("{user}@{hostname}");
                let args = vec![
                    "-o".into(),
                    "ConnectTimeout=2".into(),
                    "-o".into(),
                    "BatchMode=yes".into(),
                    destination,
                    "true".into(),
                ];
                let reachable = ProcessService::output("ssh", &args, None)
                    .await
                    .map(|output| output.status.success())
                    .unwrap_or(false);
                format!(
                    "{} {} ({})",
                    if reachable { "OK" } else { "X" },
                    alias,
                    hostname
                )
            });
        }

        let mut lines = Vec::new();
        while let Some(result) = tasks.join_next().await {
            if let Ok(line) = result {
                lines.push(line);
            }
        }
        lines.sort();
        lines
    }
}
