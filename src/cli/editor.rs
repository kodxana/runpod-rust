use super::{CliError, CliResult};
use crate::Error;
use std::{fs, io::Write, time::Duration};

fn command() -> CliResult<String> {
    for name in ["VISUAL", "EDITOR"] {
        if let Some(value) = std::env::var_os(name).filter(|v| !v.is_empty()) {
            return value
                .into_string()
                .map_err(|_| CliError::Api(Error::InvalidInput(format!("{name} must be UTF-8"))));
        }
    }
    if cfg!(windows) {
        return Ok("notepad".into());
    }
    for name in ["sensible-editor", "vim", "nano"] {
        if std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|dir| executable(&dir.join(name))))
        {
            return Ok(name.into());
        }
    }
    Ok("vi".into())
}
fn executable(path: &std::path::Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
pub(super) fn edit(text: &str) -> CliResult<Option<String>> {
    edit_with_command(text, &command()?)
}
fn edit_with_command(text: &str, command: &str) -> CliResult<Option<String>> {
    let words = crate::configuration::command_words(command, "editor")?;
    let (program, args) = words
        .split_first()
        .ok_or_else(|| Error::InvalidInput("editor command is empty".into()))?;
    let mut file = tempfile::Builder::new()
        .prefix("editor-")
        .suffix(".json")
        .tempfile()
        .map_err(Error::from)?;
    let mut text = text.to_owned();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if cfg!(windows) {
        text = format!("\u{feff}{}", text.replace('\n', "\r\n"));
    }
    file.write_all(text.as_bytes()).map_err(Error::from)?;
    file.flush().map_err(Error::from)?;
    let modified = file
        .as_file()
        .metadata()
        .and_then(|m| m.modified())
        .map_err(Error::from)?;
    let past = modified
        .checked_sub(Duration::from_secs(2))
        .ok_or_else(|| Error::InvalidInput("editor timestamp is out of range".into()))?;
    file.as_file()
        .set_times(fs::FileTimes::new().set_modified(past))
        .map_err(Error::from)?;
    let modified = file
        .as_file()
        .metadata()
        .and_then(|m| m.modified())
        .map_err(Error::from)?;
    let path = file.into_temp_path();
    let status = std::process::Command::new(program)
        .args(args)
        .arg(&path)
        .status()
        .map_err(|source| Error::Context {
            message: format!("{command}: Editing failed"),
            source: Box::new(source.into()),
        })?;
    if !status.success() {
        return Err(Error::InvalidInput(format!("{command}: Editing failed")).into());
    }
    if fs::metadata(&path)
        .and_then(|m| m.modified())
        .map_err(Error::from)?
        == modified
    {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).map_err(Error::from)?;
    Ok(Some(
        text.strip_prefix('\u{feff}')
            .unwrap_or(&text)
            .replace("\r\n", "\n"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_editor_detects_saves_and_handles_exit_status() {
        let executable = std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        let command =
            format!("\"{executable}\" --ignored --exact cli::editor::tests::editor_child --quiet");
        assert_eq!(
            edit_with_command("{}", &format!("{command} save")).unwrap(),
            Some("{\"name\":\"edited\"}\n".into())
        );
        assert_eq!(
            edit_with_command("{}", &format!("{command} leave")).unwrap(),
            None
        );
        assert!(
            edit_with_command("{}", &format!("{command} fail"))
                .unwrap_err()
                .to_string()
                .contains("Editing failed")
        );
        assert!(
            edit_with_command("{}", "  ")
                .unwrap_err()
                .to_string()
                .contains("empty")
        );
    }

    #[test]
    #[ignore = "subprocess editor fixture, invoked by its parent test"]
    fn editor_child() {
        let args: Vec<_> = std::env::args().collect();
        let path = args.last().unwrap();
        if args.iter().any(|arg| arg == "save") {
            std::fs::write(path, b"\xef\xbb\xbf{\"name\":\"edited\"}\r\n").unwrap();
        }
        std::process::exit(if args.iter().any(|arg| arg == "fail") {
            7
        } else {
            0
        });
    }
}
