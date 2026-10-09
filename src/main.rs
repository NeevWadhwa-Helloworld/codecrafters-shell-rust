#[allow(unused_imports)]
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};

#[cfg(unix)]
use std::os::fd::OwnedFd;

use std::os::unix::net::UnixStream;

#[derive(Clone, Copy, PartialEq)]
enum RedirectMode {
    Overwrite,
    Append,
}

struct Redirection {
    path: String,
    mode: RedirectMode,
}

// Struct to store background job details (job id, process id, command string, child process handle)
struct Job {
    id: usize,
    pid: u32,
    command: String,
    child: Child,
}

// Reaps finished background processes, prints their "Done" status, and removes them from job list
fn reap_jobs(jobs: &mut Vec<Job>) {
    let total_jobs = jobs.len();
    let mut remaining_jobs = Vec::new();

    for (idx, mut job) in std::mem::take(jobs).into_iter().enumerate() {
        let marker = if idx == total_jobs - 1 {
            '+'
        } else if idx == total_jobs - 2 {
            '-'
        } else {
            ' '
        };

        let is_finished = match job.child.try_wait() {
            Ok(Some(_status)) => true,
            _ => false,
        };

        if is_finished {
            let status_padded = format!("{:24}", "Done");
            let display_cmd = job.command.trim_end_matches('&').trim_end();
            println!("[{}]{} {}  {}", job.id, marker, status_padded, display_cmd);
        } else {
            remaining_jobs.push(job);
        }
    }

    *jobs = remaining_jobs;
}

fn is_builtin(cmd: &str) -> bool {
    matches!(cmd, "echo" | "pwd" | "cd" | "type" | "complete" | "jobs" | "exit" | "history")
}

fn execute_builtin(
    cmd: &str,
    args: &[String],
    completion_specs: &mut BTreeMap<String, String>,
    jobs: &mut Vec<Job>,
    history: &[String], // <--- Add this parameter
    out: &mut dyn Write,
    err_out: &mut dyn Write,
) {
    if cmd == "echo" {
        let _ = writeln!(out, "{}", args.join(" "));
    } else if cmd == "pwd" {
        if let Ok(current_dir) = std::env::current_dir() {
            let _ = writeln!(out, "{}", current_dir.display());
        }
    } else if cmd == "cd" {
        let target = match args.first().map(|s| s.as_str()) {
            Some("~") | None => std::env::var("HOME").ok(),
            Some(path) => Some(path.to_string()),
        };
        if let Some(target_path) = target {
            if let Err(_) = std::env::set_current_dir(&target_path) {
                let _ = writeln!(err_out, "cd: {}: No such file or directory", target_path);
            }
        }
    } else if cmd == "type" {
        if let Some(cmd_arg) = args.first() {
            if is_builtin(cmd_arg) {
                let _ = writeln!(out, "{} is a shell builtin", cmd_arg);
            } else if let Some(path) = get_executable_path(cmd_arg) {
                let _ = writeln!(out, "{} is {}", cmd_arg, path.display());
            } else {
                let _ = writeln!(out, "{}: not found", cmd_arg);
            }
        }
    } else if cmd == "complete" {
        if args.first().map(|s| s.as_str()) == Some("-C") {
            if args.len() >= 3 {
                let script_path = &args[1];
                let target_cmd = &args[2];
                completion_specs.insert(target_cmd.clone(), script_path.clone());
            }
        } else if args.first().map(|s| s.as_str()) == Some("-p") {
            if let Some(target_cmd) = args.get(1) {
                if let Some(script_path) = completion_specs.get(target_cmd) {
                    let _ = writeln!(out, "complete -C '{}' {}", script_path, target_cmd);
                } else {
                    let _ = writeln!(err_out, "complete: {}: no completion specification", target_cmd);
                }
            }
        } else if args.first().map(|s| s.as_str()) == Some("-r") {
            if let Some(target_cmd) = args.get(1) {
                completion_specs.remove(target_cmd);
            }
        }
    } else if cmd == "jobs" {
        let total_jobs = jobs.len();
        let mut remaining_jobs = Vec::new();

        for (idx, mut job) in std::mem::take(jobs).into_iter().enumerate() {
            let marker = if idx == total_jobs - 1 {
                '+'
            } else if idx == total_jobs - 2 {
                '-'
            } else {
                ' '
            };

            let is_finished = match job.child.try_wait() {
                Ok(Some(_status)) => true,
                _ => false,
            };

            if is_finished {
                let status_padded = format!("{:24}", "Done");
                let display_cmd = job.command.trim_end_matches('&').trim_end();
                let _ = writeln!(out, "[{}]{} {}  {}", job.id, marker, status_padded, display_cmd);
            } else {
                let status_padded = format!("{:24}", "Running");
                let _ = writeln!(out, "[{}]{} {}  {}", job.id, marker, status_padded, job.command);
                remaining_jobs.push(job);
            }
        }
        *jobs = remaining_jobs;
    }else if cmd == "history" {
        // Print all recorded commands with right-aligned numbering
        for (i, record) in history.iter().enumerate() {
            let _ = writeln!(out, "{:>5}  {}", i + 1, record);
        }
    } else if cmd == "exit" {
        std::process::exit(0);
    }
}

fn main() {
    enable_raw_mode();

    let mut completion_specs: BTreeMap<String, String> = BTreeMap::new();

    // List to track active background jobs
    let mut jobs: Vec<Job> = Vec::new();

    // Add this outside your main loop
    let mut command_history: Vec<String> = Vec::new();

    loop {
        // Automatically reap finished jobs before displaying the prompt
        reap_jobs(&mut jobs);

        print!("$ ");
        io::stdout().flush().unwrap();

        let command = read_line_with_completion(&completion_specs);

        let mut command = String::new();
        io::stdin().read_line(&mut command).unwrap();
        let trimmed_cmd = command.trim().to_string();

        if trimmed_cmd.is_empty() {
            continue;
        }

        // Record command in history BEFORE executing
        command_history.push(trimmed_cmd.clone());

        let command = command.trim_end_matches(['\n', '\r']);
        if command.is_empty() {
            continue;
        }

        let parts = parse_command_line(command);

        // Pipeline handling (|) with built-in support
        if parts.contains(&"|".to_string()) {
            let stages: Vec<Vec<String>> = parts
                .split(|p| p == "|")
                .map(|s| s.to_vec())
                .collect();

            let total_stages = stages.len();
            let mut children = Vec::new();
            let mut prev_reader: Option<std::os::unix::net::UnixStream> = None;

            for (idx, stage) in stages.iter().enumerate() {
                let is_last = idx == total_stages - 1;

                let mut stdout_redir: Option<Redirection> = None;
                let mut stderr_redir: Option<Redirection> = None;
                let mut clean_stage = Vec::new();

                let mut i = 0;
                while i < stage.len() {
                    if (stage[i] == ">>" || stage[i] == "1>>") && i + 1 < stage.len() {
                        stdout_redir = Some(Redirection {
                            path: stage[i + 1].clone(),
                            mode: RedirectMode::Append,
                        });
                        i += 2;
                    } else if (stage[i] == ">" || stage[i] == "1>") && i + 1 < stage.len() {
                        stdout_redir = Some(Redirection {
                            path: stage[i + 1].clone(),
                            mode: RedirectMode::Overwrite,
                        });
                        i += 2;
                    } else if stage[i] == "2>>" && i + 1 < stage.len() {
                        stderr_redir = Some(Redirection {
                            path: stage[i + 1].clone(),
                            mode: RedirectMode::Append,
                        });
                        i += 2;
                    } else if stage[i] == "2>" && i + 1 < stage.len() {
                        stderr_redir = Some(Redirection {
                            path: stage[i + 1].clone(),
                            mode: RedirectMode::Overwrite,
                        });
                        i += 2;
                    } else {
                        clean_stage.push(stage[i].clone());
                        i += 1;
                    }
                }

                if clean_stage.is_empty() {
                    continue;
                }

                let cmd = clean_stage[0].as_str();
                let args = &clean_stage[1..];

                // Create standard pipe stream pair for downstream stage
                let (next_reader, next_writer) = if !is_last {
                    let (r, w) = std::os::unix::net::UnixStream::pair().unwrap();
                    (Some(r), Some(w))
                } else {
                    (None, None)
                };

                if is_builtin(cmd) {
                    // Close upstream read pipe if present
                    drop(prev_reader.take());

                    let mut out_stream: Box<dyn Write> = if let Some(ref redir) = stdout_redir {
                        if let Ok(file) = open_file_for_redir(redir) {
                            Box::new(file)
                        } else {
                            Box::new(io::stdout())
                        }
                    } else if let Some(writer) = next_writer {
                        Box::new(writer)
                    } else {
                        Box::new(io::stdout())
                    };

                    let mut err_stream: Box<dyn Write> = if let Some(ref redir) = stderr_redir {
                        if let Ok(file) = open_file_for_redir(redir) {
                            Box::new(file)
                        } else {
                            Box::new(io::stderr())
                        }
                    } else {
                        Box::new(io::stderr())
                    };

// Update this line wherever execute_builtin is called:
                    execute_builtin(
                        cmd, 
                        args, 
                        &mut completion_specs, 
                        &mut jobs, 
                        &command_history, // <--- Pass the history reference here
                        &mut out_stream, 
                        &mut err_stream
                    );                
                } else if let Some(_path) = get_executable_path(cmd) {
                    let mut child_cmd = Command::new(cmd);
                    child_cmd.args(args);

                    if let Some(reader) = prev_reader.take() {
                        child_cmd.stdin(Stdio::from(OwnedFd::from(reader)));
                    } else {
                        child_cmd.stdin(Stdio::inherit());
                    }

                    if let Some(ref redir) = stdout_redir {
                        if let Ok(file) = open_file_for_redir(redir) {
                            child_cmd.stdout(Stdio::from(OwnedFd::from(file)));
                        }
                    } else if let Some(writer) = next_writer {
                        child_cmd.stdout(Stdio::from(OwnedFd::from(writer)));
                    }

                    if let Some(ref redir) = stderr_redir {
                        if let Ok(file) = open_file_for_redir(redir) {
                            child_cmd.stderr(Stdio::from(file));
                        }
                    }

                    if let Ok(child) = child_cmd.spawn() {
                        children.push(child);
                    }
                } else {
                    eprintln!("{}: command not found", cmd);
                }

                prev_reader = next_reader;
            }

            for mut child in children {
                let _ = child.wait();
            }

            continue;
        }

        if parts.is_empty() {
            continue;
        }

        let mut stdout_redir: Option<Redirection> = None;
        let mut stderr_redir: Option<Redirection> = None;
        let mut clean_parts = Vec::new();

        let mut i = 0;
        while i < parts.len() {
            if (parts[i] == ">>" || parts[i] == "1>>") && i + 1 < parts.len() {
                stdout_redir = Some(Redirection {
                    path: parts[i + 1].clone(),
                    mode: RedirectMode::Append,
                });
                i += 2;
            } else if (parts[i] == ">" || parts[i] == "1>") && i + 1 < parts.len() {
                stdout_redir = Some(Redirection {
                    path: parts[i + 1].clone(),
                    mode: RedirectMode::Overwrite,
                });
                i += 2;
            } else if parts[i] == "2>>" && i + 1 < parts.len() {
                stderr_redir = Some(Redirection {
                    path: parts[i + 1].clone(),
                    mode: RedirectMode::Append,
                });
                i += 2;
            } else if parts[i] == "2>" && i + 1 < parts.len() {
                stderr_redir = Some(Redirection {
                    path: parts[i + 1].clone(),
                    mode: RedirectMode::Overwrite,
                });
                i += 2;
            } else {
                clean_parts.push(parts[i].clone());
                i += 1;
            }
        }

        if clean_parts.is_empty() {
            continue;
        }

        // Check if the command line ends with '&' token to run in background
        let is_background = clean_parts.last().map(|s| s.as_str()) == Some("&");
        if is_background {
            clean_parts.pop();
        }

        if clean_parts.is_empty() {
            continue;
        }

        let cmd = clean_parts[0].as_str();
        let args = &clean_parts[1..];

        // Add the command to the history
        command_history.push(parts.join(" "));

        if let Some(ref redir) = stdout_redir {
            let _ = open_file_for_redir(redir);
        }
        if let Some(ref redir) = stderr_redir {
            let _ = open_file_for_redir(redir);
        }

        let write_stdout = |output: &str| {
            if let Some(ref redir) = stdout_redir {
                if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(&redir.path) {
                    let _ = writeln!(file, "{}", output);
                }
            } else {
                println!("{}", output);
            }
        };

        let write_stderr = |output: &str| {
            if let Some(ref redir) = stderr_redir {
                if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(&redir.path) {
                    let _ = writeln!(file, "{}", output);
                }
            } else {
                eprintln!("{}", output);
            }
        };

        if cmd == "exit" {
            break;
        } else if cmd == "echo" {
            write_stdout(&args.join(" "));
        } else if cmd == "pwd" {
            if let Ok(current_dir) = std::env::current_dir() {
                write_stdout(&current_dir.display().to_string());
            }
        } else if cmd == "cd" {
            let target = match args.first().map(|s| s.as_str()) {
                Some("~") | None => std::env::var("HOME").ok(),
                Some(path) => Some(path.to_string()),
            };

            if let Some(target_path) = target {
                if let Err(_) = std::env::set_current_dir(&target_path) {
                    write_stderr(&format!("cd: {}: No such file or directory", target_path));
                }
            }
        } else if cmd == "type" {
            if let Some(cmd_arg) = args.first() {
                if cmd_arg == "echo" || cmd_arg == "type" || cmd_arg == "exit" || cmd_arg == "pwd" || cmd_arg == "complete" || cmd_arg == "jobs" {
                    write_stdout(&format!("{} is a shell builtin", cmd_arg));
                } else if let Some(path) = get_executable_path(cmd_arg) {
                    write_stdout(&format!("{} is {}", cmd_arg, path.display()));
                } else {
                    write_stdout(&format!("{}: not found", cmd_arg));
                }
            }
        } else if cmd == "complete" {
            if args.first().map(|s| s.as_str()) == Some("-C") {
                if args.len() >= 3 {
                    let script_path = &args[1];
                    let target_cmd = &args[2];
                    completion_specs.insert(target_cmd.clone(), script_path.clone());
                }
            } else if args.first().map(|s| s.as_str()) == Some("-p") {
                if let Some(target_cmd) = args.get(1) {
                    if let Some(script_path) = completion_specs.get(target_cmd) {
                        write_stdout(&format!("complete -C '{}' {}", script_path, target_cmd));
                    } else {
                        write_stderr(&format!("complete: {}: no completion specification", target_cmd));
                    }
                }
            } else if args.first().map(|s| s.as_str()) == Some("-r") {
                if let Some(target_cmd) = args.get(1) {
                    completion_specs.remove(target_cmd);
                }
            }
        } else if cmd == "jobs" {
            let total_jobs = jobs.len();
            let mut remaining_jobs = Vec::new();

            for (idx, mut job) in std::mem::take(&mut jobs).into_iter().enumerate() {
                let marker = if idx == total_jobs - 1 {
                    '+'
                } else if idx == total_jobs - 2 {
                    '-'
                } else {
                    ' '
                };

                let is_finished = match job.child.try_wait() {
                    Ok(Some(_status)) => true,
                    _ => false,
                };

                if is_finished {
                    let status_padded = format!("{:24}", "Done");
                    let display_cmd = job.command.trim_end_matches('&').trim_end();
                    write_stdout(&format!("[{}]{} {}  {}", job.id, marker, status_padded, display_cmd));
                } else {
                    let status_padded = format!("{:24}", "Running");
                    write_stdout(&format!("[{}]{} {}  {}", job.id, marker, status_padded, job.command));
                    remaining_jobs.push(job);
                }
            }
            
            // Retain only the jobs that are still running
            jobs = remaining_jobs;
        } else if let Some(_path) = get_executable_path(cmd) {
            let mut child_cmd = Command::new(cmd);
            child_cmd.args(args);

            if let Some(ref redir) = stdout_redir {
                if let Ok(file) = std::fs::OpenOptions::new().append(true).open(&redir.path) {
                    child_cmd.stdout(Stdio::from(file));
                }
            }

            if let Some(ref redir) = stderr_redir {
                if let Ok(file) = std::fs::OpenOptions::new().append(true).open(&redir.path) {
                    child_cmd.stderr(Stdio::from(file));
                }
            }

            // Spawn non-blocking background process if '&' token was present
            // Spawn non-blocking background process if '&' token was present
            if is_background {
                if let Ok(child) = child_cmd.spawn() {
                    let pid = child.id();
                    
                    // Assign [1] if empty, otherwise 1 + the highest current job ID
                    let job_id = if jobs.is_empty() {
                        1
                    } else {
                        jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1
                    };

                    println!("[{}] {}", job_id, pid);

                    let raw_cmd_string = command.trim_end().to_string();
                    jobs.push(Job {
                        id: job_id,
                        pid,
                        command: raw_cmd_string,
                        child,
                    });
                }
            } else {
                let _ = child_cmd.status();
            }
        } else {
            write_stderr(&format!("{}: command not found", cmd));
        }
    }
}

fn get_executable_completions() -> Vec<String> {
    let mut completions = BTreeSet::new();

    completions.insert("echo".to_string());
    completions.insert("exit".to_string());
    completions.insert("complete".to_string());
    completions.insert("jobs".to_string());

    if let Some(path_var) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&path_var) {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    let full_path = entry.path();
                    if full_path.is_file() {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            if let Ok(metadata) = full_path.metadata() {
                                if metadata.permissions().mode() & 0o111 != 0 {
                                    if let Some(name) = full_path.file_name().and_then(|n| n.to_str()) {
                                        completions.insert(name.to_string());
                                    }
                                }
                            }
                        }
                        #[cfg(not(unix))]
                        {
                            if let Some(name) = full_path.file_name().and_then(|n| n.to_str()) {
                                completions.insert(name.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    completions.into_iter().collect()
}

fn get_path_completions(path_str: &str) -> (String, Vec<String>) {
    let mut completions = BTreeSet::new();

    let (dir_path, prefix) = if let Some(last_slash_idx) = path_str.rfind('/') {
        (&path_str[..=last_slash_idx], &path_str[last_slash_idx + 1..])
    } else {
        ("", path_str)
    };

    let search_dir = if dir_path.is_empty() {
        Path::new(".")
    } else {
        Path::new(dir_path)
    };

    if let Ok(entries) = std::fs::read_dir(search_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with(prefix) {
                    let full_item_path = search_dir.join(name);
                    if full_item_path.is_dir() {
                        completions.insert(format!("{}{}/", dir_path, name));
                    } else {
                        completions.insert(format!("{}{}", dir_path, name));
                    }
                }
            }
        }
    }

    (dir_path.to_string(), completions.into_iter().collect())
}

fn longest_common_prefix(strs: &[&String]) -> String {
    if strs.is_empty() {
        return String::new();
    }

    let mut prefix = strs[0].to_string();
    for s in &strs[1..] {
        while !s.starts_with(&prefix) {
            prefix.pop();
            if prefix.is_empty() {
                break;
            }
        }
    }
    prefix
}

fn run_completer_script(
    script_path: &str,
    command_name: &str,
    word_being_completed: &str,
    previous_word: &str,
    full_line: &str,
) -> Option<Vec<String>> {
    let output = Command::new(script_path)
        .arg(command_name)
        .arg(word_being_completed)
        .arg(previous_word)
        .env("COMP_LINE", full_line)
        .env("COMP_POINT", full_line.len().to_string())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout_str = String::from_utf8(output.stdout).ok()?;
    let mut lines: Vec<String> = stdout_str
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    lines.sort();
    lines.dedup();

    Some(lines)
}

fn read_line_with_completion(completion_specs: &BTreeMap<String, String>) -> String {
    let mut buffer = String::new();
    let mut stdin = io::stdin();
    let mut tab_count = 0;

    loop {
        let mut byte = [0u8; 1];
        if stdin.read_exact(&mut byte).is_err() {
            break;
        }

        match byte[0] {
            b'\t' => {
                let is_arg_completion = buffer.contains(' ');

                if is_arg_completion {
                    let first_space_idx = buffer.find(' ').unwrap();
                    let command_name = &buffer[..first_space_idx];

                    if let Some(script_path) = completion_specs.get(command_name) {
                        let tokens: Vec<&str> = buffer.split_whitespace().collect();

                        let (word_being_completed, previous_word) = if buffer.ends_with(' ') {
                            ("", tokens.last().copied().unwrap_or(""))
                        } else {
                            let curr = tokens.last().copied().unwrap_or("");
                            let prev = if tokens.len() >= 2 { tokens[tokens.len() - 2] } else { "" };
                            (curr, prev)
                        };

                        let previous_word_str = if previous_word == command_name && tokens.len() == 1 {
                            command_name
                        } else {
                            previous_word
                        };

                        if let Some(script_lines) = run_completer_script(
                            script_path,
                            command_name,
                            word_being_completed,
                            previous_word_str,
                            &buffer,
                        ) {
                            let match_refs: Vec<&String> = script_lines.iter().collect();

                            if match_refs.is_empty() {
                                print!("\x07");
                                io::stdout().flush().unwrap();
                                tab_count = 0;
                            } else if match_refs.len() == 1 {
                                let matched_str = match_refs[0];
                                let last_space_idx = buffer.rfind(' ').unwrap();
                                let base_prefix = &buffer[..=last_space_idx];
                                let completed = format!("{}{}{} ", base_prefix, matched_str, if matched_str.ends_with('/') { "" } else { "" });
                                print!("\r$ {}\x1b[K", completed);
                                io::stdout().flush().unwrap();
                                buffer = completed;
                                tab_count = 0;
                            } else {
                                let lcp = longest_common_prefix(&match_refs);

                                if lcp.len() > word_being_completed.len() {
                                    let last_space_idx = buffer.rfind(' ').unwrap();
                                    let base_prefix = &buffer[..=last_space_idx];
                                    let completed = format!("{}{}", base_prefix, lcp);
                                    print!("\r$ {}\x1b[K", completed);
                                    io::stdout().flush().unwrap();
                                    buffer = completed;
                                    tab_count = 0;
                                } else if tab_count == 0 {
                                    print!("\x07");
                                    io::stdout().flush().unwrap();
                                    tab_count = 1;
                                } else {
                                    let options = match_refs.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("  ");
                                    print!("\n{}\n$ {}", options, buffer);
                                    io::stdout().flush().unwrap();
                                    tab_count = 0;
                                }
                            }
                            continue;
                        }
                    }

                    let last_space_idx = buffer.rfind(' ').unwrap();
                    let base_prefix = &buffer[..=last_space_idx];
                    let partial_arg = &buffer[last_space_idx + 1..];

                    let (_, matches) = get_path_completions(partial_arg);
                    let match_refs: Vec<&String> = matches.iter().collect();

                    if match_refs.is_empty() {
                        print!("\x07");
                        io::stdout().flush().unwrap();
                        tab_count = 0;
                    } else if match_refs.len() == 1 {
                        let matched_str = match_refs[0];
                        let suffix = if matched_str.ends_with('/') { "" } else { " " };
                        let completed = format!("{}{}{}", base_prefix, matched_str, suffix);
                        print!("\r$ {}\x1b[K", completed);
                        io::stdout().flush().unwrap();
                        buffer = completed;
                        tab_count = 0;
                    } else {
                        let lcp = longest_common_prefix(&match_refs);

                        if lcp.len() > partial_arg.len() {
                            let completed = format!("{}{}", base_prefix, lcp);
                            print!("\r$ {}\x1b[K", completed);
                            io::stdout().flush().unwrap();
                            buffer = completed;
                            tab_count = 0;
                        } else if tab_count == 0 {
                            print!("\x07");
                            io::stdout().flush().unwrap();
                            tab_count = 1;
                        } else {
                            let display_names: Vec<&str> = match_refs.iter().map(|s| {
                                let trimmed = s.strip_suffix('/').unwrap_or(s);
                                if let Some(slash_idx) = trimmed.rfind('/') {
                                    &s[slash_idx + 1..]
                                } else {
                                    s.as_str()
                                }
                            }).collect();
                            let options = display_names.join("  ");
                            print!("\n{}\n$ {}", options, buffer);
                            io::stdout().flush().unwrap();
                            tab_count = 0;
                        }
                    }
                } else {
                    let candidates = get_executable_completions();
                    let matches: Vec<&String> = candidates.iter().filter(|c| c.starts_with(&buffer)).collect();

                    if matches.is_empty() {
                        print!("\x07");
                        io::stdout().flush().unwrap();
                        tab_count = 0;
                    } else if matches.len() == 1 {
                        let completed = format!("{} ", matches[0]);
                        print!("\r$ {}\x1b[K", completed);
                        io::stdout().flush().unwrap();
                        buffer = completed;
                        tab_count = 0;
                    } else {
                        let lcp = longest_common_prefix(&matches);

                        if lcp.len() > buffer.len() {
                            let completed = lcp;
                            print!("\r$ {}\x1b[K", completed);
                            io::stdout().flush().unwrap();
                            buffer = completed;
                            tab_count = 0;
                        } else if tab_count == 0 {
                            print!("\x07");
                            io::stdout().flush().unwrap();
                            tab_count = 1;
                        } else {
                            let options = matches.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("  ");
                            print!("\n{}\n$ {}", options, buffer);
                            io::stdout().flush().unwrap();
                            tab_count = 0;
                        }
                    }
                }
            }
            b'\n' | b'\r' => {
                println!();
                break;
            }
            127 | 8 => {
                tab_count = 0;
                if !buffer.is_empty() {
                    buffer.pop();
                    print!("\r$ {}\x1b[K", buffer);
                    io::stdout().flush().unwrap();
                }
            }
            b => {
                tab_count = 0;
                buffer.push(b as char);
                print!("{}", b as char);
                io::stdout().flush().unwrap();
            }
        }
    }

    buffer
}

#[cfg(unix)]
fn enable_raw_mode() {
    use std::os::unix::io::AsRawFd;

    #[repr(C)]
    struct Termios {
        c_iflag: u32,
        c_oflag: u32,
        c_cflag: u32,
        c_lflag: u32,
        c_line: u8,
        c_cc: [u8; 32],
        c_ispeed: u32,
        c_ospeed: u32,
    }

    unsafe extern "C" {
        fn tcgetattr(fd: i32, termios_p: *mut Termios) -> i32;
        fn tcsetattr(fd: i32, optional_actions: i32, termios_p: *const Termios) -> i32;
    }

    const ICANON: u32 = 0x00000002;
    const ECHO: u32 = 0x00000008;
    const TCSANOW: i32 = 0;

    unsafe {
        let fd = io::stdin().as_raw_fd();
        let mut termios: Termios = std::mem::zeroed();
        if tcgetattr(fd, &mut termios) == 0 {
            termios.c_lflag &= !(ICANON | ECHO);
            tcsetattr(fd, TCSANOW, &termios);
        }
    }
}

#[cfg(not(unix))]
fn enable_raw_mode() {}

fn open_file_for_redir(redir: &Redirection) -> io::Result<File> {
    match redir.mode {
        RedirectMode::Overwrite => File::create(&redir.path),
        RedirectMode::Append => std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&redir.path),
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