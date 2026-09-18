mod ast;
mod borrowck;
mod codegen_c;
mod diag;
mod driver;
mod externs;
mod infer;
mod lexer;
mod parser;
mod preprocess;
mod pretty;
mod resolve;
mod stdlib;
mod tir;
mod types;

use diag::SourceMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Name this compiler is installed and invoked as. Kept in sync with the
/// crate's `[package] name` and `[[bin]] name` in `Cargo.toml`.
const BIN_NAME: &str = "kelpc";

/// The version `kelpc version` prints, taken from `[package] version` at
/// compile time so it cannot drift from `Cargo.toml`.
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn usage() {
    eprintln!(
        "usage: {BIN_NAME} build <file.{EXT}> [-o <out>]\n       \
         {BIN_NAME} run <file.{EXT}>\n       \
         {BIN_NAME} version",
        BIN_NAME = BIN_NAME,
        EXT = resolve::EXT
    );
}

/// Runs the full pipeline (load modules, typecheck, borrowcheck, codegen,
/// `cc`) and produces a native binary at `out_path`. Returns `false`
/// (having already printed a diagnostic) on any failure.
///
/// `path` may be a `.kp` file or a project directory containing a manifest
/// (in which case the manifest's entry point is used).
fn build(path: &str, out_path: &Path) -> bool {
    let mut sources = SourceMap::new();

    let loaded = match resolve::load(Path::new(path), &mut sources) {
        Ok(l) => l,
        Err(d) => {
            sources.report(&d);
            return false;
        }
    };

    let typed = match infer::check_program(&loaded.program, &loaded.project) {
        Ok(p) => p,
        Err(d) => {
            sources.report(&d);
            return false;
        }
    };

    let drops = match borrowck::check_program(&typed) {
        Ok(plan) => plan,
        Err(d) => {
            sources.report(&d);
            return false;
        }
    };

    let c_source = codegen_c::generate(&typed, &drops, &sources);

    if let Err(e) = driver::compile_c(&c_source, out_path) {
        eprintln!("error: {}", e.message);
        return false;
    }

    true
}

fn default_out_path(source_path: &str) -> PathBuf {
    let stem = Path::new(source_path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "a.out".to_string());
    PathBuf::from(stem)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let command = match args.get(1).map(String::as_str) {
        // `kelpc version` needs no further arguments, so it is answered before
        // the two commands that do.
        Some("version") => {
            println!("{BIN_NAME} {VERSION}");
            return ExitCode::SUCCESS;
        }
        Some(c @ ("build" | "run")) => c,
        _ => {
            usage();
            return ExitCode::FAILURE;
        }
    };
    let path = match args.get(2) {
        Some(p) => p.as_str(),
        None => {
            usage();
            return ExitCode::FAILURE;
        }
    };
    let out_path = match args.iter().position(|a| a == "-o") {
        Some(i) => match args.get(i + 1) {
            Some(p) => PathBuf::from(p),
            None => {
                eprintln!("error: '-o' requires a path argument");
                return ExitCode::FAILURE;
            }
        },
        None => default_out_path(path),
    };

    if !build(path, &out_path) {
        return ExitCode::FAILURE;
    }

    if command == "run" {
        let abs_out = if out_path.is_absolute() {
            out_path.clone()
        } else {
            PathBuf::from("./").join(&out_path)
        };
        match Command::new(&abs_out).status() {
            Ok(status) => {
                return ExitCode::from(status.code().unwrap_or(1) as u8);
            }
            Err(e) => {
                eprintln!("error: could not run '{}': {e}", abs_out.display());
                return ExitCode::FAILURE;
            }
        }
    }

    ExitCode::SUCCESS
}
