//! `hoangsa-ui` — local web UI for hoangsa config + memory browsing.
//!
//! Usage: `hoangsa-ui [project_dir] [--no-open]`. Without `project_dir`,
//! defaults to the current working directory.

use std::path::PathBuf;

fn main() {
    let mut project_dir: Option<PathBuf> = None;
    let mut no_open = false;
    for arg in std::env::args().skip(1) {
        if arg == "--no-open" {
            no_open = true;
        } else if arg == "--help" || arg == "-h" {
            print_help();
            return;
        } else if !arg.starts_with('-') {
            project_dir = Some(PathBuf::from(arg));
        }
    }

    let project_dir = project_dir.unwrap_or_else(|| {
        std::env::current_dir().expect("cwd is readable")
    });

    if !project_dir.exists() {
        eprintln!("project_dir does not exist: {}", project_dir.display());
        std::process::exit(1);
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime builds");

    let opts = hoangsa_ui_server::RunOptions {
        project_dir,
        open_browser: !no_open,
    };

    if let Err(e) = runtime.block_on(hoangsa_ui_server::run(opts)) {
        eprintln!("ui server error: {e}");
        std::process::exit(1);
    }
}

fn print_help() {
    println!(
        "hoangsa-ui — local web UI for hoangsa\n\
         \n\
         Usage:\n  \
           hoangsa-ui [project_dir] [--no-open]\n  \
           hoangsa-ui --help\n\
         \n\
         Args:\n  \
           project_dir   Directory whose .hoangsa/ to manage (default: cwd)\n  \
           --no-open     Don't auto-open the browser; just print the URL\n"
    );
}
