#[cfg(test)]
mod sdk;
mod serve;
mod steam;

use modfoundry_platform::steam::{SteamEvent, SteamRequest};
use std::io::{Read, Write};
use steamworks::Client;

fn main() {
    // steamworks unwraps and asserts inside its setters; a panic still answers with a failure.
    std::panic::set_hook(Box::new(|info| {
        print_event(&SteamEvent::Failed {
            message: format!("the Steam helper failed: {info}"),
        })
    }));
    std::process::exit(run());
}

fn run() -> i32 {
    let mut input = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut input) {
        return fail(format!("bad request: {error}"), 2);
    }
    let request: SteamRequest = match serde_json::from_str(&input) {
        Ok(request) => request,
        Err(error) => return fail(format!("bad request: {error}"), 2),
    };
    let client = match Client::init_app(steam::APP_ID) {
        Ok(client) => client,
        Err(error) => {
            return fail(
                format!("Steam is not running or not logged in ({error})"),
                1,
            )
        }
    };
    let mut workshop = steam::SteamWorkshop::new(client);
    let mut last_failed = false;
    serve::serve(&request, &mut workshop, &mut |event| {
        last_failed = matches!(event, SteamEvent::Failed { .. });
        print_event(&event);
    });
    i32::from(last_failed)
}

fn fail(message: String, code: i32) -> i32 {
    print_event(&SteamEvent::Failed { message });
    code
}

fn print_event(event: &SteamEvent) {
    let mut out = std::io::stdout().lock();
    let line = serde_json::to_string(event).expect("an event always serializes");
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}
