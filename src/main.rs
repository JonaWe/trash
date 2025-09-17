use nix::{sys::wait::waitpid,unistd::{fork, ForkResult, write, execvp}};
use std::io::{self, BufRead, Write};
use std::ffi::CString;
use std::process::exit;


fn parse_input(input: &str) -> Vec<&str> {
    // for now we just split at whitespace
    // TODO: split correctly for quotes etc.
    input.split_whitespace().collect()
}

fn command_handler(command: &str, args: Vec<&str>) {
    match command {
        "exit" => {
            println!("exit");
            exit(0);
        }
        "cd" => {
            println!("The change directory command is not yet implemented");
        }
        _ => run_command(command, args),
    }

}

fn run_command(command: &str, args: Vec<&str>) {
    match unsafe{fork()} {
        Ok(ForkResult::Parent { child, .. }) => {
            waitpid(child, None).unwrap();
        }
        Ok(ForkResult::Child) => {
            // Unsafe to use `println!` (or `unwrap`) here. See Safety.
            // write(std::io::stdout(), "I'm a new child process\n".as_bytes()).ok();
            let c_command = CString::new(command).unwrap();
            let c_args: Vec<CString> = args.iter()
                .map(|str| CString::new(*str).unwrap())
                .collect();
            let _ = execvp(&c_command, &c_args);
            write(std::io::stdout(), "Command not found!\n".as_bytes()).ok();
            std::process::exit(1);
            // unsafe { libc::_exit(0) };
        }
        Err(_) => println!("Fork failed"),
    }
}

fn main() {
    print!("\n");
    loop {
        print!("$ ");
        io::stdout().flush().unwrap();
        let stdin = io::stdin();
        let mut input = String::new();
        if stdin.lock().read_line(&mut input).is_err() {
            continue;
        }

        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        // let arguments: Vec<&str> = input.split_whitespace().collect();
        let arguments = parse_input(input);

        // let command = CString::new(arguments[0]).unwrap();
        // let args: Vec<CString> = arguments.iter()
        //     .map(|s| CString::new(*s).unwrap())
        //     .collect();
        let command = arguments[0];
        let args = arguments;

        command_handler(command, args);


        // println!("{:#?}", command);
        // println!("{:#?}", args);

        // println!("{}", input);
    }
}
