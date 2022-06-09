use chrono::Local;
use nix::errno::Errno;
use nix::sys::wait::{waitpid, WaitStatus};
use nix::unistd::{close, dup2, execvp, fork, pipe, ForkResult};
use std::env;
use std::error::Error;
use std::ffi::CString;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::io::FromRawFd;
use std::process::exit;
use std::thread;

fn main() {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let fmt = args
        .get(0)
        .filter(|arg| arg.starts_with('+'))
        .map(|arg| arg.chars().skip(1).collect());
    if fmt.is_some() {
        args.remove(0);
    }
    if args
        .get(0)
        .map_or(true, |arg| arg == "-h" || arg == "-help" || arg == "--help")
    {
        let progname = env::args()
            .next()
            .and_then(|arg| Some(arg.rsplit('/').next()?.to_string()))
            .unwrap_or_else(|| "annotate-output".to_string());
        println!("Usage: {} [options] program [args ...]", progname);
        println!("  Run program and annotate STDOUT/STDERR with a timestamp.");
        println!();
        println!("  Options:");
        println!("   +FORMAT    - Controls the timestamp format as per date(1)");
        println!("   -h, --help - Show this message");
        exit(0);
    }
    exit(run(&fmt.unwrap_or_else(|| "%H:%M:%S".to_string()), args).unwrap());
}

fn run(fmt: &str, args: Vec<String>) -> Result<i32, Box<dyn Error>> {
    let (ird, iwr) = pipe()?;
    let (erd, ewr) = pipe()?;
    match unsafe { fork() }? {
        ForkResult::Child => {
            close(ird)?;
            close(erd)?;
            if iwr != 1 {
                dup2(iwr, 1)?;
                close(iwr)?;
            }
            if ewr != 2 {
                dup2(ewr, 2)?;
                close(ewr)?;
            }
            let cargs = args
                .into_iter()
                .map(CString::new)
                .collect::<Result<Vec<_>, _>>()?;
            Err(execvp(cargs[0].as_c_str(), &cargs)?.into())
        }
        ForkResult::Parent { child, .. } => {
            close(iwr)?;
            close(ewr)?;
            println!("{} I: Started {}", Local::now().format(fmt), args.join(" "));
            let ithread = {
                let fmt = fmt.to_string();
                let file = unsafe { File::from_raw_fd(ird) };
                thread::spawn(move || annotate(&fmt, "I", &mut BufReader::new(file)))
            };
            let ethread = {
                let fmt = fmt.to_string();
                let file = unsafe { File::from_raw_fd(erd) };
                thread::spawn(move || annotate(&fmt, "E", &mut BufReader::new(file)))
            };
            let rc = loop {
                match waitpid(child, None) {
                    Ok(WaitStatus::Exited(_, rc)) => break Ok(rc),
                    Ok(_) | Err(Errno::EINTR) => {}
                    Err(err) => break Err(err),
                }
            }?;
            let done = Local::now();
            ithread.join().unwrap()?;
            ethread.join().unwrap()?;
            println!("{} I: Finished with exitcode {}", done.format(fmt), rc);
            Ok(rc)
        }
    }
}

fn annotate(fmt: &str, name: &str, input: &mut impl BufRead) -> Result<(), io::Error> {
    let stdout = io::stdout();
    let mut buffer = Vec::<u8>::new();
    loop {
        input.read_until(b'\n', &mut buffer)?;
        if buffer.is_empty() {
            break Ok(());
        }
        let prefix = format!("{} {}: ", Local::now().format(fmt), name);
        let mut stdout = stdout.lock();
        stdout.write_all(prefix.as_bytes())?;
        stdout.write_all(&buffer[..])?;
        if buffer[buffer.len() - 1] != b'\n' {
            stdout.write_all(b"\n")?;
        }
        buffer.clear();
    }
}
