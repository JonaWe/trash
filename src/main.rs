use nix::{sys::wait::waitpid,unistd::{chdir, execvp, fork, write, ForkResult}};
use std::io::{self, BufRead, Write};
use std::ffi::CString;
use std::process::exit;
use std::env;
use std::path::PathBuf;

struct Command {
    // TODO: look into OsString for POSIX compatibility
    cmd: String,
    args: Vec<String>,
}

impl Command {
    fn cmd_as_cstring(&self) -> CString {
        CString::new(self.cmd.as_str()).unwrap()
    }
    fn args_as_cstring(&self) -> Vec<CString> {
        self.args.iter()
            .map(|arg| CString::new(arg.as_str()).unwrap())
            .collect()
    }

    fn new(cmd: String, args: Vec<String>) -> Self {
        Self { cmd, args }
    }
}


fn parse_input(input: &str) -> Option<Command> {
    // for now we just split at whitespace
    // TODO: split correctly for quotes etc.
    let splitted: Vec<&str> = input.trim().split_whitespace().collect();

    if splitted.is_empty() {
        return None
    }

    let cmd = splitted[0].to_string();
    let args = splitted.iter()
        .map(|arg| arg.to_string())
        .collect();

    Some(Command::new(cmd, args))
}

fn command_handler(command: Command) {
    match command.cmd.as_str() {
        "exit" => {
            println!("exit");
            exit(0);
        }
        "cd" => {
            let target = if command.args.len() == 2 {
                // TODO: handle last directory with -
                PathBuf::from(command.args[1].as_str())
            } else if command.args.len() == 1{
                match env::var("HOME") {
                    Ok(home) => PathBuf::from(home),
                    Err(_) => {
                        eprintln!("cd: HOME is not set");
                        return;
                    }
                }
            } else {
                eprintln!("cd: too many arguments");
                return
            };

            if let Err(e) = chdir(&target) {
                eprintln!("cd: {}", e);
                return;
            }
        }
        _ => run_command(command),
    }

}

fn run_command(command: Command) {
    match unsafe{fork()} {
        Ok(ForkResult::Parent { child, .. }) => {
            // TODO: handle potential errors here
            waitpid(child, None).unwrap();
        }
        Ok(ForkResult::Child) => {
            // Unsafe to use `println!` (or `unwrap`) here. See Safety.
            // write(std::io::stdout(), "I'm a new child process\n".as_bytes()).ok();
            let _ = execvp(&command.cmd_as_cstring(), &command.args_as_cstring());
            write(std::io::stdout(), "Command not found!\n".as_bytes()).ok();
            std::process::exit(1);
            // unsafe { libc::_exit(0) };
        }
        Err(_) => println!("Fork failed"),
    }
}

fn main() {
    loop {
        print!("$ ");
        io::stdout().flush().unwrap();
        let stdin = io::stdin();
        let mut input = String::new();
        let bytes_read = stdin.lock().read_line(&mut input).unwrap();

        if bytes_read == 0 {
            println!("\nexit");
            exit(0);
        }

        if let Some(command) = parse_input(input.as_str()) {
            command_handler(command);
        } else {
            continue;
        }
    }
}
