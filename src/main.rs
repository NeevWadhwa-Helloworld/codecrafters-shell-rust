#[allow(unused_imports)]
use std::io::{self, Write};

fn main() {
    loop{
        print!("$ ");
        io::stdout().flush().unwrap();

        let mut command=String::new();
        io::stdin().read_line(&mut command).unwrap();
        command=command.trim().to_string();
        if command == "exit" {
            break;
        }else if command.starts_with("echo "){
            println!("{}", &command[5..]);
        }else if command.starts_with("type "){
            let cmd_arg = command[5..].trim();
            if cmd_arg == "echo" || cmd_arg == "type" || cmd_arg == "exit" {
                println!("{} is a shell builtin", cmd_arg);
            } else if let Some(path) = get_executable_path(cmd_arg) {
                println!("{} is {}", cmd_arg, path.display());
            } else {
                println!("{}: not found", cmd_arg);
            }
        }else{
            println!("{}: command not found",command);
        }
    }
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