#[allow(unused_imports)]
use std::io::{self, Write};
use std::process::Command;

fn main() {
    loop {
        print!("$ ");
        io::stdout().flush().unwrap();

        let mut command = String::new();
        io::stdin().read_line(&mut command).unwrap();
        let command = command.trim_end_matches(['\n', '\r']);
        if command.is_empty() {
            continue;
        }

        let parts = parse_command_line(command);
        if parts.is_empty() {
            continue;
        }
        let cmd = parts[0].as_str();
        let args = &parts[1..];

        if cmd == "exit" {
            break;
        } else if cmd == "echo" {
            println!("{}", args.join(" "));
        } else if cmd == "pwd" {
            if let Ok(current_dir) = std::env::current_dir() {
                println!("{}", current_dir.display());
            }
        } else if cmd == "cd" {
            let target = match args.first().map(|s| s.as_str()) {
                Some("~") | None => std::env::var("HOME").ok(),
                Some(path) => Some(path.to_string()),
            };

            if let Some(target_path) = target {
                if let Err(_) = std::env::set_current_dir(&target_path) {
                    println!("cd: {}: No such file or directory", target_path);
                }
            }
        } else if cmd == "type" {
            if let Some(cmd_arg) = args.first() {
                if cmd_arg == "echo" || cmd_arg == "type" || cmd_arg == "exit" || cmd_arg == "pwd" {
                    println!("{} is a shell builtin", cmd_arg);
                } else if let Some(path) = get_executable_path(cmd_arg) {
                    println!("{} is {}", cmd_arg, path.display());
                } else {
                    println!("{}: not found", cmd_arg);
                }
            }
        } else if let Some(_path) = get_executable_path(cmd) {
            let _ = Command::new(cmd).args(args).status();
        } else {
            println!("{}: command not found", cmd);
        }
    }
}

fn parse_command_line(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current_arg = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut token_started = false;
    let mut escaped = false;

    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        if escaped {
            current_arg.push(ch);
            token_started = true;
            escaped = false;
            i += 1;
            continue;
        }

        if in_single_quote {
            if ch == '\'' {
                in_single_quote = false;
            } else {
                current_arg.push(ch);
            }
            token_started = true;
            i += 1;
            continue;
        }

        if in_double_quote {
            if ch == '\\' {
                if i + 1 < chars.len() {
                    let next_ch = chars[i + 1];
                    if next_ch == '"' || next_ch == '\\' {
                        current_arg.push(next_ch);
                        token_started = true;
                        i += 2;
                        continue;
                    }
                }
                current_arg.push('\\');
                token_started = true;
                i += 1;
                continue;
            } else if ch == '"' {
                in_double_quote = false;
                token_started = true;
                i += 1;
                continue;
            } else {
                current_arg.push(ch);
                token_started = true;
                i += 1;
                continue;
            }
        }

        match ch {
            '\\' => {
                escaped = true;
            }
            '\'' => {
                in_single_quote = true;
                token_started = true;
            }
            '"' => {
                in_double_quote = true;
                token_started = true;
            }
            ' ' | '\t' => {
                if token_started {
                    args.push(current_arg.clone());
                    current_arg.clear();
                    token_started = false;
                }
            }
            _ => {
                current_arg.push(ch);
                token_started = true;
            }
        }

        i += 1;
    }

    if token_started {
        args.push(current_arg);
    }

    args
}

fn get_executable_path(cmd: &str) -> Option<std::path::PathBuf> {
    if let Some(path_var) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&path_var) {
            let full_path = path.join(cmd);
            if full_path.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(metadata) = full_path.metadata() {
                        let mode = metadata.permissions().mode();
                        if mode & 0o111 != 0 {
                            return Some(full_path);
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    return Some(full_path);
                }
            }
        }
    }
    None
}