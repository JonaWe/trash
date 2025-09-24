use nix::sys::signal::{SigHandler, Signal, signal};
use nix::sys::wait::waitpid;
use nix::unistd::{
    ForkResult, Pid, chdir, execvp, fork, getpgrp, getpid, setpgid, tcsetpgrp, write,
};
use regex::Regex;
use std::env;
use std::ffi::CString;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::exit;

enum Command {
    Builtin(BuiltinCommand),
    External(ExternalCommand),
}

enum BuiltinCommand {
    Exit,
    Cd(Vec<String>),
}

#[derive(Debug, PartialEq, Eq)]
enum Quoting {
    Unquoted,
    SingleQuoted,
    DoubleQuoted,
}

#[derive(Debug, PartialEq, Eq)]
enum Operator {
    And,
    Or,
    Pipe,
    Andpercent,
    Semicolon,
}

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Word(String, Quoting),
    Operator(Operator),
    Whitespace,
}

struct Parser {
    variable_regex: Regex,
}

impl Parser {
    fn new() -> Self {
        let variable_regex = Regex::new(r"\$([a-zA-Z0-9_]+|\$|!)").unwrap();
        Self { variable_regex }
    }

    fn tokenize(&self, input: &str) -> Vec<Token> {
        let mut single_quotes = false;
        let mut double_quotes = false;
        let mut chars = input.chars().peekable();
        let mut tokens: Vec<Token> = Vec::new();
        let mut current = String::new();

        while let Some(current_char) = chars.next() {
            // remove preceding whitespace
            if !single_quotes && !double_quotes && current.trim().is_empty() {
                current.clear();
            }

            match current_char {
                _ if current_char.is_whitespace() && !single_quotes && !double_quotes => {
                    if !current.trim().is_empty() {
                        tokens.push(Token::Word(current.clone(), Quoting::Unquoted));
                        current.clear();
                    }
                    // only push if last token is not whitespace
                    match tokens.last() {
                        Some(Token::Whitespace) => {}
                        _ => {
                            tokens.push(Token::Whitespace);
                            current.clear();
                        }
                    }
                }
                '\'' if !double_quotes => {
                    if !current.is_empty() {
                        let quoting = if single_quotes {
                            Quoting::SingleQuoted
                        } else {
                            Quoting::Unquoted
                        };
                        tokens.push(Token::Word(current.clone(), quoting));
                        current.clear();
                    }
                    single_quotes = !single_quotes;
                }
                '"' if !single_quotes => {
                    if !current.is_empty() {
                        let quoting = if double_quotes {
                            Quoting::DoubleQuoted
                        } else {
                            Quoting::Unquoted
                        };
                        tokens.push(Token::Word(current.clone(), quoting));
                        current.clear();
                    }
                    double_quotes = !double_quotes;
                }
                '&' if !single_quotes && !double_quotes => {
                    if !current.trim().is_empty() {
                        tokens.push(Token::Word(current.clone(), Quoting::Unquoted));
                        current.clear();
                    }
                    if let Some(&ch) = chars.peek() {
                        if ch == '&' {
                            chars.next();
                            tokens.push(Token::Operator(Operator::And));
                        } else {
                            tokens.push(Token::Operator(Operator::Andpercent));
                        }
                    } else {
                        tokens.push(Token::Operator(Operator::Andpercent));
                    }
                    current.clear()
                }
                ';' if !single_quotes && !double_quotes => {
                    if !current.trim().is_empty() {
                        tokens.push(Token::Word(current.clone(), Quoting::Unquoted));
                        current.clear();
                    }
                    tokens.push(Token::Operator(Operator::Semicolon));
                    current.clear()
                }
                '\\' => {
                    if let Some(_) = chars.peek() {
                        match chars.next().unwrap() {
                            'n' => current.push('\n'),
                            't' => current.push('\t'),
                            'r' => current.push('\r'),
                            '0' => current.push('\0'),
                            ch => current.push(ch),
                        }
                    }
                }
                _ => current.push(current_char),
            }
        }

        // for now if a quote is opened and not closed the whole content is just discarded
        if !current.trim().is_empty() && !single_quotes && !double_quotes {
            tokens.push(Token::Word(current, Quoting::Unquoted));
        }

        tokens
    }

    fn parse(&self, tokens: Vec<Token>) -> Option<Command> {
        if tokens.is_empty() {
            None
        } else {
            let args: Vec<String> = tokens
                .into_iter()
                .filter_map(|token| match token {
                    Token::Word(word, Quoting::SingleQuoted) => Some(word),
                    Token::Word(word, _) => Some(
                        self.variable_regex
                            .replace_all(word.as_str(), |caps: &regex::Captures| {
                                let k = &caps[1];
                                env::var(k).unwrap_or_default()
                            })
                            .into_owned(),
                    ),
                    _ => None,
                })
                .collect();

            match args[0].as_str() {
                "exit" => Some(Command::Builtin(BuiltinCommand::Exit)),
                "cd" => Some(Command::Builtin(BuiltinCommand::Cd(args))),
                command => {
                    let external_command = ExternalCommand::new(command.to_string(), args);
                    Some(Command::External(external_command))
                }
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

            let tokens = parser.tokenize(input.as_str());

            if let Some(command) = parser.parse(tokens) {
                self.execute(command)?;
            }
        }
    }

    fn execute(&self, command: Command) -> nix::Result<()> {
        match command {
            Command::Builtin(builtin) => self.handle_builtin(builtin),
            Command::External(external) => self.spawn_foreground(external),
        }
    }

    fn spawn_foreground(&self, command: ExternalCommand) -> nix::Result<()> {
        match unsafe { fork() } {
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
                } else if args.len() == 1 {
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
        Self { cmd, args }
    }

    fn cmd_as_cstring(&self) -> CString {
        CString::new(self.cmd.as_str()).unwrap()
    }

    fn args_as_cstring(&self) -> Vec<CString> {
        self.args
            .iter()
            .map(|arg| CString::new(arg.as_str()).unwrap())
            .collect()
    }
}

fn main() {
    let mut shell = Shell::new().expect("Failed to spawn shell");
    shell.run().expect("Failed to run shell");
}

mod test {
    use super::*;

    #[test]
    fn test_simple_words() {
        let parser = Parser::new();
        let tokens = parser.tokenize("echo hello world");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into(), Quoting::Unquoted),
                Token::Whitespace,
                Token::Word("hello".into(), Quoting::Unquoted),
                Token::Whitespace,
                Token::Word("world".into(), Quoting::Unquoted),
            ]
        );
    }

    #[test]
    fn test_simple_single_quotes() {
        let parser = Parser::new();
        let tokens = parser.tokenize("echo 'hello world'");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into(), Quoting::Unquoted),
                Token::Whitespace,
                Token::Word("hello world".into(), Quoting::SingleQuoted),
            ]
        );
    }

    #[test]
    fn test_simple_double_quotes() {
        let parser = Parser::new();
        let tokens = parser.tokenize("echo \"hello world\"");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into(), Quoting::Unquoted),
                Token::Whitespace,
                Token::Word("hello world".into(), Quoting::DoubleQuoted),
            ]
        );
    }

    #[test]
    fn test_single_and_double_quotes_with_space() {
        let parser = Parser::new();
        let tokens = parser.tokenize("echo 'hello' \"world\"");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into(), Quoting::Unquoted),
                Token::Whitespace,
                Token::Word("hello".into(), Quoting::SingleQuoted),
                Token::Whitespace,
                Token::Word("world".into(), Quoting::DoubleQuoted),
            ]
        );
    }

    #[test]
    fn test_single_and_double_quotes_without_space() {
        let parser = Parser::new();
        let tokens = parser.tokenize("echo 'hello'\"world\"");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into(), Quoting::Unquoted),
                Token::Whitespace,
                Token::Word("hello".into(), Quoting::SingleQuoted),
                Token::Word("world".into(), Quoting::DoubleQuoted),
            ]
        );
    }

    #[test]
    fn test_single_and_double_inside_eachother() {
        let parser = Parser::new();
        let tokens = parser.tokenize("echo 'he\"llo' \"w'orld\"");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into(), Quoting::Unquoted),
                Token::Whitespace,
                Token::Word("he\"llo".into(), Quoting::SingleQuoted),
                Token::Whitespace,
                Token::Word("w'orld".into(), Quoting::DoubleQuoted),
            ]
        );
    }

    #[test]
    fn test_simple_semicolon() {
        let parser = Parser::new();
        let tokens = parser.tokenize("echo hi;");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into(), Quoting::Unquoted),
                Token::Whitespace,
                Token::Word("hi".into(), Quoting::Unquoted),
                Token::Operator(Operator::Semicolon),
            ]
        );
    }

    #[test]
    fn test_simple_and() {
        let parser = Parser::new();
        let tokens = parser.tokenize("hello && world");
        assert_eq!(
            tokens,
            vec![
                Token::Word("hello".into(), Quoting::Unquoted),
                Token::Whitespace,
                Token::Operator(Operator::And),
                Token::Whitespace,
                Token::Word("world".into(), Quoting::Unquoted),
            ]
        );
    }

    #[test]
    fn test_multiple_whitespace() {
        let parser = Parser::new();
        let tokens = parser.tokenize("echo  hi");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into(), Quoting::Unquoted),
                Token::Whitespace,
                Token::Word("hi".into(), Quoting::Unquoted),
            ]
        );
    }

    #[test]
    fn test_escaping_double_quotes() {
        let parser = Parser::new();
        let tokens = parser.tokenize("echo \\\"hi\\\"");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into(), Quoting::Unquoted),
                Token::Whitespace,
                Token::Word("\"hi\"".into(), Quoting::Unquoted),
            ]
        );
    }

    #[test]
    fn test_escaping_backslash() {
        let parser = Parser::new();
        let tokens = parser.tokenize("echo \\\\");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into(), Quoting::Unquoted),
                Token::Whitespace,
                Token::Word("\\".into(), Quoting::Unquoted),
            ]
        );
    }

    #[test]
    fn test_escaping_newline() {
        let parser = Parser::new();
        let tokens = parser.tokenize("echo \"\\n\"");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into(), Quoting::Unquoted),
                Token::Whitespace,
                Token::Word("\n".into(), Quoting::DoubleQuoted),
            ]
        );
    }
}
