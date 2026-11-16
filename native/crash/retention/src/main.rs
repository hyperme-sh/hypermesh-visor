fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = match args.as_slice() {
        [action, path] if action == "register" => {
            hypermesh_crash_retention::Owner::register(std::path::Path::new(path))
        }
        [action] if action == "service" => hypermesh_crash_retention::platform::serve(),
        _ => Err(hypermesh_crash_retention::refused()),
    };
    if result.is_err() {
        eprintln!("Crash cleanup service unavailable; raw capture must remain off");
        std::process::exit(1);
    }
}
