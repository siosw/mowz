use std::{
    env, fs,
    path::{Path, PathBuf},
};

use eyre::{Context, Result};

const CONFIG_FILE: &str = ".mowz.toml";

pub(crate) fn load() -> Result<mowz::Config> {
    let start = env::current_dir().wrap_err("failed to determine current directory")?;
    let start = fs::canonicalize(&start)
        .wrap_err_with(|| format!("failed to resolve current directory {}", start.display()))?;
    let home = env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .and_then(|home| fs::canonicalize(home).ok());
    let path = find(&start, home.as_deref())?.ok_or_else(|| {
        eyre::eyre!(
            "could not find {CONFIG_FILE} from {} to the configuration boundary",
            start.display()
        )
    })?;
    mowz::Config::load(&path)
}

fn find(start: &Path, home: Option<&Path>) -> Result<Option<PathBuf>> {
    let mut directory = start.to_path_buf();

    loop {
        if home == Some(directory.as_path()) {
            return Ok(None);
        }

        let candidate = directory.join(CONFIG_FILE);
        if candidate
            .try_exists()
            .wrap_err_with(|| format!("failed to inspect {}", candidate.display()))?
        {
            return Ok(Some(candidate));
        }

        if !directory.pop() {
            return Ok(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn finds_config_in_starting_directory() {
        let directory = tempdir().unwrap();
        let config = directory.path().join(CONFIG_FILE);
        fs::write(&config, "[projects]\n").unwrap();

        assert_eq!(find(directory.path(), None).unwrap(), Some(config));
    }

    #[test]
    fn finds_config_in_parent_directory() {
        let directory = tempdir().unwrap();
        let child = directory.path().join("project").join("src");
        fs::create_dir_all(&child).unwrap();
        let config = directory.path().join(CONFIG_FILE);
        fs::write(&config, "[projects]\n").unwrap();

        assert_eq!(find(&child, None).unwrap(), Some(config));
    }

    #[test]
    fn nearest_config_wins() {
        let directory = tempdir().unwrap();
        let child = directory.path().join("project");
        fs::create_dir(&child).unwrap();
        fs::write(directory.path().join(CONFIG_FILE), "[projects]\n").unwrap();
        let nearest = child.join(CONFIG_FILE);
        fs::write(&nearest, "[projects]\n").unwrap();

        assert_eq!(find(&child, None).unwrap(), Some(nearest));
    }

    #[test]
    fn home_is_a_boundary_not_a_candidate() {
        let home = tempdir().unwrap();
        let child = home.path().join("project");
        fs::create_dir(&child).unwrap();
        fs::write(home.path().join(CONFIG_FILE), "[projects]\n").unwrap();

        assert_eq!(find(&child, Some(home.path())).unwrap(), None);
        assert_eq!(find(home.path(), Some(home.path())).unwrap(), None);
    }

    #[test]
    fn returns_none_when_no_config_exists() {
        let directory = tempdir().unwrap();
        let child = directory.path().join("project");
        fs::create_dir(&child).unwrap();

        assert_eq!(find(&child, Some(directory.path())).unwrap(), None);
    }
}
