#[allow(unused_imports)]
use std::io::{self, Write};

fn main() {
    loop{
        print!("$ ");
        io::stdout().flush().unwrap();

        let mut command=String::new();
        io::stdin().read_line(&mut command).unwrap();
        command=command.trim().to_string();
        if(command=="exit"){
            break;
        }else if command.starts_with("echo "){
            println!("{}", &command[5..]);
        }else if command.starts_with("type "){
            if(command[5..].trim()=="echo" || command[5..].trim()=="type" || command[5..].trim()=="exit"){
                println!("{} is a shell builtin", command[5..].trim());
            }else{
                println!("invalid_command: not found");
            }
        }else{
            println!("{}: command not found",command);
        }
    }
}