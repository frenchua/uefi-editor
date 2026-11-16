//! `efivars-tui`: browse and edit UEFI variables in the terminal.

mod app;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

use efivars_core::decode::{self, Field};
use efivars_core::store::DirStore;
use efivars_core::{VarStore, attrs, guids};

const USAGE: &str = "\
usage: efivars-tui [--dir <path>] [--write] [--list | --dump <path>]

  --dir <path>   use a directory of efivarfs-format files instead of firmware
  --write        allow edits (otherwise read-only)
  --list         print every variable, decoded, and exit
  --dump <path>  save every variable to <path> in efivarfs format and exit

On Windows run from an elevated terminal; on Linux run as root to see and
write every variable.";

struct Args {
    dir: Option<PathBuf>,
    write: bool,
    list: bool,
    dump: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { dir: None, write: false, list: false, dump: None };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dir" => args.dir = Some(it.next().ok_or("--dir needs a path")?.into()),
            "--dump" => args.dump = Some(it.next().ok_or("--dump needs a path")?.into()),
            "--write" => args.write = true,
            "--list" => args.list = true,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(args)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("error: {msg}\n");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    let store: Box<dyn VarStore> = match &args.dir {
        Some(dir) => Box::new(DirStore::new(dir)),
        None => match efivars_sys::open_system() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot open firmware variables: {e}");
                return ExitCode::FAILURE;
            }
        },
    };

    let result = if args.list {
        print_list(store)
    } else if let Some(path) = &args.dump {
        dump(store, path)
    } else {
        app::App::new(store, args.write).and_then(app::run)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_list(mut store: Box<dyn VarStore>) -> std::io::Result<()> {
    let mut vars = store.list()?;
    vars.sort_by(|a, b| a.key.cmp(&b.key));
    for v in &vars {
        let vendor = guids::vendor_name(&v.key.vendor).map(|n| format!(" ({n})")).unwrap_or_default();
        println!("{}-{}{vendor} [{}] {} bytes", v.key.name, v.key.vendor, attrs::describe(v.attributes), v.data.len());
        match decode::decode(v) {
            None => {}
            Some(Err(e)) => println!("    !! {e}"),
            Some(Ok(d)) => {
                let mut flat = Vec::new();
                Field::flatten(&d.fields, 0, &mut flat);
                for (depth, f) in flat {
                    println!("    {}{}: {}", "  ".repeat(depth), f.label, f.value);
                }
            }
        }
    }
    println!("{} variable(s) from {}", vars.len(), store.describe());
    Ok(())
}

fn dump(mut store: Box<dyn VarStore>, path: &std::path::Path) -> std::io::Result<()> {
    let n = DirStore::dump(path, &store.list()?)?;
    println!("dumped {n} variable(s) to {}", path.display());
    Ok(())
}
