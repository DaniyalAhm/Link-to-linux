//! Append-only journal so a killed process never loses state.
//! Android calls this from the JNI bridge with an explicit `filesDir` —
//! Rust never hardcodes `/data/data/...` paths.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

/// Append `line` (+ `\n`) to `dir/file`. Creates parent dirs. Syncs for durability.
pub fn append_line(dir: &str, file: &str, line: &str) -> anyhow::Result<()> {
    let path = Path::new(dir).join(file);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(f, "{line}")?;
    f.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_appends_and_syncs() {
        let dir = std::env::temp_dir().join(format!("link-test-{}", std::process::id()));
        append_line(dir.to_str().unwrap(), "n.log", "a|b|c").unwrap();
        let content = std::fs::read_to_string(dir.join("n.log")).unwrap();
        assert!(content.contains("a|b|c"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
