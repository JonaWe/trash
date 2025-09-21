use nix::{sys::{signal::{signal, kill, SigHandler, Signal::{self, SIGTSTP, SIGTTIN, SIGTTOU}}, wait::waitpid}, unistd::{chdir, execvp, fork, getpgrp, getpid, setpgid, tcgetpgrp, tcsetpgrp, write, ForkResult, Pid}};
use std::io::{self, BufRead, Write};
use std::ffi::CString;
use std::process::exit;
use std::env;
use std::path::PathBuf;
use regex::Regex;

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

    fn execute(&self) {
        match self.cmd.as_str() {
            "exit" => {
                println!("exit");
                exit(0);
            }
            "cd" => {
                let target = if self.args.len() == 2 {
                    // TODO: handle last directory with -
                    PathBuf::from(self.args[1].as_str())
                } else if self.args.len() == 1{
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
            _ => {
                match unsafe{fork()} {
                    Ok(ForkResult::Parent { child, .. }) => {
                        let _ = setpgid(child, child);
                        let _ = tcsetpgrp(&std::io::stdin(), child);
                        // maybe WUNTRACED/WCONTINUED later for ctrl-z job controll
                        waitpid(child, None).unwrap();
                        let pgid = getpgrp();
                        let _ = tcsetpgrp(&std::io::stdin(), pgid);
                    }
                    Ok(ForkResult::Child) => {
                        let _ = setpgid(Pid::from_raw(0), Pid::from_raw(0));
                        let _ = execvp(&self.cmd_as_cstring(), &self.args_as_cstring());
                        write(std::io::stdout(), b"command not found\n").ok();
                        unsafe { libc::_exit(127) };
                    }
                    Err(_) => {
                        println!("Fork failed");
                    }
                }
            }

        }
    }
}

fn replace_variables(input: String) -> String {
    let re = Regex::new(r"\$([a-zA-Z0-9_]+|\$|!)").unwrap();

    let result = re.replace_all(input.as_str(), |captures: &regex::Captures| {
        let var_name = &captures[1];
        env::var(var_name).unwrap_or_else(|_| "".to_string())
    });
    result.into_owned()
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


fn claim_terminal() -> nix::Result<()> {
    // ignore signals
    unsafe {
        // required when shell process is not foreground and uses tcsetpgrp
        signal(SIGTTOU, SigHandler::SigIgn)?;
        // required for ignoring ctrl-z
        signal(SIGTSTP, SigHandler::SigIgn)?;
    }
    let shell_pid = getpid();
    setpgid(shell_pid, shell_pid)?;
    tcsetpgrp(&std::io::stdin(), shell_pid)?;
    Ok(())
}

fn main() {
    claim_terminal().expect("Failed to get foreground for terminal");
    loop {
        print!("\n$ ");
        io::stdout().flush().unwrap();
        let stdin = io::stdin();
        let mut input = String::new();
        let bytes_read = stdin.lock().read_line(&mut input).unwrap();

        if bytes_read == 0 {
            println!("\nexit");
            exit(0);
        }

        input = replace_variables(input);

        if let Some(command) = parse_input(input.as_str()) {
            command.execute();
        } else {
            continue;
        }
    }
}
