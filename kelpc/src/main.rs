mod ast;
mod backend;
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

use backend::Backend;
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
        "usage: {BIN_NAME} build <file.{EXT}> [-o <out>] [--backend=<name>]\n       \
         {BIN_NAME} run <file.{EXT}>\n       \
         {BIN_NAME} version\n\n\
         The backend may also be chosen with the KELPC_BACKEND environment\n\
         variable; an explicit --backend wins over it. The default is 'c'.",
        BIN_NAME = BIN_NAME,
        EXT = resolve::EXT
    );
}

/// Runs the full pipeline (load modules, typecheck, borrowcheck, codegen,
/// finish the artifact) and produces a native binary at `out_path`. Returns
/// `false` (having already printed a diagnostic) on any failure.
///
/// `path` may be a `.kp` file or a project directory containing a manifest
/// (in which case the manifest's entry point is used).
///
/// Everything before `backend` is backend-independent, which is why the suite
/// that exercises it does not care which backend is selected; the seam is the
/// one `match` below, and it is the only place a code generator is named.
fn build(path: &str, out_path: &Path, backend: Backend) -> bool {
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

    // The backend seam (MK7 stage 1). One arm today. A second backend adds its
    // own arm here, and the fixture suite already knows how to validate it: it
    // is backend-agnostic by construction, so a second run with
    // `KELPC_BACKEND=<name>` exercises the same 248 fixtures against it.
    match backend {
        Backend::C => {
            let c_source = codegen_c::generate(&typed, &drops, &sources);
            if let Err(e) = driver::compile_c(&c_source, out_path) {
                eprintln!("error: {}", e.message);
                return false;
            }
        }
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
    // A scan rather than a positional read: `--backend` may come before or
    // after the source path, and both it and `-o` take a value. The source
    // path is the first argument that is neither.
    let mut path: Option<String> = None;
    let mut out: Option<PathBuf> = None;
    let mut backend_flag: Option<String> = None;
    let mut i = 2;
    while i < args.len() {
        let arg = args[i].as_str();
        let takes_value = arg == "-o" || arg == "--backend";
        let inline = arg
            .strip_prefix("--backend=")
            .or_else(|| arg.strip_prefix("-o="));
        if takes_value {
            let value = match args.get(i + 1) {
                Some(v) => v.clone(),
                None => {
                    eprintln!("error: '{arg}' requires a value");
                    return ExitCode::FAILURE;
                }
            };
            if arg == "-o" {
                out = Some(PathBuf::from(value));
            } else {
                backend_flag = Some(value);
            }
            i += 2;
        } else if let Some(value) = inline {
            if arg.starts_with("--backend=") {
                backend_flag = Some(value.to_string());
            } else {
                out = Some(PathBuf::from(value));
            }
            i += 1;
        } else if path.is_none() {
            path = Some(arg.to_string());
            i += 1;
        } else {
            eprintln!("error: unexpected argument '{arg}'");
            usage();
            return ExitCode::FAILURE;
        }
    }

    let path = match path {
        Some(p) => p,
        None => {
            usage();
            return ExitCode::FAILURE;
        }
    };

    // The flag wins over the environment, so one-off runs are possible without
    // editing whatever set the variable. A non-UTF-8 value reads as unset, the
    // same as an empty one being reported rather than silently defaulting.
    let env_backend = std::env::var("KELPC_BACKEND").ok();
    let backend = match backend::resolve(backend_flag.as_deref(), env_backend.as_deref()) {
        Ok(b) => b,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };

    let out_path = out.unwrap_or_else(|| default_out_path(&path));

    if !build(&path, &out_path, backend) {
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
