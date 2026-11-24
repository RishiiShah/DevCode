/// Returns true if the environment variable name is considered sensitive and must be stripped.
pub fn is_sensitive_env_var(key: &str) -> bool {
    key.starts_with("AWS_")
        || key == "GITHUB_TOKEN"
        || key == "API_KEY"
        || key.ends_with("_API_KEY")
        || key == "TOKEN"
        || key.ends_with("_TOKEN")
        || key.starts_with("SSH_")
}

/// Removes sensitive variables: `AWS_*`, `GITHUB_TOKEN`, `*_API_KEY`, `*_TOKEN`, `SSH_*`
/// (case-sensitive on POSIX).
pub fn strip_env<I, K, V>(vars: I) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    vars.into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .filter(|(k, _)| !is_sensitive_env_var(k))
        .collect()
}
