//! Embeds search text on the GPU for `mf`: the model folder is the one argument, the frames go over stdin and stdout.

use std::ffi::OsString;
use std::io::BufWriter;
use std::path::Path;
use std::process::ExitCode;

use modfoundry_search::embed::{check_gpu, BgeSmall};
use modfoundry_search::helper::serve;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let [model] = args.as_slice() else {
        eprintln!("usage: mf-embed-gpu <model dir> | --check");
        return ExitCode::from(2);
    };
    if model == "--check" {
        return match check_gpu() {
            Ok(()) => {
                println!("cuda device ready");
                ExitCode::SUCCESS
            }
            Err(err) => fail(&err),
        };
    }
    let embedder = match BgeSmall::load_gpu(Path::new(model)) {
        Ok(embedder) => embedder,
        Err(err) => return fail(&err),
    };
    let stdout = BufWriter::new(std::io::stdout().lock());
    match serve(&embedder, std::io::stdin().lock(), stdout) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => fail(&err),
    }
}

fn fail(err: &str) -> ExitCode {
    eprintln!("mf-embed-gpu: {err}");
    ExitCode::from(1)
}
