use nix::{sys::{signal::{signal, SigHandler, Signal}, wait::waitpid}, unistd::{chdir, execvp, fork, getpgrp, getpid, setpgid, tcsetpgrp, write, ForkResult, Pid}};
use std::io::{BufRead, Write};
use std::ffi::CString;
use std::process::exit;
use std::env;
use std::path::PathBuf;
use regex::Regex;

struct Shell {
    shell_pid: Pid,
    last_status: i32,
    stdin_handle: std::io::Stdin,
    stdout_handle: std::io::Stdout,
    // TODO: jobs table
}

impl Shell {
    fn new() -> nix::Result<Self> {
        // ignore signals
        unsafe {
            // required when shell process is not foreground and uses tcsetpgrp
            signal(Signal::SIGTTOU, SigHandler::SigIgn)?;
            // required for ignoring ctrl-z
            signal(Signal::SIGTSTP, SigHandler::SigIgn)?;
        }
        let shell_pid = getpid();
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        setpgid(shell_pid, shell_pid)?;
        tcsetpgrp(&stdin, shell_pid)?;
        Ok(Self {
            last_status: 0,
            shell_pid: shell_pid,
            stdin_handle: stdin,
            stdout_handle: stdout,
        })
    }

    fn run(&mut self) -> nix::Result<()> {
        loop {
            print!("\n$ ");
            self.stdout_handle.flush().unwrap();

            let mut input = String::new();
            let bytes_read = self.stdin_handle.lock().read_line(&mut input).unwrap();

            if bytes_read == 0 {
                println!("\nexit");
                exit(0);
            }

            self.handle_line(input)?;

        }
    }

    fn handle_line(&mut self, line: String) -> nix::Result<()> {
        let re = Regex::new(r"\$([a-zA-Z0-9_]+|\$|!)").unwrap();

        let result = re.replace_all(line.as_str(), |captures: &regex::Captures| {
            let var_name = &captures[1];
            env::var(var_name).unwrap_or_else(|_| "".to_string())
        });
        let line = result.into_owned();

        if let Some(command) = Command::from_input(line) {
            command.execute();
        }
        Ok(())
    }
}


struct Command {
    // TODO: look into OsString for POSIX compatibility
    cmd: String,
    args: Vec<String>,
}

impl Command {
    fn from_input(input: String) -> Option<Self> {
        // for now we just split at whitespace
        // TODO: split correctly for quotes etc.
        let splitted: Vec<&str> = input.as_str().trim().split_whitespace().collect();

        if splitted.is_empty() {
            return None
        }

        let cmd = splitted[0].to_string();
        let args = splitted.iter()
            .map(|arg| arg.to_string())
            .collect();

        Some(Self {
            cmd,
            args,
        })

    }

    fn cmd_as_cstring(&self) -> CString {
        CString::new(self.cmd.as_str()).unwrap()
    }

    fn args_as_cstring(&self) -> Vec<CString> {
        self.args.iter()
            .map(|arg| CString::new(arg.as_str()).unwrap())
            .collect()
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

                // TODO: handle cd - with OLDPWD env variable
                // TODO: set new PWD variable
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

fn main() {
    let mut shell = Shell::new().expect("Failed to spawn shell");
    shell.run().expect("Failed to run shell");
}
