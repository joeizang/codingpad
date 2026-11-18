use std::{env, path::PathBuf, process::ExitCode};

const USAGE: &str = "\
Usage: cpad run [--html] <script>

Run a CodingPad script from the terminal and print its dumps as text.
Supports Python (.py), C# (.cs) and SQL (.sql) scripts. A header such as
\"# codingpad: connection=<name>\" attaches a saved connection to the script.

Options:
  --html  Print one standalone HTML page instead of text (for example
          redirect it to a file: cpad run --html report.py > report.html).

Environment:
  CODINGPAD_DATA_DIR  Override the CodingPad data folder (connections, secrets
                      and private toolchains).

Exit codes: 0 success, 1 script failure, 2 usage or script type error,
            130 stopped by Ctrl-C.
";

#[tokio::main]
async fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    let Some(command) = args.next() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    match command.to_str() {
        Some("run") => {
            let mut format = cpad::OutputFormat::Text;
            let mut script: Option<PathBuf> = None;
            let mut options = true;
            for argument in args {
                let text = argument.to_string_lossy();
                if options && text == "--" {
                    options = false;
                } else if options && text == "--html" {
                    format = cpad::OutputFormat::Html;
                } else if options && (text == "-h" || text == "--help") {
                    print!("{USAGE}");
                    return ExitCode::SUCCESS;
                } else if options && text.starts_with('-') {
                    eprintln!("Unknown option \"{text}\".\n");
                    eprint!("{USAGE}");
                    return ExitCode::from(2);
                } else if script.is_some() {
                    eprintln!("cpad run takes one script at a time.\n");
                    eprint!("{USAGE}");
                    return ExitCode::from(2);
                } else {
                    script = Some(PathBuf::from(argument));
                }
            }
            let Some(script) = script else {
                eprintln!("cpad run needs a script path.\n");
                eprint!("{USAGE}");
                return ExitCode::from(2);
            };
            ExitCode::from(run(script, format).await as u8)
        }
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("-V" | "--version" | "version") => {
            println!("cpad {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("Unknown command \"{other}\".\n");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
        None => {
            // A non-UTF-8 command can never be a valid command name.
            eprintln!("Unknown command {:?}.\n", command);
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

async fn run(script: PathBuf, format: cpad::OutputFormat) -> i32 {
    cpad::run_file_with_format(&script, format).await
}
