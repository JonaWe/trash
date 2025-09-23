use nix::{sys::{signal::{signal, SigHandler, Signal}, wait::waitpid}, unistd::{chdir, execvp, fork, getpgrp, getpid, setpgid, tcsetpgrp, write, ForkResult, Pid}};
use std::{io::{BufRead, Write}};
use std::ffi::CString;
use std::process::exit;
use std::env;
use std::path::PathBuf;
use regex::Regex;

enum Command {
    Builtin(BuiltinCommand),
    External(ExternalCommand),
}

enum BuiltinCommand {
    Exit,
    Cd(Vec<String>),
}

enum Token {
    Andpercent,
    Semicolon,
    And,
    Or,
    Pipe,
    Word(String),
    /// for single quote strings that do not expand variables
    LiteralWord(String),
}



fn tokenize(input: &str) -> Vec<Token> {
    let mut single_quotes = false;
    let mut double_quotes = false;
    let mut chars  = input.chars().peekable();
    let mut tokens: Vec<Token> = Vec::new();
    let mut current = String::new();

    while let Some(current_char) = chars.next() {
        match current_char {
            _ if current_char.is_whitespace() && !current.is_empty() => {
                tokens.push(Token::Word(current.clone()));
                current.clear();
            }
            '\'' if !double_quotes => {
                if !current.is_empty() {
                    let word = if single_quotes {
                        Token::LiteralWord(current.clone())
                    } else {
                        Token::Word(current.clone())
                    };
                    tokens.push(word);
                    current.clear();
                }
                single_quotes = !single_quotes;
            }
            '"' if !single_quotes => {
                if !current.is_empty() {
                    tokens.push(Token::Word(current.clone()));
                    current.clear();
                }
                double_quotes = !double_quotes;
            }
            '&' if !single_quotes && !double_quotes => {
                if !current.is_empty() {
                    tokens.push(Token::Word(current.clone()));
                    current.clear();
                }
                if let Some(&ch) = chars.peek() {
                    if ch == '&' {
                        chars.next();
                        tokens.push(Token::And);
                    } else {
                        tokens.push(Token::Andpercent);
                    }
                } else {
                    tokens.push(Token::Andpercent);
                }
            }
            ';' if !single_quotes && !double_quotes => {
                if !current.is_empty() {
                    tokens.push(Token::Word(current.clone()));
                    current.clear();
                }
                tokens.push(Token::Semicolon);
            }
            ch if single_quotes => {
                current.push(ch);
            }
            _ => current.push(current_char)
        }
    }

    // for now if a quote is opened and not closed the whole content is just discarded
    if !current.is_empty() && !single_quotes && !double_quotes {
        tokens.push(Token::Word(current));
    }

    tokens
}

struct Parser {
    // variable_regex: Regex,
}

impl Parser {
    fn new() -> Self {
        // let variable_regex = Regex::new(r"\$([a-zA-Z0-9_]+|\$|!)").unwrap();
        Self {
            // variable_regex,
        }
    }

    fn parse(&self, tokens: Vec<Token>) -> Option<Command> {
        if tokens.is_empty() {
            None
        } else {
            let args: Vec<String> = tokens.into_iter().filter_map(|token| {
                match token {
                    Token::Word(word) => Some(word),
                    _ => None,
                }
            }).collect();

            match args[0].as_str() {
                "exit" => Some(Command::Builtin(BuiltinCommand::Exit)),
                "cd" => Some(Command::Builtin(BuiltinCommand::Cd(args))),
                command => {
                    let external_command = ExternalCommand::new(command.to_string(), args);
                    Some(Command::External(external_command))
                },
            }
        }
    }
}

struct Shell {
    shell_pid: Pid,
    last_status: i32,
    stdin_handle: std::io::Stdin,
    stdout_handle: std::io::Stdout,
    variable_regex: Regex,
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

        let variable_regex = Regex::new(r"\$([a-zA-Z0-9_]+|\$|!)").unwrap();

        Ok(Self {
            last_status: 0,
            shell_pid: shell_pid,
            stdin_handle: stdin,
            stdout_handle: stdout,
            variable_regex,
        })
    }

    fn run(&mut self) -> nix::Result<()> {
        let parser = Parser::new();
        loop {
            print!("\n$ ");
            self.stdout_handle.flush().unwrap();

            let mut input = String::new();
            let bytes_read = self.stdin_handle.lock().read_line(&mut input).unwrap();

            if bytes_read == 0 {
                println!("\nexit");
                exit(0);
            }

            let tokens = tokenize(input.as_str());

            if let Some(command) = parser.parse(tokens) {
                self.execute(command)?;
            }
        }
    }

    fn execute(&self, command: Command) -> nix::Result<()> {
        match command {
            Command::Builtin(builtin) => {
                self.handle_builtin(builtin)
            }
            Command::External(external) => {
                self.spawn_foreground(external)
            }
        }
    }

    fn spawn_foreground(&self, command: ExternalCommand) -> nix::Result<()> {
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
                let _ = execvp(&command.cmd_as_cstring(), &command.args_as_cstring());
                write(std::io::stdout(), b"command not found\n").ok();
                unsafe { libc::_exit(127) };
            }
            Err(_) => {
                println!("Fork failed");
            }
        }
        Ok(())
    }

    fn handle_builtin(&self, builtin: BuiltinCommand) -> nix::Result<()> {
        match builtin {
            BuiltinCommand::Exit => {
                println!("exit");
                exit(0);
            }
            BuiltinCommand::Cd(args) => {
                let target = if args.len() == 2 {
                    // TODO: handle last directory with -
                    // TODO: handle home with ~
                    PathBuf::from(args[1].as_str())
                } else if args.len() == 1{
                    match env::var("HOME") {
                        Ok(home) => PathBuf::from(home),
                        Err(_) => {
                            eprintln!("cd: HOME is not set");
                            return Err(nix::Error::EINVAL);
                        }
                    }
                } else {
                    eprintln!("cd: too many arguments");
                    return Err(nix::Error::EINVAL);
                };

                // TODO: handle cd - with OLDPWD env variable
                // TODO: set new PWD variable
                if let Err(e) = chdir(&target) {
                    eprintln!("cd: {}", e);
                }
            }
        }

        Ok(())
    }
}


struct ExternalCommand {
    // TODO: look into OsString for POSIX compatibility
    cmd: String,
    args: Vec<String>,
    // redirects: Vec<Redirect>,
    // background: bool,
}

impl ExternalCommand {
    fn new(cmd: String, args: Vec<String>) -> Self {
        Self {
            cmd,
            args,
        }
    }

    fn cmd_as_cstring(&self) -> CString {
        CString::new(self.cmd.as_str()).unwrap()
    }

    fn args_as_cstring(&self) -> Vec<CString> {
        self.args.iter()
            .map(|arg| CString::new(arg.as_str()).unwrap())
            .collect()
    }
}

fn main() {
    let mut shell = Shell::new().expect("Failed to spawn shell");
    shell.run().expect("Failed to run shell");
}
