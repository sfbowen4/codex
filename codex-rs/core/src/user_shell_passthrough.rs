use std::path::Path;
use std::path::PathBuf;

use codex_protocol::user_input::UserInput;

const MULTIWORD_COMMAND_ALLOWLIST: &[&str] = &[
    "awk", "cargo", "cat", "chmod", "chown", "cp", "curl", "cut", "diff", "docker", "echo", "env",
    "fd", "find", "git", "go", "grep", "head", "jq", "just", "kubectl", "ls", "make", "mkdir",
    "mv", "npm", "npx", "perl", "pip", "pnpm", "printf", "python", "python3", "rg", "rm", "rsync",
    "ruby", "sed", "stat", "tail", "tar", "touch", "tr", "tree", "uname", "uniq", "uv", "wc",
    "which", "yarn",
];

const NEVER_AUTO_PASSTHROUGH: &[&str] = &[
    "htop", "less", "man", "more", "nano", "scp", "screen", "sftp", "ssh", "sudo", "tmux", "top",
    "vi", "vim", "watch",
];

const SINGLE_TOKEN_DENYLIST: &[&str] = &[
    "bash",
    "node",
    "perl",
    "powershell",
    "pwsh",
    "python",
    "python3",
    "ruby",
    "sh",
    "zsh",
];

pub(crate) fn detect_user_shell_passthrough_command(
    enabled: bool,
    cwd: &Path,
    items: &[UserInput],
) -> Option<String> {
    if !enabled {
        return None;
    }

    let [
        UserInput::Text {
            text,
            text_elements,
        },
    ] = items
    else {
        return None;
    };

    if !text_elements.is_empty() {
        return None;
    }

    let trimmed = text.trim();
    if trimmed.is_empty()
        || trimmed.contains('\n')
        || matches!(trimmed.chars().next(), Some('"') | Some('\'') | Some('`'))
    {
        return None;
    }

    let words = shlex::split(trimmed)?;
    let (command_index, command_word) = first_command_word(&words)?;
    let command_key = command_key(command_word)?;
    if NEVER_AUTO_PASSTHROUGH.contains(&command_key.as_str()) {
        return None;
    }

    let command_is_explicit_path = is_explicit_command_path(command_word);
    if !command_exists(command_word, cwd) {
        return None;
    }

    let command_args = &words[command_index + 1..];
    if command_args.is_empty() {
        if SINGLE_TOKEN_DENYLIST.contains(&command_key.as_str()) {
            return None;
        }
        return Some(trimmed.to_string());
    }

    if command_is_explicit_path
        || MULTIWORD_COMMAND_ALLOWLIST.contains(&command_key.as_str())
        || command_args
            .iter()
            .any(|arg| looks_like_command_argument(arg))
    {
        return Some(trimmed.to_string());
    }

    None
}

fn first_command_word(words: &[String]) -> Option<(usize, &str)> {
    words.iter().enumerate().find_map(|(idx, word)| {
        if looks_like_env_assignment(word) {
            None
        } else {
            Some((idx, word.as_str()))
        }
    })
}

fn looks_like_env_assignment(word: &str) -> bool {
    let Some((name, _value)) = word.split_once('=') else {
        return false;
    };

    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn looks_like_command_argument(arg: &str) -> bool {
    arg == "--"
        || arg.chars().all(|ch| ch.is_ascii_digit())
        || arg.chars().any(|ch| {
            matches!(
                ch,
                '-' | '/'
                    | '.'
                    | '='
                    | ':'
                    | '@'
                    | '~'
                    | '*'
                    | '?'
                    | '$'
                    | '%'
                    | ','
                    | '&'
                    | '|'
                    | ';'
                    | '<'
                    | '>'
            )
        })
}

fn command_key(command_word: &str) -> Option<String> {
    Path::new(command_word)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_ascii_lowercase)
}

fn is_explicit_command_path(command_word: &str) -> bool {
    command_word.contains(std::path::MAIN_SEPARATOR) || command_word.starts_with('.')
}

fn command_exists(command_word: &str, cwd: &Path) -> bool {
    if is_explicit_command_path(command_word) {
        let path = if Path::new(command_word).is_absolute() {
            PathBuf::from(command_word)
        } else {
            cwd.join(command_word)
        };
        return path.is_file();
    }

    which::which_in(command_word, std::env::var_os("PATH"), cwd).is_ok()
}

#[cfg(test)]
mod tests {
    use super::detect_user_shell_passthrough_command;
    use codex_protocol::user_input::UserInput;
    use pretty_assertions::assert_eq;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn text_item(text: &str) -> Vec<UserInput> {
        vec![UserInput::Text {
            text: text.to_string(),
            text_elements: Vec::new(),
        }]
    }

    #[test]
    fn detects_single_word_command() {
        let cwd = TempDir::new().expect("temp dir");

        assert_eq!(
            detect_user_shell_passthrough_command(true, cwd.path(), &text_item("pwd")),
            Some("pwd".to_string())
        );
    }

    #[test]
    fn quoted_input_skips_passthrough() {
        let cwd = TempDir::new().expect("temp dir");

        assert_eq!(
            detect_user_shell_passthrough_command(true, cwd.path(), &text_item("\"pwd\"")),
            None
        );
    }

    #[test]
    fn natural_language_prompt_skips_passthrough() {
        let cwd = TempDir::new().expect("temp dir");

        assert_eq!(
            detect_user_shell_passthrough_command(true, cwd.path(), &text_item("who are you")),
            None
        );
    }

    #[test]
    fn allowlisted_multiword_command_passthroughs() {
        let cwd = TempDir::new().expect("temp dir");

        assert_eq!(
            detect_user_shell_passthrough_command(true, cwd.path(), &text_item("git status")),
            Some("git status".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn explicit_relative_path_passthroughs() {
        let cwd = TempDir::new().expect("temp dir");
        let script = cwd.path().join("bin-script");
        fs::write(&script, "#!/bin/sh\nexit 0\n").expect("write script");
        let mut permissions = fs::metadata(&script)
            .expect("script metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).expect("chmod script");

        assert_eq!(
            detect_user_shell_passthrough_command(true, cwd.path(), &text_item("./bin-script run")),
            Some("./bin-script run".to_string())
        );
    }
}
